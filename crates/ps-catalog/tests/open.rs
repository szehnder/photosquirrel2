//! Opening, data directory and migrations (catalog tasks 1.1-1.3).

use std::path::{Path, PathBuf};

use ps_catalog::{Catalog, CatalogError, db, latest_schema_version};
use rusqlite::Connection;
use rusqlite::types::Value;

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn scenario_catalog_catalog_created_in_data_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("data");
    let source = tmp.path().join("source");
    std::fs::create_dir_all(&source).expect("mkdir");
    std::fs::write(source.join("IMG_0001.JPG"), b"\xFF\xD8\xFF").expect("write");
    let before_source = listing(&source);

    let cat = Catalog::open(&data).expect("open");
    assert_eq!(cat.path(), data.join("catalog.db"));
    assert!(data.join("catalog.db").is_file());
    let mode: String = cat
        .read()
        .with(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
        .expect("journal mode");
    assert_eq!(mode, "wal");
    let raw_mode: String = Connection::open(data.join("catalog.db"))
        .expect("raw")
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .expect("raw journal mode");
    assert_eq!(raw_mode, "wal", "WAL is persistent in the file");

    // Only catalog files in the data directory; the source tree and the temp root are untouched.
    for f in listing(&data) {
        assert!(
            [
                "catalog.db",
                "catalog.db-wal",
                "catalog.db-shm",
                ps_catalog::LOCK_FILE
            ]
            .contains(&f.as_str()),
            "unexpected file {f} in data dir"
        );
    }
    cat.close().expect("close");
    assert_eq!(listing(&source), before_source);
    assert_eq!(listing(tmp.path()), vec!["data", "source"]);
}

#[test]
fn data_dir_override_and_default() {
    let p = PathBuf::from("/tmp/ps-data");
    assert_eq!(ps_catalog::data_dir_from(Some(p.clone())).expect("dir"), p);
    // An empty override falls back to the per-user directory.
    let d = ps_catalog::data_dir_from(Some(PathBuf::new())).expect("default dir");
    assert!(
        d.to_string_lossy().to_lowercase().contains("photosquirrel"),
        "{}",
        d.display()
    );
}

/// The environment override, checked in a child process so no test mutates this process's environment.
#[test]
fn data_dir_env_override() {
    let exe = std::env::current_exe().expect("exe");
    let out = std::process::Command::new(exe)
        .args([
            "env_probe_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ps_catalog::DATA_DIR_ENV, "/tmp/ps-env-data")
        .output()
        .expect("child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("DATA_DIR=/tmp/ps-env-data"), "{stdout}");
}

#[test]
#[ignore = "child process of data_dir_env_override"]
fn env_probe_child() {
    if let Ok(d) = ps_catalog::data_dir() {
        println!("DATA_DIR={}", d.display());
    }
}

fn tables(conn: &Connection) -> Vec<String> {
    let mut s = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .expect("prepare");
    s.query_map([], |r| r.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("rows")
}

#[test]
fn scenario_catalog_fresh_catalog_migrated_to_latest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let latest = latest_schema_version();
    assert_eq!(latest, db::CATALOG_MIGRATIONS.len() as u32);
    assert_eq!(cat.read().schema_version().expect("version"), latest);
    let names = cat.read().with(|c| Ok(tables(c))).expect("tables");
    for t in ["file", "file_error", "scan_run", "source"] {
        assert!(names.iter().any(|n| n == t), "missing table {t}: {names:?}");
    }
    cat.close().expect("close");
    // Reopening is a no-op.
    let cat = Catalog::open(tmp.path()).expect("reopen");
    assert_eq!(cat.read().schema_version().expect("version"), latest);
}

fn snapshot(path: &Path) -> Vec<Vec<Value>> {
    let conn = Connection::open(path).expect("open raw");
    let mut s = conn
        .prepare("SELECT * FROM file ORDER BY id")
        .expect("prepare");
    let n = s.column_count();
    s.query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect())
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("rows")
}

#[test]
fn scenario_catalog_old_catalog_upgraded() {
    // The version-1 fixture is generated from migration 1 only (design: not committed).
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("catalog.db");
    db::create_catalog_at_version(&path, 1).expect("v1 fixture");
    {
        let mut conn = Connection::open(&path).expect("raw");
        let v: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("version");
        assert_eq!(v, 1);
        let tx = conn.transaction().expect("tx");
        tx.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, X'2F6D6E74', 0)",
            [],
        )
        .expect("source");
        for i in 0..1000i64 {
            tx.execute(
                "INSERT INTO file (source_id, rel_path, name, size, mtime_ns, file_index, stage, class,
                                   format, media_kind, width, height, content_id, identity_kind, capture_ts,
                                   metadata)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, 2, 0, 'jpeg', 0, 4000, 3000, ?6, 0, ?7, '{}')",
                rusqlite::params![
                    format!("a/IMG_{i}.JPG").into_bytes(),
                    format!("IMG_{i}.JPG").into_bytes(),
                    1000 + i,
                    i * 1_000_000_007,
                    i,
                    vec![(i % 251) as u8; 32],
                    1_200_000_000_000 + i * 997,
                ],
            )
            .expect("row");
        }
        tx.commit().expect("commit");
    }
    let before = snapshot(&path);
    assert_eq!(before.len(), 1000);

    let cat = Catalog::open_file(&path, Default::default()).expect("upgrade");
    assert_eq!(
        cat.read().schema_version().expect("version"),
        latest_schema_version()
    );
    assert!(latest_schema_version() > 1);
    cat.close().expect("close");
    // Later migrations may add columns (3: `first_seen`); the existing columns are unchanged and the new
    // ones are NULL for old rows, except migration 17's two columns with defaults (the last two: capture
    // precision 0 = second, date version 1 = the rules before ADR-0069/0070; none of these rows has an EXIF
    // provenance, so none is marked current), then migration 19's `decoder_missing` (NULL: none is a HEIC or
    // AVIF waiting for a decoder), migration 20's `decode_failure` (NULL: no failure recorded) and migration 21's
    // `exclusion` (NULL: every row is a photo), and migration 23's `frame_id` (NULL) and `meta_version` (1: a
    // JPEG is marked current without reading it).
    let width = before[0].len();
    let after = snapshot(&path);
    assert!(
        after.iter().all(|r| {
            let added = &r[width..];
            let (nullable, rest) = added.split_at(added.len() - 7);
            nullable.iter().all(|v| *v == Value::Null)
                && rest
                    == [
                        Value::Integer(0),
                        Value::Integer(1),
                        Value::Null,
                        Value::Null,
                        Value::Null,
                        Value::Null,
                        Value::Integer(1),
                    ]
        }),
        "added columns are empty (or at their defaults) for old rows"
    );
    let after: Vec<Vec<Value>> = after
        .into_iter()
        .map(|mut r| {
            r.truncate(width);
            r
        })
        .collect();
    assert_eq!(after, before, "rows unchanged by the upgrade");
}

#[test]
fn scenario_catalog_newer_catalog_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("catalog.db");
    Catalog::open_file(&path, Default::default())
        .expect("create")
        .close()
        .expect("close");
    let newer = latest_schema_version() + 1;
    {
        let conn = Connection::open(&path).expect("raw");
        conn.pragma_update(None, "user_version", newer)
            .expect("bump");
        // Fold the WAL back into the main file so the bytes below are the whole database.
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .expect("checkpoint");
    }
    let bytes = std::fs::read(&path).expect("read");

    let err = Catalog::open_file(&path, Default::default()).expect_err("must refuse");
    match &err {
        CatalogError::NewerSchema {
            found, supported, ..
        } => {
            assert_eq!(*found, newer);
            assert_eq!(*supported, latest_schema_version());
        }
        other => panic!("unexpected error {other:?}"),
    }
    let msg = err.to_string();
    assert!(msg.contains(&format!("version {newer}")), "{msg}");
    assert!(
        msg.contains(&format!("version {}", latest_schema_version())),
        "{msg}"
    );
    assert_eq!(std::fs::read(&path).expect("read"), bytes, "file unchanged");
}
