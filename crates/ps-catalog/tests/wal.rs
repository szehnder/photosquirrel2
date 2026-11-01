//! The write-ahead log stays small (catalog-wal-size, ADR-0122): a reset log is cut back to 64 MiB; migrations, run
//! ends and close empty it; a catalog or thumbnail store opened with a log over 8 MiB empties it in the background.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::*;
use ps_catalog::wal::{SHRINK_AT_OPEN, wal_len};
use ps_catalog::{Catalog, Command, ThumbStore, db};
use ps_model::{ContentId, RunStatus, ThumbKind, ThumbnailRecord};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};

const ROWS: u64 = 30_000;
const MIB: u64 = 1 << 20;

/// Commit `mib` MiB of pages to the log of the database at `path` and close without checkpointing, as a process that
/// is killed (or never closes its connections, like the app at quit) leaves it.
fn leave_log(path: &Path, mib: u64) {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .unwrap();
    conn.execute_batch(&format!(
        "PRAGMA wal_autocheckpoint = 0;
         CREATE TABLE junk(b BLOB);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {})
         INSERT INTO junk SELECT randomblob(4000) FROM n;
         DROP TABLE junk;",
        mib * 256
    ))
    .unwrap();
    drop(conn);
    assert!(
        wal_len(path) >= mib * MIB,
        "the log holds {} bytes",
        wal_len(path)
    );
}

fn count(path: &Path, sql: &str) -> i64 {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// A catalog at `path` with `ROWS` analysed rows, closed.
fn catalog_with_rows(dir: &Path) -> std::path::PathBuf {
    let cat = Catalog::open(dir).unwrap();
    let src = add_source(&cat, "/mnt/a");
    write_rows(&cat, src, 0..ROWS, 2, full_row);
    let path = cat.path().to_path_buf();
    cat.close().unwrap();
    path
}

#[test]
fn scenario_catalog_migrated_catalog_leaves_an_empty_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = catalog_with_rows(dir.path());
    // Back to the schema before the colour migration (ADR-0113), folded into the main file.
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "ALTER TABLE file DROP COLUMN colour;
             PRAGMA user_version = {};",
            db::COLOUR_MIGRATION.version - 1
        ))
        .unwrap();
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    }
    let analysed = count(
        &path,
        "SELECT count(*) FROM file WHERE content_id IS NOT NULL",
    );
    assert!(analysed > 0);

    let cat = Catalog::open(dir.path()).unwrap();
    let shrink = cat
        .wait_for_wal_shrink()
        .expect("a migration starts the checkpoint");
    assert!(shrink.complete, "{shrink:?}");
    assert!(shrink.before > 0, "{shrink:?}");
    assert_eq!(wal_len(&path), 0);
    assert_eq!(count(&path, "SELECT count(*) FROM file"), ROWS as i64);
    assert_eq!(
        count(
            &path,
            "SELECT count(*) FROM file WHERE colour = 'unchecked'"
        ),
        analysed
    );
    cat.close().unwrap();
}

#[test]
fn scenario_catalog_large_leftover_log_emptied_after_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = catalog_with_rows(dir.path());
    let sum = "SELECT count(*) + total(size) FROM file";
    let before = count(&path, "SELECT count(*) FROM file");
    let size: f64 = {
        let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        c.query_row(sum, [], |r| r.get(0)).unwrap()
    };
    leave_log(&path, 40);
    assert!(wal_len(&path) > 32 * MIB);

    let cat = Catalog::open(dir.path()).unwrap();
    // Opening returned without folding the log (the shrink waits for its delay), and the readers answer meanwhile.
    assert_eq!(count(cat.path(), "SELECT count(*) FROM file"), before);
    assert!(
        wal_len(&path) > 32 * MIB,
        "opening did not wait for the log to be folded: {}",
        wal_len(&path)
    );
    let shrink = cat
        .wait_for_wal_shrink()
        .expect("a large log starts the checkpoint");
    assert!(shrink.complete, "{shrink:?}");
    assert!(shrink.before > 32 * MIB, "{shrink:?}");
    assert_eq!(shrink.after, 0);
    assert_eq!(wal_len(&path), 0);
    let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let after: f64 = c.query_row(sum, [], |r| r.get(0)).unwrap();
    assert_eq!(after, size);
    drop(c);
    cat.close().unwrap();
}

fn thumb(seed: u8) -> ThumbnailRecord {
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg.extend((0..20_000u32).map(|i| (i as u8).wrapping_mul(seed)));
    jpeg.extend([0xFF, 0xD9]);
    ThumbnailRecord {
        kind: ThumbKind::Image,
        width: 384,
        height: 288,
        dominant_rgb: [seed, 40, 200],
        jpeg,
        placeholder: None,
    }
}

#[test]
fn scenario_catalog_large_leftover_thumbnail_log_emptied_after_open() {
    let dir = tempfile::tempdir().unwrap();
    let ids: Vec<ContentId> = (1..=50u8).map(|i| ContentId::full([i; 32])).collect();
    let path = {
        let store = ThumbStore::open(dir.path()).unwrap();
        let thumbs: Vec<ThumbnailRecord> = (1..=50u8).map(thumb).collect();
        store.put_many(ids.iter().zip(&thumbs)).unwrap();
        store.path().to_path_buf()
    };
    leave_log(&path, 40);

    let store = ThumbStore::open(dir.path()).unwrap();
    let shrink = store
        .wait_for_wal_shrink()
        .expect("a large log starts the checkpoint");
    assert!(shrink.complete, "{shrink:?}");
    assert_eq!(wal_len(&path), 0);
    for (i, id) in ids.iter().enumerate() {
        let t = store.get(id).unwrap().expect("stored thumbnail");
        assert_eq!(t, thumb(i as u8 + 1));
    }
}

#[test]
fn scenario_catalog_small_leftover_log_left_to_normal_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let path = catalog_with_rows(dir.path());
    leave_log(&path, 2);
    assert!(wal_len(&path) < SHRINK_AT_OPEN);
    let cat = Catalog::open(dir.path()).unwrap();
    assert_eq!(cat.wait_for_wal_shrink(), None);
    cat.close().unwrap();
    // A new catalog starts none either.
    let fresh = tempfile::tempdir().unwrap();
    let cat = Catalog::open(fresh.path()).unwrap();
    assert_eq!(cat.wait_for_wal_shrink(), None);
}

#[test]
fn scenario_catalog_scan_run_end_empties_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    let run = cat.writer().start_run(vec![src]).unwrap();
    write_rows(&cat, src, 0..ROWS, 2, full_row);
    // A reader has read through the log.
    assert_eq!(count(cat.path(), "SELECT count(*) FROM file"), ROWS as i64);
    assert!(wal_len(cat.path()) > MIB, "{}", wal_len(cat.path()));
    cat.writer()
        .submit(Command::SetRunStatus {
            run,
            status: RunStatus::Completed,
            stats: None,
        })
        .unwrap();
    cat.writer().flush().unwrap();
    // The checkpoint follows the final flush once the queue is quiet (it is not part of the flush).
    wait_until(|| wal_len(cat.path()) == 0);
    assert_eq!(count(cat.path(), "SELECT count(*) FROM file"), ROWS as i64);
}

/// Poll `f` for up to 20 s.
fn wait_until(f: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "condition not met");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The run's final status and flush (the scanner's cancel path) do not wait for readers that hold the log: the log
/// checkpoint follows them. A reader inside a transaction would make a checkpoint inside the status write wait for
/// its whole busy timeout; the flush must return long before that.
#[test]
fn the_run_end_checkpoint_does_not_delay_the_final_flush() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    // Hold a read transaction open on its own connection for the whole test.
    let reader = Connection::open_with_flags(cat.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM file;")
        .unwrap();
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        let run = cat.writer().start_run(vec![src]).unwrap();
        write_rows(&cat, src, 0..2_000, 2, full_row);
        let t = std::time::Instant::now();
        cat.writer()
            .submit(Command::SetRunStatus {
                run,
                status: RunStatus::Cancelled,
                stats: None,
            })
            .unwrap();
        cat.writer().flush().unwrap();
        best = best.min(t.elapsed());
        // Let the checkpoint (which waits for the reader) pass before the next round.
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    // A checkpoint inside the status write would take at least its 100 ms wait every time.
    assert!(
        best < std::time::Duration::from_millis(90),
        "the final flush took {best:?}"
    );
    reader.execute_batch("COMMIT").unwrap();
}

#[test]
fn scenario_catalog_closed_catalog_leaves_an_empty_log() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    write_rows(&cat, src, 0..ROWS, 2, full_row);
    assert_eq!(count(cat.path(), "SELECT count(*) FROM file"), ROWS as i64);
    let path = cat.path().to_path_buf();
    assert!(wal_len(&path) > MIB, "{}", wal_len(&path));
    // `close` stops the writer while the reader pool is still open (it closes last and cannot checkpoint).
    cat.close().unwrap();
    assert_eq!(wal_len(&path), 0);
}

/// Opening a thumbnail store whose log file is large but holds no committed frames (a reset log keeps its size)
/// starts no background checkpoint, so it does not run again at every launch.
#[test]
fn a_reset_thumbnail_log_starts_no_checkpoint_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = ThumbStore::open(dir.path()).unwrap().path().to_path_buf();
    {
        let conn = Connection::open(&path).unwrap();
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .unwrap();
        conn.pragma_update(None, "journal_size_limit", -1).unwrap();
        conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        conn.execute_batch(
            "CREATE TABLE junk(b BLOB);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2600)
             INSERT INTO junk SELECT randomblob(4000) FROM n;",
        )
        .unwrap();
        conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
            .unwrap();
        conn.execute_batch("CREATE TABLE small(x);").unwrap();
    }
    assert!(wal_len(&path) > SHRINK_AT_OPEN, "{}", wal_len(&path));
    let store = ThumbStore::open(dir.path()).unwrap();
    assert_eq!(store.wait_for_wal_shrink(), None);
}

/// The background shrink holds the write lock only for short tries, below the writer's busy timeout: writes made while
/// it runs all commit, and readers answer throughout.
#[test]
fn writes_during_the_background_shrink_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let path = catalog_with_rows(dir.path());
    leave_log(&path, 40);
    let cat = Catalog::open(dir.path()).unwrap();
    let src = cat.read().sources().unwrap()[0].id;
    std::thread::scope(|s| {
        let shrink = s.spawn(|| cat.wait_for_wal_shrink());
        for start in (ROWS..ROWS + 10_000).step_by(500) {
            cat.writer()
                .upsert_files((start..start + 500).map(|i| full_row(src, i)).collect())
                .unwrap();
            cat.writer().flush().unwrap();
            assert!(count(cat.path(), "SELECT count(*) FROM file") >= start as i64);
        }
        let r = shrink.join().unwrap().expect("started");
        assert!(r.before > 32 * MIB, "{r:?}");
    });
    assert_eq!(
        count(cat.path(), "SELECT count(*) FROM file"),
        (ROWS + 10_000) as i64
    );
    cat.close().unwrap();
    assert_eq!(wal_len(&path), 0);
}
