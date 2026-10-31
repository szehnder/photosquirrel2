//! The write-ahead log policy of the catalog and the thumbnail store (ADR-0122).
//!
//! - Every writer connection sets `journal_size_limit` to [`WAL_SIZE_LIMIT`]: a log that is reset is cut back to
//!   it, so a large transaction (a migration) cannot leave a file of its size behind for good.
//! - [`truncate`] folds the log into the database and empties it, waiting at most a given time for readers; when
//!   the wait runs out SQLite carries on as a PASSIVE checkpoint and reports `busy`, and nothing fails. The writer
//!   uses it at the end of each scan run and at shutdown.
//! - A background thread does the same when a catalog or store is opened with a log over [`SHRINK_AT_OPEN`] or
//!   after a migration, [`SHRINK_DELAY`] after opening: a PASSIVE pass (the long copy, no write lock) and then a
//!   short TRUNCATE. Opening never waits for it.
//!
//! A leftover log costs its WAL recovery at every open (the first connection rebuilds the log index from every
//! committed page in it): 1.09 GB of migration log made the first gallery frame of a 1M catalog 1.2 s instead of
//! 0.48 s.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OpenFlags};

/// `journal_size_limit` of every writer connection: one automatic checkpoint interval (`wal_autocheckpoint`
/// 16,384 pages of 4 KiB), so normal scans do not shrink and regrow the file at every reset.
pub const WAL_SIZE_LIMIT: i64 = 64 << 20;

/// A log larger than this when a catalog or thumbnail store is opened is folded and truncated in the background.
/// WAL recovery costs about 0.66 ms per MiB of committed log at each open, so 8 MiB is about 5 ms.
pub const SHRINK_AT_OPEN: u64 = 8 << 20;

/// The busy timeout of every catalog connection.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the TRUNCATE at the end of a scan run waits for readers (the next queued run must not stall).
pub(crate) const RUN_END_WAIT: Duration = Duration::from_millis(250);

/// How long the TRUNCATE at writer shutdown waits for readers (quitting must not be held up).
pub(crate) const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

/// How long each TRUNCATE try of the background shrink waits. It holds the write lock meanwhile, so it must stay
/// well below [`BUSY_TIMEOUT`], the writer's own wait for that lock.
const SHRINK_WAIT: Duration = Duration::from_secs(1);
const SHRINK_TRIES: u32 = 10;
const SHRINK_PAUSE: Duration = Duration::from_millis(250);

/// The row of `PRAGMA wal_checkpoint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalCheckpoint {
    /// The checkpoint could not finish (a reader or another checkpoint was in the way); it copied what it could.
    pub busy: bool,
    /// Frames in the log before the checkpoint (`-1` when the database is not in WAL mode).
    pub log_frames: i64,
    /// Frames copied into the database.
    pub checkpointed: i64,
}

/// Run `PRAGMA wal_checkpoint(<mode>)`.
pub(crate) fn checkpoint(conn: &Connection, mode: &str) -> rusqlite::Result<WalCheckpoint> {
    conn.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |r| {
        Ok(WalCheckpoint {
            busy: r.get::<_, i64>(0)? != 0,
            log_frames: r.get(1)?,
            checkpointed: r.get(2)?,
        })
    })
}

/// Fold the log into the database and truncate it to zero bytes, waiting at most `wait` for readers (and for a
/// writer, on a connection that is not the writer). The connection's busy timeout is [`BUSY_TIMEOUT`] again
/// afterwards.
pub(crate) fn truncate(conn: &Connection, wait: Duration) -> rusqlite::Result<WalCheckpoint> {
    conn.busy_timeout(wait)?;
    let r = checkpoint(conn, "TRUNCATE");
    conn.busy_timeout(BUSY_TIMEOUT)?;
    r
}

/// The write-ahead log file of the database at `db`.
pub fn wal_path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-wal");
    PathBuf::from(p)
}

/// The size of the write-ahead log file of the database at `db` (0 when there is none).
pub fn wal_len(db: &Path) -> u64 {
    std::fs::metadata(wal_path(db)).map_or(0, |m| m.len())
}

/// How a background shrink ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalShrink {
    /// Log file size when it started, in bytes.
    pub before: u64,
    /// Log file size when it ended, in bytes (0 when `complete`).
    pub after: u64,
    /// The log was folded and truncated (else readers kept it busy for every try, or it failed; the log is then
    /// folded as far as they allowed and truncated to [`WAL_SIZE_LIMIT`] at its next reset).
    pub complete: bool,
    pub elapsed: Duration,
}

/// How long after opening the background shrink starts. Its copy competes with the first gallery query for the disk
/// (measured on a 1M catalog with a 1 GB log: the first ID list took 2.8 s instead of 0.15 s while it ran), so it
/// waits until the first frame and its thumbnails are drawn.
pub const SHRINK_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    Waiting,
    Go,
    Cancelled,
}

#[derive(Debug)]
struct Shared {
    gate: Mutex<Gate>,
    cv: Condvar,
}

impl Shared {
    /// Move a waiting shrink on to `to` (a started or cancelled one stays as it is).
    fn release(&self, to: Gate) {
        let mut g = self.gate.lock();
        if *g == Gate::Waiting {
            *g = to;
        }
        self.cv.notify_all();
    }
}

/// The background shrink of one open catalog or store. It starts [`SHRINK_DELAY`] after opening, or at once when
/// someone waits for it ([`ShrinkTask::wait`]). Dropping it cancels a shrink that has not started yet and joins one
/// that has, before the owner's connections close and its lock is released, so no connection of it outlives the
/// owner. (The catalog's writer empties the log when it shuts down anyway.)
#[derive(Debug, Default)]
pub(crate) struct ShrinkTask {
    shared: Option<Arc<Shared>>,
    handle: Mutex<Option<JoinHandle<Option<WalShrink>>>>,
    result: Mutex<Option<WalShrink>>,
}

impl ShrinkTask {
    /// No shrink (observers, new files, small logs).
    pub(crate) fn none() -> ShrinkTask {
        ShrinkTask::default()
    }

    /// Start a shrink of the database at `path` when `migrated` or its log is over [`SHRINK_AT_OPEN`].
    pub(crate) fn start_if_needed(path: &Path, migrated: bool) -> ShrinkTask {
        if !migrated && wal_len(path) <= SHRINK_AT_OPEN {
            return ShrinkTask::none();
        }
        let shared = Arc::new(Shared {
            gate: Mutex::new(Gate::Waiting),
            cv: Condvar::new(),
        });
        let gate = shared.clone();
        let p = path.to_path_buf();
        match std::thread::Builder::new()
            .name("catalog-wal".into())
            .spawn(move || {
                let deadline = Instant::now() + SHRINK_DELAY;
                let mut g = gate.gate.lock();
                while *g == Gate::Waiting {
                    if gate.cv.wait_until(&mut g, deadline).timed_out() {
                        *g = Gate::Go;
                    }
                }
                let go = *g == Gate::Go;
                drop(g);
                go.then(|| shrink(&p))
            }) {
            Ok(h) => ShrinkTask {
                shared: Some(shared),
                handle: Mutex::new(Some(h)),
                result: Mutex::new(None),
            },
            Err(e) => {
                log::warn!(
                    "{}: could not start the log checkpoint: {e}",
                    path.display()
                );
                ShrinkTask::none()
            }
        }
    }

    /// Start the shrink now if it is still waiting, wait for it to end and return how it ended; `None` when none
    /// was started (or it was cancelled).
    pub(crate) fn wait(&self) -> Option<WalShrink> {
        self.finish(Gate::Go)
    }

    fn finish(&self, to: Gate) -> Option<WalShrink> {
        let shared = self.shared.as_ref()?;
        shared.release(to);
        if let Some(h) = self.handle.lock().take() {
            *self.result.lock() = h.join().ok().flatten();
        }
        *self.result.lock()
    }
}

impl Drop for ShrinkTask {
    fn drop(&mut self) {
        let _ = self.finish(Gate::Cancelled);
    }
}

fn shrink(path: &Path) -> WalShrink {
    let started = Instant::now();
    let before = wal_len(path);
    let complete = shrink_on_own_connection(path).unwrap_or_else(|e| {
        log::warn!("{}: log checkpoint failed: {e}", path.display());
        false
    });
    let r = WalShrink {
        before,
        after: wal_len(path),
        complete,
        elapsed: started.elapsed(),
    };
    log::info!(
        "{}: write-ahead log {} -> {} bytes in {} ms{}",
        path.display(),
        r.before,
        r.after,
        r.elapsed.as_millis(),
        if complete {
            ""
        } else {
            " (readers kept it busy)"
        }
    );
    r
}

/// The PASSIVE pass copies every committed page without taking the write lock (the writer and readers carry on);
/// the TRUNCATE tries then hold the write lock only for what was written meanwhile and the truncation.
fn shrink_on_own_connection(path: &Path) -> rusqlite::Result<bool> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT)?;
    conn.busy_timeout(SHRINK_WAIT)?;
    checkpoint(&conn, "PASSIVE")?;
    for attempt in 0..SHRINK_TRIES {
        if !truncate(&conn, SHRINK_WAIT)?.busy {
            return Ok(true);
        }
        if attempt + 1 < SHRINK_TRIES {
            std::thread::sleep(SHRINK_PAUSE);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn writer_connections_limit_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_writer(&dir.path().join("catalog.db")).unwrap();
        let limit: i64 = conn
            .pragma_query_value(None, "journal_size_limit", |r| r.get(0))
            .unwrap();
        assert_eq!(limit, WAL_SIZE_LIMIT);
    }

    #[test]
    fn truncate_empties_the_log_and_restores_the_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.db");
        let conn = crate::db::open_writer(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE junk(b BLOB);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 512)
             INSERT INTO junk SELECT randomblob(4096) FROM n;",
        )
        .unwrap();
        assert!(wal_len(&path) > 1 << 20, "the inserts are in the log");
        let r = truncate(&conn, Duration::from_millis(10)).unwrap();
        assert!(!r.busy, "{r:?}");
        assert_eq!(wal_len(&path), 0);
        let timeout: i64 = conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, BUSY_TIMEOUT.as_millis() as i64);
    }

    /// A log over the threshold left by a connection that closed without checkpointing.
    fn leftover_log(path: &Path) {
        let conn = crate::db::open_writer(path).unwrap();
        conn.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TABLE junk(b BLOB);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2600)
             INSERT INTO junk SELECT randomblob(4000) FROM n;",
        )
        .unwrap();
        drop(conn);
        assert!(wal_len(path) > SHRINK_AT_OPEN, "{}", wal_len(path));
    }

    #[test]
    fn a_shrink_waits_for_its_delay_and_drop_cancels_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.db");
        leftover_log(&path);
        let t = Instant::now();
        let task = ShrinkTask::start_if_needed(&path, false);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            wal_len(&path) > SHRINK_AT_OPEN,
            "not started before its delay"
        );
        drop(task);
        assert!(
            t.elapsed() < SHRINK_DELAY / 2,
            "drop did not wait for the delay"
        );
        assert!(
            wal_len(&path) > SHRINK_AT_OPEN,
            "a cancelled shrink copies nothing"
        );
        // Waiting starts it at once.
        let task = ShrinkTask::start_if_needed(&path, false);
        let r = task.wait().expect("started");
        assert!(r.complete && r.after == 0, "{r:?}");
        assert!(
            t.elapsed() < SHRINK_DELAY,
            "wait did not wait for the delay"
        );
    }

    #[test]
    fn wal_path_appends_the_suffix() {
        assert_eq!(
            wal_path(Path::new("/d/catalog.db")),
            PathBuf::from("/d/catalog.db-wal")
        );
    }
}
