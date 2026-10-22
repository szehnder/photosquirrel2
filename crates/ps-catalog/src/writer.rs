//! The single batching writer (ADR-0005).
//!
//! One thread owns the read-write connection. Producers on any thread send [`Command`]s through a bounded
//! channel (back-pressure); the thread applies them inside a transaction and commits when the batch reaches
//! [`WriterConfig::max_rows`] rows or [`WriterConfig::max_delay`] after the batch's first row, whichever comes
//! first. [`Writer::flush`] is a barrier: everything submitted before it (by any thread) is committed when it
//! returns, and it reports the write errors collected since the previous flush.
//!
//! Every stage update is a single statement that writes the stage together with that stage's results, and
//! a stage is derived from the results a row carries, so a crash at any instant leaves every file's stage
//! consistent with its stored results (ADR-0008).

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use ps_model::{
    FileId, PerceptualHash, RelPath, RunStatus, ScanRunId, SourceId, SourceStatus, Stage,
};

use crate::error::{CatalogError, Result, WriteError, WriteErrors};
use crate::model::{
    AnalysisResult, FileFailure, FileKey, FileRow, NewSource, ProbeResult, Reclassification,
    Results, codes, now_ms,
};

/// Batching parameters (ADR-0005 defaults).
#[derive(Clone, Copy, Debug)]
pub struct WriterConfig {
    /// Commit when a batch reaches this many rows.
    pub max_rows: usize,
    /// Commit this long after the batch's first row at the latest.
    pub max_delay: Duration,
    /// Commands buffered before producers block.
    pub channel_capacity: usize,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            max_rows: 2_000,
            max_delay: Duration::from_millis(100),
            channel_capacity: 256,
        }
    }
}

/// A typed catalog write. Each file entry counts as one batch row.
#[derive(Debug, Clone)]
pub enum Command {
    /// Walk results: insert new paths, update fingerprints. A changed fingerprint resets the file to
    /// `discovered` and clears its results in the same statement (ADR-0007); an unchanged one keeps
    /// everything. Rows carrying results also write them (stage derived from them).
    UpsertFiles(Vec<FileRow>),
    /// Probe results with stage `probed` (or `skipped`); clears any analysis results.
    Probed(Vec<(FileKey, ProbeResult)>),
    /// New classes of unchanged files after a classifier settings change, decided from their stored
    /// [`ps_model::ClassFacts`] without reading them (ADR-0051). Other results are kept.
    Reclassify(Vec<Reclassification>),
    /// Analysis results with stage `analyzed`. The file must have been probed.
    Analyzed(Vec<(FileKey, Box<AnalysisResult>)>),
    /// Stage `thumbed`: the thumbnail is in `thumbs.db`. The file must be analyzed.
    Thumbed(Vec<FileKey>),
    /// Stage `failed`, attempts + 1, and an error row.
    Failed(Vec<FileFailure>),
    /// The files are gone from their source (kept, never deleted; ADR-0007).
    MarkMissing { files: Vec<FileKey>, at_ms: i64 },
    SetSourceStatus {
        source: SourceId,
        status: SourceStatus,
    },
    /// Update a run's status (and summary). Terminal statuses set `ended_at`.
    SetRunStatus {
        run: ScanRunId,
        status: RunStatus,
        stats: Option<serde_json::Value>,
    },
    /// Start-up: runs left `running` by a crash become `interrupted` (ADR-0008).
    InterruptRunning,
    /// Re-link a source to a new root (ADR-0006): new root path and volume facts, status `online`.
    RelinkSource {
        source: SourceId,
        root: Vec<u8>,
        volume_uuid: Option<String>,
        device_hint: Option<String>,
    },
    /// Fold `inner` into `outer` (ADR-0006): the inner source's files move to `prefix/<their path>` in the
    /// outer source keeping their ids, stages and results, and the inner source is marked folded. Where the
    /// outer source already has a row at a folded path, the inner (older) row wins.
    FoldSource {
        inner: SourceId,
        outer: SourceId,
        prefix: RelPath,
    },
    /// Remove a source's catalog records: its files, their error rows, sources folded into it and the
    /// source row itself. Nothing on disk is touched (ADR-0004).
    RemoveSource { source: SourceId },
    /// A file moved within its source (ADR-0007): the row at `from` takes over the path `to`, replacing the
    /// row just discovered there, and takes that row's stat fingerprint. It keeps its id, first-seen time,
    /// stage and results, and is no longer missing.
    MoveFile { from: FileKey, to: FileKey },
    /// Perceptual hashes recomputed by a hash-version upgrade (ADR-0012).
    SetHashes(Vec<(FileId, PerceptualHash)>),
    /// The user's include/exclude decision about a file (`None` clears it). Needs catalog migration 7
    /// (`user_inclusion`, [`crate::db::GALLERY_MIGRATION`]); without it the command reports an error.
    SetUserDecision {
        file: FileId,
        decision: Option<crate::UserDecision>,
        reason: Option<String>,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::UpsertFiles(_) => "upsert_files",
            Command::Probed(_) => "probed",
            Command::Reclassify(_) => "reclassify",
            Command::Analyzed(_) => "analyzed",
            Command::Thumbed(_) => "thumbed",
            Command::Failed(_) => "failed",
            Command::MarkMissing { .. } => "mark_missing",
            Command::SetSourceStatus { .. } => "set_source_status",
            Command::SetRunStatus { .. } => "set_run_status",
            Command::InterruptRunning => "interrupt_running",
            Command::RelinkSource { .. } => "relink_source",
            Command::FoldSource { .. } => "fold_source",
            Command::RemoveSource { .. } => "remove_source",
            Command::MoveFile { .. } => "move_file",
            Command::SetHashes(_) => "set_hashes",
            Command::SetUserDecision { .. } => "set_user_decision",
        }
    }
}

/// A closure run on the writer connection (typed wrappers in [`crate::groups`], [`crate::stores`]).
type ExecFn = Box<dyn FnOnce(&Connection) -> rusqlite::Result<()> + Send>;

enum Msg {
    Cmd(Command),
    /// Run atomically (inside a savepoint) and commit; reply when committed.
    Exec(ExecFn, Sender<Result<()>>),
    /// [`Msg::Exec`] committed with `synchronous = FULL` (the cleanup journal, ADR-0064).
    ExecDurable(ExecFn, Sender<Result<()>>),
    Flush(Sender<WriteErrors>),
    AddSource(NewSource, Sender<Result<SourceId>>),
    StartRun(Vec<SourceId>, Sender<Result<ScanRunId>>),
    Shutdown,
}

/// Counters for tests, progress and benchmarks.
#[derive(Debug, Default)]
struct Stats {
    commits: AtomicU64,
    rows_committed: AtomicU64,
    errors: AtomicU64,
    durable_commits: AtomicU64,
}

/// A snapshot of the writer's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriterStats {
    /// Committed transactions.
    pub commits: u64,
    /// Rows in committed transactions.
    pub rows_committed: u64,
    /// Write errors so far.
    pub errors: u64,
    /// Transactions committed with `synchronous = FULL` ([`Writer::exec_durable`]).
    pub durable_commits: u64,
}

struct Shared {
    stats: Arc<Stats>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// An observer's writer: another process owns the catalog, every write fails with
    /// [`CatalogError::ReadOnly`].
    read_only: bool,
}

impl Drop for Shared {
    // The last handle is gone: its sender was dropped first (field order), so the thread sees the channel
    // disconnect, commits what it has and exits.
    fn drop(&mut self) {
        if let Some(t) = self.thread.get_mut().take() {
            let _ = t.join();
        }
    }
}

/// Handle to the writer thread. Cheap to clone and usable from any number of producer threads.
#[derive(Clone)]
pub struct Writer {
    // Declared before `shared`: dropped first, so the thread is told to stop before it is joined.
    tx: Sender<Msg>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer")
            .field("stats", &self.stats())
            .finish()
    }
}

impl Writer {
    /// Start the writer thread on an already migrated connection.
    pub(crate) fn spawn(conn: Connection, config: WriterConfig) -> Result<Writer> {
        let (tx, rx) = bounded(config.channel_capacity.max(1));
        let stats = Arc::new(Stats::default());
        let thread_stats = stats.clone();
        let thread = std::thread::Builder::new()
            .name("catalog-writer".into())
            .spawn(move || {
                let mut w = WriterThread {
                    conn,
                    config,
                    in_tx: false,
                    rows: 0,
                    batch_start: Instant::now(),
                    errors: WriteErrors::default(),
                    stats: thread_stats,
                };
                w.run(rx);
            })
            .map_err(|e| CatalogError::Io {
                path: "catalog-writer thread".into(),
                source: e,
            })?;
        Ok(Writer {
            tx,
            shared: Arc::new(Shared {
                stats,
                thread: Mutex::new(Some(thread)),
                read_only: false,
            }),
        })
    }

    /// A writer that refuses every write (observer catalogs, ADR-0005 single writer).
    pub(crate) fn read_only() -> Writer {
        let (tx, _rx) = bounded(1);
        Writer {
            tx,
            shared: Arc::new(Shared {
                stats: Arc::new(Stats::default()),
                thread: Mutex::new(None),
                read_only: true,
            }),
        }
    }

    /// This writer refuses writes (the catalog is open as an observer).
    pub fn is_read_only(&self) -> bool {
        self.shared.read_only
    }

    fn send(&self, m: Msg) -> Result<()> {
        if self.shared.read_only {
            return Err(CatalogError::ReadOnly);
        }
        self.tx.send(m).map_err(|_| CatalogError::WriterClosed)
    }

    /// Queue a command (blocks while the queue is full).
    pub fn submit(&self, cmd: Command) -> Result<()> {
        self.send(Msg::Cmd(cmd))
    }

    pub fn upsert_files(&self, rows: Vec<FileRow>) -> Result<()> {
        self.submit(Command::UpsertFiles(rows))
    }

    pub fn probed(&self, rows: Vec<(FileKey, ProbeResult)>) -> Result<()> {
        self.submit(Command::Probed(rows))
    }

    pub fn analyzed(&self, rows: Vec<(FileKey, Box<AnalysisResult>)>) -> Result<()> {
        self.submit(Command::Analyzed(rows))
    }

    pub fn thumbed(&self, files: Vec<FileKey>) -> Result<()> {
        self.submit(Command::Thumbed(files))
    }

    pub fn failed(&self, rows: Vec<FileFailure>) -> Result<()> {
        self.submit(Command::Failed(rows))
    }

    /// Barrier: commit everything submitted before this call (from any thread). Returns the write errors
    /// collected since the previous flush (by any producer) as [`CatalogError::Write`].
    pub fn flush(&self) -> Result<()> {
        let (ack, rx) = bounded(1);
        self.send(Msg::Flush(ack))?;
        let errors = rx.recv().map_err(|_| CatalogError::WriterClosed)?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CatalogError::Write(errors))
        }
    }

    /// Run `f` on the writer connection atomically (a savepoint inside the current batch), commit, and return
    /// its result. For the typed write helpers of the other catalog modules.
    pub(crate) fn exec<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, rx) = bounded(1);
        let (out_tx, out_rx) = bounded(1);
        self.send(Msg::Exec(
            Box::new(move |c| {
                let v = f(c)?;
                let _ = out_tx.send(v);
                Ok(())
            }),
            reply,
        ))?;
        rx.recv().map_err(|_| CatalogError::WriterClosed)??;
        out_rx.recv().map_err(|_| CatalogError::WriterClosed)
    }

    /// [`Writer::exec`] whose commit is durable when it returns: the batch in progress is committed first,
    /// then `f` runs in its own transaction committed with `synchronous = FULL` (in WAL mode `NORMAL` may
    /// lose the last commits on power loss). For the catalog half of the cleanup journal (ADR-0064), whose
    /// intent must be on disk before the rename it describes.
    pub(crate) fn exec_durable<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, rx) = bounded(1);
        let (out_tx, out_rx) = bounded(1);
        self.send(Msg::ExecDurable(
            Box::new(move |c| {
                let v = f(c)?;
                let _ = out_tx.send(v);
                Ok(())
            }),
            reply,
        ))?;
        rx.recv().map_err(|_| CatalogError::WriterClosed)??;
        out_rx.recv().map_err(|_| CatalogError::WriterClosed)
    }

    /// Add a source and return its id once committed.
    pub fn add_source(&self, source: NewSource) -> Result<SourceId> {
        let (reply, rx) = bounded(1);
        self.send(Msg::AddSource(source, reply))?;
        rx.recv().map_err(|_| CatalogError::WriterClosed)?
    }

    /// Record a new `running` scan run over `sources` and return its id once committed.
    pub fn start_run(&self, sources: Vec<SourceId>) -> Result<ScanRunId> {
        let (reply, rx) = bounded(1);
        self.send(Msg::StartRun(sources, reply))?;
        rx.recv().map_err(|_| CatalogError::WriterClosed)?
    }

    pub fn stats(&self) -> WriterStats {
        let s = &self.shared.stats;
        WriterStats {
            commits: s.commits.load(Ordering::Relaxed),
            rows_committed: s.rows_committed.load(Ordering::Relaxed),
            errors: s.errors.load(Ordering::Relaxed),
            durable_commits: s.durable_commits.load(Ordering::Relaxed),
        }
    }

    /// Commit, stop the thread and wait for it. Other clones get [`CatalogError::WriterClosed`] afterwards.
    pub(crate) fn shutdown(&self) -> Result<()> {
        if self.shared.read_only {
            return Ok(());
        }
        let flushed = self.flush();
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.shared.thread.lock().take() {
            let _ = t.join();
        }
        match flushed {
            Err(CatalogError::WriterClosed) => Ok(()),
            other => other,
        }
    }
}

struct WriterThread {
    conn: Connection,
    config: WriterConfig,
    in_tx: bool,
    rows: usize,
    batch_start: Instant,
    errors: WriteErrors,
    stats: Arc<Stats>,
}

impl WriterThread {
    fn run(&mut self, rx: Receiver<Msg>) {
        loop {
            let msg = if self.in_tx {
                match rx.recv_deadline(self.batch_start + self.config.max_delay) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        self.commit();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                }
            };
            match msg {
                Msg::Cmd(cmd) => self.apply(cmd),
                Msg::Flush(ack) => {
                    self.commit();
                    let _ = ack.send(std::mem::take(&mut self.errors));
                }
                Msg::Exec(f, reply) => {
                    let r = self.exec(f);
                    let _ = reply.send(r);
                }
                Msg::ExecDurable(f, reply) => {
                    let r = self.exec_durable(f);
                    let _ = reply.send(r);
                }
                Msg::AddSource(s, reply) => {
                    let r = self.immediate(|c| add_source(c, &s));
                    let _ = reply.send(r);
                }
                Msg::StartRun(sources, reply) => {
                    let r = self.immediate(|c| start_run(c, &sources));
                    let _ = reply.send(r);
                }
                Msg::Shutdown => break,
            }
        }
        self.commit();
        if let Err(e) = self
            .conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
        {
            log::warn!("catalog checkpoint at shutdown failed: {e}");
        }
    }

    fn begin(&mut self) {
        if self.in_tx {
            return;
        }
        match self.conn.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => {
                self.in_tx = true;
                self.rows = 0;
                self.batch_start = Instant::now();
            }
            Err(e) => self.error("begin", None, &e),
        }
    }

    fn commit(&mut self) {
        if !self.in_tx {
            return;
        }
        self.in_tx = false;
        if self.conn.is_autocommit() {
            // SQLite rolled the transaction back by itself (disk full, I/O error): the batch is lost.
            self.error_msg(
                "commit",
                format!("batch of {} rows was rolled back by SQLite", self.rows),
            );
            return;
        }
        match self.conn.execute_batch("COMMIT") {
            Ok(()) => {
                self.stats.commits.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .rows_committed
                    .fetch_add(self.rows as u64, Ordering::Relaxed);
            }
            Err(e) => {
                self.error("commit", None, &e);
                let _ = self.conn.execute_batch("ROLLBACK");
            }
        }
        self.rows = 0;
    }

    /// Run `f` inside the current batch and commit right away (callers wait for the result).
    fn immediate<T>(&mut self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        self.begin();
        let r = f(&self.conn);
        self.rows += 1;
        self.commit();
        Ok(r?)
    }

    /// Run `f` atomically inside a savepoint of the current batch, then commit.
    fn exec(&mut self, f: ExecFn) -> Result<()> {
        self.begin();
        let r = (|| -> rusqlite::Result<()> {
            self.conn.execute_batch("SAVEPOINT exec")?;
            match f(&self.conn) {
                Ok(()) => self.conn.execute_batch("RELEASE exec"),
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK TO exec; RELEASE exec");
                    Err(e)
                }
            }
        })();
        self.rows += 1;
        self.commit();
        Ok(r?)
    }

    /// [`WriterThread::exec`] in a transaction of its own, committed with `synchronous = FULL`.
    fn exec_durable(&mut self, f: ExecFn) -> Result<()> {
        self.commit();
        if let Err(e) = self.conn.execute_batch("PRAGMA synchronous = FULL") {
            return Err(e.into());
        }
        let commits = self.stats.commits.load(Ordering::Relaxed);
        let r = self.exec(f);
        if self.stats.commits.load(Ordering::Relaxed) > commits {
            self.stats.durable_commits.fetch_add(1, Ordering::Relaxed);
        }
        if let Err(e) = self.conn.execute_batch("PRAGMA synchronous = NORMAL") {
            log::warn!("catalog: could not restore synchronous = NORMAL: {e}");
        }
        r
    }

    fn error(&mut self, command: &'static str, file: Option<&FileKey>, e: &rusqlite::Error) {
        self.stats.errors.fetch_add(1, Ordering::Relaxed);
        self.errors.push(WriteError {
            command,
            file: file.cloned(),
            message: e.to_string(),
        });
    }

    fn error_msg(&mut self, command: &'static str, message: String) {
        self.stats.errors.fetch_add(1, Ordering::Relaxed);
        self.errors.push(WriteError {
            command,
            file: None,
            message,
        });
    }

    /// Apply one row inside the current batch; commit when the batch is full.
    fn row(
        &mut self,
        command: &'static str,
        file: Option<&FileKey>,
        f: impl FnOnce(&Connection) -> rusqlite::Result<()>,
    ) {
        self.begin();
        if let Err(e) = f(&self.conn) {
            self.error(command, file, &e);
        }
        self.rows += 1;
        if self.rows >= self.config.max_rows {
            self.commit();
        }
    }

    fn apply(&mut self, cmd: Command) {
        let name = cmd.name();
        match cmd {
            Command::UpsertFiles(rows) => {
                for r in &rows {
                    self.row(name, Some(&r.key), |c| upsert_file(c, r));
                }
            }
            Command::Probed(rows) => {
                for (k, p) in &rows {
                    self.row(name, Some(k), |c| expect_one(set_probed(c, k, p)?, "unknown file"));
                }
            }
            Command::Reclassify(rows) => {
                for r in &rows {
                    self.row(name, Some(&r.key), |c| {
                        expect_one(set_reclassified(c, r)?, "unknown or unprobed file")
                    });
                }
            }
            Command::Analyzed(rows) => {
                for (k, a) in &rows {
                    self.row(name, Some(k), |c| {
                        expect_one(set_analyzed(c, k, a)?, "unknown or unprobed file")
                    });
                }
            }
            Command::Thumbed(files) => {
                for k in &files {
                    self.row(name, Some(k), |c| {
                        let n = c
                            .prepare_cached(
                                "UPDATE file SET stage = 3
                                 WHERE source_id = ?1 AND rel_path = ?2 AND stage IN (2, 3)",
                            )?
                            .execute(params![k.source.0, k.path.as_bytes()])?;
                        expect_one(n, "unknown or unanalyzed file")
                    });
                }
            }
            Command::Failed(rows) => {
                let at = now_ms();
                for f in &rows {
                    self.row(name, Some(&f.key), |c| set_failed(c, f, at));
                }
            }
            Command::MarkMissing { files, at_ms } => {
                for k in &files {
                    self.row(name, Some(k), |c| {
                        c.prepare_cached(
                            "UPDATE file SET missing_since = ?3, kflags = kflags | 16
                             WHERE source_id = ?1 AND rel_path = ?2 AND missing_since IS NULL",
                        )?
                        .execute(params![k.source.0, k.path.as_bytes(), at_ms])?;
                        Ok(())
                    });
                }
            }
            Command::SetSourceStatus { source, status } => self.row(name, None, |c| {
                c.prepare_cached("UPDATE source SET status = ?2 WHERE id = ?1")?
                    .execute(params![source.0, status.as_str()])?;
                Ok(())
            }),
            Command::SetRunStatus { run, status, stats } => {
                let terminal = matches!(
                    status,
                    RunStatus::Cancelled | RunStatus::Completed | RunStatus::Interrupted
                );
                let stats = stats.map(|s| s.to_string());
                self.row(name, None, |c| {
                    c.prepare_cached(
                        "UPDATE scan_run SET status = ?2, stats = coalesce(?3, stats),
                                ended_at = CASE WHEN ?4 THEN ?5 WHEN ?2 = 'running' THEN NULL
                                                ELSE ended_at END
                         WHERE id = ?1",
                    )?
                    .execute(params![run.0, status.as_str(), stats, terminal, now_ms()])?;
                    Ok(())
                });
                if terminal {
                    // Keep the WAL from growing across runs (design: passive checkpoint after each run).
                    self.commit();
                    let _ = self
                        .conn
                        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
                }
            }
            Command::InterruptRunning => self.row(name, None, |c| {
                c.execute(
                    "UPDATE scan_run SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'",
                    [now_ms()],
                )?;
                Ok(())
            }),
            Command::RelinkSource {
                source,
                root,
                volume_uuid,
                device_hint,
            } => self.row(name, None, |c| {
                let n = c
                    .prepare_cached(
                        "UPDATE source SET root_path = ?2, volume_uuid = coalesce(?3, volume_uuid),
                                device_hint = coalesce(?4, device_hint), status = 'online'
                         WHERE id = ?1",
                    )?
                    .execute(params![source.0, root, volume_uuid, device_hint])?;
                expect_one(n, "unknown source")
            }),
            Command::FoldSource {
                inner,
                outer,
                prefix,
            } => self.row(name, None, |c| fold_source(c, inner, outer, &prefix)),
            Command::RemoveSource { source } => {
                self.row(name, None, |c| remove_source(c, source))
            }
            Command::MoveFile { from, to } => {
                self.row(name, Some(&to), |c| move_file(c, &from, &to))
            }
            Command::SetUserDecision {
                file,
                decision,
                reason,
            } => self.row(name, None, |c| {
                match decision {
                    Some(d) => {
                        c.prepare_cached(
                            "INSERT INTO user_inclusion (file_id, decision, reason, at)
                             VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT (file_id) DO UPDATE SET
                                decision = excluded.decision, reason = excluded.reason, at = excluded.at",
                        )?
                        .execute(params![file.0, d.as_str(), reason, now_ms()])?;
                    }
                    None => {
                        c.prepare_cached("DELETE FROM user_inclusion WHERE file_id = ?1")?
                            .execute([file.0])?;
                    }
                }
                Ok(())
            }),
            Command::SetHashes(rows) => {
                for (id, h) in &rows {
                    self.row(name, None, |c| {
                        let n = c
                            .prepare_cached(
                                "UPDATE file SET phash = ?2, dhash = ?3, hash_version = ?4 WHERE id = ?1",
                            )?
                            .execute(params![
                                id.0,
                                h.phash as i64,
                                h.dhash as i64,
                                i64::from(h.version)
                            ])?;
                        expect_one(n, "unknown file")
                    });
                }
            }
        }
    }
}

fn expect_one(n: usize, what: &str) -> rusqlite::Result<()> {
    if n == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::ModuleError(what.to_owned()))
    }
}

/// Result columns cleared when a file's fingerprint changes or it is re-probed.
const ANALYSIS_COLUMNS: &[(&str, &str)] = &[
    ("content_id", "NULL"),
    ("identity_kind", "NULL"),
    ("phash", "NULL"),
    ("dhash", "NULL"),
    ("hash_version", "NULL"),
    ("capture_ts", "NULL"),
    ("capture_offset_min", "NULL"),
    ("capture_provenance", "NULL"),
    ("capture_undated", "0"),
    ("camera_model", "NULL"),
    ("has_gps", "0"),
    ("metadata", "NULL"),
];
const PROBE_COLUMNS: &[(&str, &str)] = &[
    ("class", "NULL"),
    ("reasons", "NULL"),
    ("format", "NULL"),
    ("media_kind", "NULL"),
    ("width", "NULL"),
    ("height", "NULL"),
    ("class_facts", "NULL"),
    ("classifier_hash", "NULL"),
    ("attempts", "0"),
];

static UPSERT_SQL: LazyLock<String> = LazyLock::new(|| {
    let changed = "(size != excluded.size OR mtime_ns != excluded.mtime_ns
        OR (file_index IS NOT NULL AND excluded.file_index IS NOT NULL AND file_index != excluded.file_index))";
    let mut sets = vec![format!("stage = CASE WHEN {changed} THEN 0 ELSE stage END")];
    for (col, empty) in PROBE_COLUMNS.iter().chain(ANALYSIS_COLUMNS) {
        sets.push(format!(
            "{col} = CASE WHEN {changed} THEN {empty} ELSE {col} END"
        ));
    }
    // Plain assignments last: every right-hand side above still sees the old row.
    sets.push("size = excluded.size".into());
    sets.push("mtime_ns = excluded.mtime_ns".into());
    sets.push("file_index = excluded.file_index".into());
    sets.push(format!(
        "kflags = CASE WHEN {changed} THEN 7 ELSE (kflags & 15) END"
    ));
    sets.push("missing_since = NULL".into());
    format!(
        "INSERT INTO file (source_id, rel_path, name, size, mtime_ns, file_index, first_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (source_id, rel_path) DO UPDATE SET {}
         WHERE {changed} OR missing_since IS NOT NULL OR file_index IS NOT excluded.file_index",
        sets.join(",\n")
    )
});

static PROBED_SQL: LazyLock<String> = LazyLock::new(|| {
    let clear: Vec<String> = ANALYSIS_COLUMNS
        .iter()
        .map(|(c, e)| format!("{c} = {e}"))
        .collect();
    format!(
        "UPDATE file SET stage = ?3, class = ?4, reasons = ?5, format = ?6, media_kind = ?7, width = ?8,
                height = ?9, class_facts = ?10, classifier_hash = ?11,
                kflags = (?4 | (coalesce(?7, 0) << 3) | (kflags & 16)), {}
         WHERE source_id = ?1 AND rel_path = ?2",
        clear.join(", ")
    )
});

const ANALYZED_SQL: &str = "UPDATE file SET stage = 2,
        content_id = ?3, identity_kind = ?4, phash = ?5, dhash = ?6, hash_version = ?7,
        capture_ts = ?8, capture_offset_min = ?9, capture_provenance = ?10, capture_undated = ?11,
        camera_model = ?12, has_gps = ?13, metadata = ?14,
        width = coalesce(?15, width), height = coalesce(?16, height)
    WHERE source_id = ?1 AND rel_path = ?2 AND class IS NOT NULL";

/// All columns at once, for rows that arrive with results and are new (the common synthetic/import case).
const INSERT_FULL_SQL: &str = "INSERT INTO file (source_id, rel_path, name, size, mtime_ns, file_index,
        stage, class, reasons, format, media_kind, width, height,
        content_id, identity_kind, phash, dhash, hash_version, capture_ts, capture_offset_min,
        capture_provenance, capture_undated, camera_model, has_gps, metadata, kflags, first_seen,
        class_facts, classifier_hash)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
            ?21, ?22, ?23, ?24, ?25, ?8 | (coalesce(?11, 0) << 3), ?26, ?27, ?28)
    ON CONFLICT (source_id, rel_path) DO NOTHING";

fn upsert_file(c: &Connection, r: &FileRow) -> rusqlite::Result<()> {
    let file_index = r.stat.file_index.map(|i| i as i64);
    let base = params![
        r.key.source.0,
        r.key.path.as_bytes(),
        r.key.path.file_name(),
        r.stat.size as i64,
        r.stat.mtime_ns,
        file_index,
        now_ms()
    ];
    match &r.results {
        Results::None => {
            c.prepare_cached(&UPSERT_SQL)?.execute(base)?;
        }
        Results::Probed(p) => {
            c.prepare_cached(&UPSERT_SQL)?.execute(base)?;
            set_probed(c, &r.key, p)?;
        }
        Results::Analyzed(p, a) => {
            let inserted = insert_full(c, r, p, a, file_index)?;
            if inserted == 0 {
                c.prepare_cached(&UPSERT_SQL)?.execute(base)?;
                set_probed(c, &r.key, p)?;
                set_analyzed(c, &r.key, a)?;
            }
        }
    }
    Ok(())
}

struct AnalysisCols {
    content_id: [u8; 32],
    identity_kind: i64,
    phash: Option<i64>,
    dhash: Option<i64>,
    hash_version: Option<i64>,
    capture_ts: Option<i64>,
    capture_offset: Option<i64>,
    provenance: Option<&'static str>,
    undated: bool,
    camera_model: Option<String>,
    has_gps: bool,
    metadata: String,
    width: Option<u32>,
    height: Option<u32>,
}

fn analysis_cols(a: &AnalysisResult) -> rusqlite::Result<AnalysisCols> {
    let capture = a.capture();
    let m = &a.metadata;
    Ok(AnalysisCols {
        content_id: a.identity.digest,
        identity_kind: codes::identity_kind(a.identity.kind),
        phash: a.hashes.map(|h| h.phash as i64),
        dhash: a.hashes.map(|h| h.dhash as i64),
        hash_version: a.hashes.map(|h| i64::from(h.version)),
        capture_ts: capture.as_ref().map(codes::capture_ms),
        capture_offset: capture.and_then(|t| t.offset_minutes).map(i64::from),
        provenance: m.capture_date.map(|d| d.provenance.as_str()),
        undated: m.capture_date.is_some_and(|d| d.undated),
        camera_model: m.fields.camera_model.clone(),
        has_gps: m.fields.gps.is_some(),
        metadata: serde_json::to_string(m)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        width: m.fields.width,
        height: m.fields.height,
    })
}

fn insert_full(
    c: &Connection,
    r: &FileRow,
    p: &ProbeResult,
    a: &AnalysisResult,
    file_index: Option<i64>,
) -> rusqlite::Result<usize> {
    let x = analysis_cols(a)?;
    let reasons = reasons_json(&p.reasons)?;
    c.prepare_cached(INSERT_FULL_SQL)?.execute(params![
        r.key.source.0,
        r.key.path.as_bytes(),
        r.key.path.file_name(),
        r.stat.size as i64,
        r.stat.mtime_ns,
        file_index,
        codes::stage(Stage::Analyzed),
        p.class.code(),
        reasons,
        p.format.map(|f| f.storage_key()),
        p.media_kind().map(codes::kind),
        x.width.or(p.width),
        x.height.or(p.height),
        &x.content_id[..],
        x.identity_kind,
        x.phash,
        x.dhash,
        x.hash_version,
        x.capture_ts,
        x.capture_offset,
        x.provenance,
        x.undated,
        x.camera_model,
        x.has_gps,
        x.metadata,
        now_ms(),
        facts_json(p.facts.as_ref())?,
        p.classifier_hash.map(|h| h as i64),
    ])
}

fn reasons_json(r: &[String]) -> rusqlite::Result<String> {
    serde_json::to_string(r).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn facts_json(f: Option<&ps_model::ClassFacts>) -> rusqlite::Result<Option<String>> {
    f.map(|f| {
        serde_json::to_string(f).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })
    .transpose()
}

const RECLASSIFY_SQL: &str = "UPDATE file SET class = ?3, reasons = ?4, classifier_hash = ?5,
        stage = coalesce(?6, stage), kflags = (?3 | (kflags & 24))
    WHERE source_id = ?1 AND rel_path = ?2 AND class IS NOT NULL";

fn set_reclassified(c: &Connection, r: &Reclassification) -> rusqlite::Result<usize> {
    c.prepare_cached(RECLASSIFY_SQL)?.execute(params![
        r.key.source.0,
        r.key.path.as_bytes(),
        r.class.code(),
        reasons_json(&r.reasons)?,
        r.classifier_hash as i64,
        r.stage.map(codes::stage),
    ])
}

fn set_probed(c: &Connection, k: &FileKey, p: &ProbeResult) -> rusqlite::Result<usize> {
    c.prepare_cached(&PROBED_SQL)?.execute(params![
        k.source.0,
        k.path.as_bytes(),
        codes::stage(p.stage()),
        p.class.code(),
        reasons_json(&p.reasons)?,
        p.format.map(|f| f.storage_key()),
        p.media_kind().map(codes::kind),
        p.width,
        p.height,
        facts_json(p.facts.as_ref())?,
        p.classifier_hash.map(|h| h as i64),
    ])
}

fn set_analyzed(c: &Connection, k: &FileKey, a: &AnalysisResult) -> rusqlite::Result<usize> {
    let x = analysis_cols(a)?;
    c.prepare_cached(ANALYZED_SQL)?.execute(params![
        k.source.0,
        k.path.as_bytes(),
        &x.content_id[..],
        x.identity_kind,
        x.phash,
        x.dhash,
        x.hash_version,
        x.capture_ts,
        x.capture_offset,
        x.provenance,
        x.undated,
        x.camera_model,
        x.has_gps,
        x.metadata,
        x.width,
        x.height,
    ])
}

fn set_failed(c: &Connection, f: &FileFailure, at: i64) -> rusqlite::Result<()> {
    let n = c
        .prepare_cached(
            "UPDATE file SET stage = 5, attempts = attempts + 1 WHERE source_id = ?1 AND rel_path = ?2",
        )?
        .execute(params![f.key.source.0, f.key.path.as_bytes()])?;
    expect_one(n, "unknown file")?;
    c.prepare_cached(
        "INSERT INTO file_error (file_id, run_id, stage, message, at)
         SELECT id, ?3, ?4, ?5, ?6 FROM file WHERE source_id = ?1 AND rel_path = ?2",
    )?
    .execute(params![
        f.key.source.0,
        f.key.path.as_bytes(),
        f.run.map(|r| r.0),
        codes::stage(f.stage),
        f.message,
        at
    ])?;
    Ok(())
}

fn fold_source(
    c: &Connection,
    inner: SourceId,
    outer: SourceId,
    prefix: &RelPath,
) -> rusqlite::Result<()> {
    if inner == outer {
        return Err(rusqlite::Error::ModuleError(
            "cannot fold a source into itself".into(),
        ));
    }
    let rows: Vec<(i64, Vec<u8>)> = {
        let mut stmt = c.prepare("SELECT id, rel_path FROM file WHERE source_id = ?1")?;
        stmt.query_map([inner.0], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, rel) in rows {
        let rel = RelPath::from_bytes(rel)
            .ok_or_else(|| rusqlite::Error::ModuleError("invalid stored rel_path".into()))?;
        let new_path = prefix.join(&rel);
        // A row the outer source already has at this path is replaced by the inner one (it has history).
        if let Some(existing) = c
            .prepare_cached("SELECT id FROM file WHERE source_id = ?1 AND rel_path = ?2")?
            .query_row(params![outer.0, new_path.as_bytes()], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            delete_file_row(c, existing)?;
        }
        c.prepare_cached("UPDATE file SET source_id = ?2, rel_path = ?3 WHERE id = ?1")?
            .execute(params![id, outer.0, new_path.as_bytes()])?;
    }
    let n = c.execute(
        "UPDATE source SET folded_into = ?2 WHERE id = ?1",
        params![inner.0, outer.0],
    )?;
    expect_one(n, "unknown inner source")?;
    // Sources previously folded into the inner one now belong to the outer one.
    c.execute(
        "UPDATE source SET folded_into = ?2 WHERE folded_into = ?1",
        params![inner.0, outer.0],
    )?;
    Ok(())
}

fn delete_file_row(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.prepare_cached("DELETE FROM file_error WHERE file_id = ?1")?
        .execute([id])?;
    c.prepare_cached("DELETE FROM file WHERE id = ?1")?
        .execute([id])?;
    Ok(())
}

fn remove_source(c: &Connection, source: SourceId) -> rusqlite::Result<()> {
    c.execute(
        "DELETE FROM file_error WHERE file_id IN (SELECT id FROM file WHERE source_id = ?1)",
        [source.0],
    )?;
    c.execute("DELETE FROM file WHERE source_id = ?1", [source.0])?;
    c.execute("DELETE FROM source WHERE folded_into = ?1", [source.0])?;
    let n = c.execute("DELETE FROM source WHERE id = ?1", [source.0])?;
    expect_one(n, "unknown source")
}

fn move_file(c: &Connection, from: &FileKey, to: &FileKey) -> rusqlite::Result<()> {
    if from.source != to.source {
        return Err(rusqlite::Error::ModuleError(
            "a file can only move within its source".into(),
        ));
    }
    let lookup = |k: &FileKey| -> rusqlite::Result<Option<(i64, i64, i64, Option<i64>)>> {
        c.prepare_cached(
            "SELECT id, size, mtime_ns, file_index FROM file WHERE source_id = ?1 AND rel_path = ?2",
        )?
        .query_row(params![k.source.0, k.path.as_bytes()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()
    };
    let old = lookup(from)?.ok_or_else(|| rusqlite::Error::ModuleError("unknown file".into()))?;
    let new = lookup(to)?;
    let (size, mtime, index) = match new {
        Some((new_id, size, mtime, index)) => {
            delete_file_row(c, new_id)?;
            (size, mtime, index)
        }
        None => (old.1, old.2, old.3),
    };
    c.prepare_cached(
        "UPDATE file SET rel_path = ?2, name = ?3, size = ?4, mtime_ns = ?5, file_index = ?6,
                missing_since = NULL, kflags = kflags & 15
         WHERE id = ?1",
    )?
    .execute(params![
        old.0,
        to.path.as_bytes(),
        to.path.file_name(),
        size,
        mtime,
        index
    ])?;
    Ok(())
}

fn add_source(c: &Connection, s: &NewSource) -> rusqlite::Result<SourceId> {
    // `source.id` is not AUTOINCREMENT, so SQLite would hand a removed source's id to the next one. An id a
    // cleanup run refers to is never handed out again (ADR-0063): its runs keep pointing at their own source.
    c.prepare_cached(
        "INSERT INTO source (id, root_path, volume_uuid, device_hint, total_bytes, fingerprint, added_at)
         VALUES (1 + max((SELECT coalesce(max(id), 0) FROM source),
                         (SELECT coalesce(max(source_id), 0) FROM cleanup_run)),
                 ?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![
        s.root,
        s.volume_uuid,
        s.device_hint,
        s.total_bytes.map(|b| b as i64),
        s.fingerprint,
        now_ms()
    ])?;
    Ok(SourceId(c.last_insert_rowid()))
}

fn start_run(c: &Connection, sources: &[SourceId]) -> rusqlite::Result<ScanRunId> {
    let ids: Vec<i64> = sources.iter().map(|s| s.0).collect();
    let json = serde_json::to_string(&ids)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    c.prepare_cached(
        "INSERT INTO scan_run (status, started_at, sources) VALUES ('running', ?1, ?2)",
    )?
    .execute(params![now_ms(), json])?;
    Ok(ScanRunId(c.last_insert_rowid()))
}
