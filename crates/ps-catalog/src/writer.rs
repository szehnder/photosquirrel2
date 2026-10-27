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
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, params};

use ps_model::{
    FileId, PerceptualHash, RelPath, RunStatus, ScanRunId, SourceId, SourceStatus, Stage,
};

use crate::error::{CatalogError, Result, WriteError, WriteErrors};
use crate::model::{
    AnalysisResult, FileFailure, FileKey, FileRow, MetaReread, NewSource, ProbeResult,
    Reclassification, Redate, Results, codes, now_ms,
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
    /// Capture dates chosen again by the current date rules from stored facts (ADR-0071): only the five date
    /// columns and `date_version` change. Rows without metadata are left alone.
    Redate(Vec<Redate>),
    /// Frame identities re-read from the header of unchanged RAW rows (ADR-0105): merged into the stored
    /// metadata with `frame_id` and `meta_version`; the dedup stamp changes, so the next regroup takes them up.
    /// Rows without metadata are left alone.
    MetaReread(Vec<MetaReread>),
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
    /// A re-decode once the missing decoder is available (ADR-0091): the perceptual hashes, and the
    /// `decoder_missing` marker cleared. Stage, class, metadata and dates are untouched; the new hashes change
    /// the file's dedup stamp, so the next regroup takes it up. Only rows still marked are written.
    Redecoded(Vec<(FileKey, PerceptualHash)>),
    /// Clear `decoder_missing` without hashes (the decoder rejected the data; the file keeps its exact-copy
    /// analysis and is not read again, ADR-0091).
    ClearDecoderMissing(Vec<FileKey>),
    /// A re-decode failed with the decoder present (ADR-0098): `decoder_missing` cleared and the failure recorded,
    /// so the next rescan retries it only while the failure is transient or after a decoder upgrade.
    DecodeFailed(Vec<(FileKey, ps_model::DecodeFailure)>),
    /// A re-decode's placeholder outcome (ADR-0109): `Some` stamps the new placeholder's generation and reason (it
    /// is not filled in again until the file or the generation changes), `None` clears them (a preview now). Only
    /// thumbed rows are written.
    SetPreview(Vec<(FileKey, Option<ps_model::PreviewStamp>)>),
    /// A one-time catalog task is done (`catalog_task`, migration 25, ADR-0111): its row is removed.
    FinishCatalogTask(String),
    /// The user's include/exclude decision about a file (`None` clears it). Needs catalog migration 7
    /// (`user_inclusion`, [`crate::db::GALLERY_MIGRATION`]); without it the command reports an error.
    ///
    /// `ai_kind` is set for an exclusion recorded by accepting an AI suggestion of that kind (`origin = 'ai'`,
    /// migration 21, ADR-0102); every other decision is the user's own.
    SetUserDecision {
        file: FileId,
        decision: Option<crate::UserDecision>,
        reason: Option<String>,
        ai_kind: Option<ps_model::AiKind>,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::UpsertFiles(_) => "upsert_files",
            Command::Probed(_) => "probed",
            Command::Reclassify(_) => "reclassify",
            Command::Redate(_) => "redate",
            Command::MetaReread(_) => "meta_reread",
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
            Command::Redecoded(_) => "redecoded",
            Command::ClearDecoderMissing(_) => "clear_decoder_missing",
            Command::SetPreview(_) => "set_preview",
            Command::FinishCatalogTask(_) => "finish_catalog_task",
            Command::DecodeFailed(_) => "decode_failed",
            Command::SetUserDecision { .. } => "set_user_decision",
        }
    }
}

/// A closure run on the writer connection (typed wrappers in [`crate::groups`], [`crate::stores`]).
type ExecFn = Box<dyn FnOnce(&Connection) -> rusqlite::Result<()> + Send>;

enum Msg {
    Cmd(Command),
    /// Run atomically (inside a savepoint) and commit; reply when committed. The flag: the write changes what
    /// decides cleanup safety ([`Writer::constraints_generation`]).
    Exec(ExecFn, Sender<Result<()>>, bool),
    /// [`Msg::Exec`] committed with `synchronous = FULL` (the cleanup journal, ADR-0064).
    ExecDurable(ExecFn, Sender<Result<()>>),
    Flush(Sender<WriteErrors>),
    AddSource(NewSource, Sender<Result<SourceId>>),
    StartRun(
        Vec<SourceId>,
        bool,
        Option<ps_model::FillScope>,
        Sender<Result<ScanRunId>>,
    ),
    Shutdown,
}

/// Counters for tests, progress and benchmarks.
#[derive(Debug, Default)]
struct Stats {
    commits: AtomicU64,
    rows_committed: AtomicU64,
    errors: AtomicU64,
    durable_commits: AtomicU64,
    /// Bumped after every commit that changed groups, keepers, separations, merges, user decisions, the export
    /// ledger or file rows ([`Writer::constraints_generation`], ADR-0099).
    constraints: AtomicU64,
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
                    constraints_dirty: false,
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
        self.exec_flagged(f, false)
    }

    /// [`Writer::exec`] for a write that changes what decides cleanup safety: the constraints generation is
    /// bumped once it is committed (ADR-0099).
    pub(crate) fn exec_constraints<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        self.exec_flagged(f, true)
    }

    fn exec_flagged<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        constraints: bool,
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
            constraints,
        ))?;
        rx.recv().map_err(|_| CatalogError::WriterClosed)??;
        out_rx.recv().map_err(|_| CatalogError::WriterClosed)
    }

    /// A counter bumped after every committed write that can change what makes a file safe to clean up: group
    /// and keeper edits, separations and merges, regroups, user include/exclude decisions, export-ledger rows
    /// and scan writes of file rows (every [`Command`]). Cleanup runs compare it before each move and re-derive
    /// the dispositions of their remaining files when it changed (ADR-0099). Not persisted: the catalog has one
    /// writer process (ADR-0005), so every such write goes through this handle.
    pub fn constraints_generation(&self) -> u64 {
        self.shared.stats.constraints.load(Ordering::SeqCst)
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
        self.start_run_with(sources, false)
    }

    /// [`Writer::start_run`], recording whether the run refreshes the analysis of unchanged files without
    /// stored class facts, so a resume refreshes too (ADR-0082).
    pub fn start_run_with(
        &self,
        sources: Vec<SourceId>,
        refresh_analysis: bool,
    ) -> Result<ScanRunId> {
        let (reply, rx) = bounded(1);
        self.send(Msg::StartRun(sources, refresh_analysis, None, reply))?;
        rx.recv().map_err(|_| CatalogError::WriterClosed)?
    }

    /// Record a new `running` Fill in previews job (`scan_run.kind = 'fill'`, ADR-0109) over `sources` with its
    /// scope, so a resume fills in the same scope.
    pub fn start_fill_run(
        &self,
        sources: Vec<SourceId>,
        scope: ps_model::FillScope,
    ) -> Result<ScanRunId> {
        let (reply, rx) = bounded(1);
        self.send(Msg::StartRun(sources, false, Some(scope), reply))?;
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

/// One write error as a [`CatalogError::Write`].
fn one_error(e: WriteError) -> CatalogError {
    CatalogError::Write(WriteErrors {
        errors: vec![e],
        total: 1,
    })
}

struct WriterThread {
    conn: Connection,
    config: WriterConfig,
    in_tx: bool,
    rows: usize,
    batch_start: Instant,
    errors: WriteErrors,
    stats: Arc<Stats>,
    /// The open batch holds a write that changes what decides cleanup safety.
    constraints_dirty: bool,
}

impl WriterThread {
    fn run(&mut self, rx: Receiver<Msg>) {
        loop {
            let msg = if self.in_tx {
                match rx.recv_deadline(self.batch_start + self.config.max_delay) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        self.commit_batch();
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
                Msg::Cmd(cmd) => {
                    self.constraints_dirty = true;
                    self.apply(cmd);
                }
                Msg::Flush(ack) => {
                    self.commit_batch();
                    let _ = ack.send(std::mem::take(&mut self.errors));
                }
                Msg::Exec(f, reply, constraints) => {
                    self.constraints_dirty |= constraints;
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
                Msg::StartRun(sources, refresh, fill, reply) => {
                    let r = self.immediate(|c| start_run(c, &sources, refresh, fill.as_ref()));
                    let _ = reply.send(r);
                }
                Msg::Shutdown => break,
            }
        }
        self.commit_batch();
        if let Err(e) = self
            .conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
        {
            log::warn!("catalog checkpoint at shutdown failed: {e}");
        }
    }

    /// Start a batch unless one is open. A failed `BEGIN` is recorded (for the next flush) and returned.
    fn begin(&mut self) -> std::result::Result<(), WriteError> {
        if self.in_tx {
            return Ok(());
        }
        match self.conn.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => {
                self.in_tx = true;
                self.rows = 0;
                self.batch_start = Instant::now();
                Ok(())
            }
            Err(e) => Err(self.error("begin", None, &e)),
        }
    }

    /// Commit the batch in progress (if any). A failed `COMMIT`, or a transaction SQLite already rolled back
    /// by itself (disk full, I/O error), means every row of the batch is lost: the error is recorded for the
    /// next flush and returned, so a caller waiting on this commit ([`WriterThread::exec`]) fails too.
    fn commit(&mut self) -> std::result::Result<(), WriteError> {
        if !self.in_tx {
            return Ok(());
        }
        self.in_tx = false;
        let rows = std::mem::take(&mut self.rows);
        let constraints = std::mem::take(&mut self.constraints_dirty);
        if self.conn.is_autocommit() {
            return Err(self.error_msg(
                "commit",
                format!("batch of {rows} rows was rolled back by SQLite"),
            ));
        }
        match self.conn.execute_batch("COMMIT") {
            Ok(()) => {
                self.stats.commits.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .rows_committed
                    .fetch_add(rows as u64, Ordering::Relaxed);
                if constraints {
                    // After the commit: a reader that sees the new value also sees the write.
                    self.stats.constraints.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
            Err(e) => {
                let err = self.error("commit", None, &e);
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    /// Commit the batch in progress, logging a failure (recorded for the next flush). For the batch timer,
    /// flushes and shutdown, whose callers learn of a lost batch from the flush's errors.
    fn commit_batch(&mut self) {
        // Already logged and recorded by `commit`.
        let _ = self.commit();
    }

    /// Run `f` inside the current batch and commit right away (callers wait for the result). Fails when the
    /// commit fails: the rows `f` wrote are not in the catalog.
    fn immediate<T>(&mut self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        self.begin().map_err(one_error)?;
        let r = f(&self.conn);
        self.rows += 1;
        let committed = self.commit();
        let v = r?;
        committed.map_err(one_error)?;
        Ok(v)
    }

    /// Run `f` atomically inside a savepoint of the current batch, then commit. `Ok` only once the commit
    /// succeeded; a failed commit (or `BEGIN`) is an error even when `f` succeeded.
    fn exec(&mut self, f: ExecFn) -> Result<()> {
        self.begin().map_err(one_error)?;
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
        let committed = self.commit();
        r?;
        committed.map_err(one_error)
    }

    /// [`WriterThread::exec`] in a transaction of its own, committed with `synchronous = FULL`. `Ok` only when
    /// that commit succeeded (the cleanup journal's intent is then on disk, ADR-0064).
    fn exec_durable(&mut self, f: ExecFn) -> Result<()> {
        // The batch in progress holds other producers' rows; a failure there is theirs (reported at their
        // flush), and `f` still gets a transaction of its own.
        self.commit_batch();
        if let Err(e) = self.conn.execute_batch("PRAGMA synchronous = FULL") {
            return Err(e.into());
        }
        let r = self.exec(f);
        if r.is_ok() {
            self.stats.durable_commits.fetch_add(1, Ordering::Relaxed);
        }
        if let Err(e) = self.conn.execute_batch("PRAGMA synchronous = NORMAL") {
            log::warn!("catalog: could not restore synchronous = NORMAL: {e}");
        }
        r
    }

    /// Record a write error for the next flush; returns it.
    fn error(
        &mut self,
        command: &'static str,
        file: Option<&FileKey>,
        e: &rusqlite::Error,
    ) -> WriteError {
        self.push_error(WriteError {
            command,
            file: file.cloned(),
            message: e.to_string(),
        })
    }

    fn error_msg(&mut self, command: &'static str, message: String) -> WriteError {
        self.push_error(WriteError {
            command,
            file: None,
            message,
        })
    }

    fn push_error(&mut self, e: WriteError) -> WriteError {
        self.stats.errors.fetch_add(1, Ordering::Relaxed);
        self.errors.push(e.clone());
        e
    }

    /// Apply one row inside the current batch; commit when the batch is full.
    fn row(
        &mut self,
        command: &'static str,
        file: Option<&FileKey>,
        f: impl FnOnce(&Connection) -> rusqlite::Result<()>,
    ) {
        // A failed BEGIN is recorded; the row then runs (and commits) on its own, as before.
        let _ = self.begin();
        // Every command row is a file-row or user-decision write (a batch may commit part-way through one).
        self.constraints_dirty = true;
        if let Err(e) = f(&self.conn) {
            self.error(command, file, &e);
        }
        self.rows += 1;
        if self.rows >= self.config.max_rows {
            self.commit_batch();
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
            Command::Redate(rows) => {
                for r in &rows {
                    self.row(name, Some(&r.key), |c| {
                        expect_one(set_redated(c, r)?, "unknown or unanalysed file")
                    });
                }
            }
            Command::MetaReread(rows) => {
                for r in &rows {
                    self.row(name, Some(&r.key), |c| {
                        expect_one(set_meta_reread(c, r)?, "unknown or unanalysed file")
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
                    self.commit_batch();
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
                ai_kind,
            } => self.row(name, None, |c| {
                crate::gallery::write_decision(c, file, decision, reason.as_deref(), ai_kind, now_ms())
            }),
            Command::Redecoded(rows) => {
                for (k, h) in &rows {
                    self.row(name, Some(k), |c| {
                        let n = c
                            .prepare_cached(
                                "UPDATE file SET phash = ?3, dhash = ?4, hash_version = ?5,
                                     decoder_missing = NULL, decode_failure = NULL,
                                     preview_gen = NULL, preview_reason = NULL
                                 WHERE source_id = ?1 AND rel_path = ?2
                                   AND (decoder_missing IS NOT NULL OR decode_failure IS NOT NULL
                                        OR preview_gen IS NOT NULL)",
                            )?
                            .execute(params![
                                k.source.0,
                                k.path.as_bytes(),
                                h.phash as i64,
                                h.dhash as i64,
                                i64::from(h.version)
                            ])?;
                        expect_one(n, "unknown file or not waiting for a decode")
                    });
                }
            }
            Command::DecodeFailed(rows) => {
                for (k, f) in &rows {
                    self.row(name, Some(k), |c| {
                        c.prepare_cached(
                            "UPDATE file SET decoder_missing = NULL, decode_failure = ?3
                             WHERE source_id = ?1 AND rel_path = ?2",
                        )?
                        .execute(params![k.source.0, k.path.as_bytes(), f.to_storage()])?;
                        Ok(())
                    });
                }
            }
            Command::ClearDecoderMissing(files) => {
                for k in &files {
                    self.row(name, Some(k), |c| {
                        c.prepare_cached(
                            "UPDATE file SET decoder_missing = NULL, decode_failure = NULL
                             WHERE source_id = ?1 AND rel_path = ?2",
                        )?
                        .execute(params![k.source.0, k.path.as_bytes()])?;
                        Ok(())
                    });
                }
            }
            Command::SetPreview(rows) => {
                for (k, p) in &rows {
                    self.row(name, Some(k), |c| {
                        c.prepare_cached(
                            "UPDATE file SET preview_gen = ?3, preview_reason = ?4
                             WHERE source_id = ?1 AND rel_path = ?2 AND stage = 3",
                        )?
                        .execute(params![
                            k.source.0,
                            k.path.as_bytes(),
                            p.map(|p| i64::from(p.generation)),
                            p.map(|p| p.reason.as_str())
                        ])?;
                        Ok(())
                    });
                }
            }
            Command::FinishCatalogTask(task) => self.row(name, None, |c| {
                c.prepare_cached("DELETE FROM catalog_task WHERE name = ?1")?
                    .execute([task.as_str()])?;
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
    ("capture_precision", "0"),
    ("camera_model", "NULL"),
    ("has_gps", "0"),
    ("metadata", "NULL"),
    ("decoder_missing", "NULL"),
    ("decode_failure", "NULL"),
    ("frame_id", "NULL"),
    ("meta_version", "0"),
    ("preview_gen", "NULL"),
    ("preview_reason", "NULL"),
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
    ("exclusion", "NULL"),
    ("attempts", "0"),
];

/// The classifier's exclusion category (`file.exclusion`, migration 21, ADR-0102 as amended by ADR-0104) as an SQL
/// expression over a class code, a format, a reasons JSON array and a file name (columns or parameters): the same
/// rule as [`ps_model::ExclusionCategory::of`] and migration 22's recomputation. Written in the statement that
/// writes the class.
pub(crate) fn exclusion_sql(class: &str, format: &str, reasons: &str, name: &str) -> String {
    let prefix = ps_model::PROBE_FAILED_PREFIX;
    let media: Vec<String> = ps_model::media_extensions()
        .iter()
        .map(|e| format!("'{e}'"))
        .collect();
    format!(
        "(CASE {class} WHEN 2 THEN 'cache' WHEN 3 THEN 'graphic' WHEN 4 THEN CASE
            WHEN {format} IS NOT NULL THEN CASE
                WHEN json_valid({reasons}) AND json_extract({reasons}, '$[0]') LIKE '{prefix}%' THEN 'unreadable'
                ELSE 'small' END
            WHEN EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid({reasons}) THEN {reasons} ELSE '[]' END) AS r
                         WHERE r.value LIKE '{prefix}%') THEN 'unreadable'
            WHEN {} IN ({}) THEN 'unreadable'
            ELSE 'unsupported' END END)",
        extension_sql(name),
        media.join(", ")
    )
}

/// A file name's extension in SQL, lower case (ASCII): the text after its last `.`, NULL without one; the same as
/// [`ps_model::extension_of`].
pub(crate) fn extension_sql(name: &str) -> String {
    let n = format!("CAST({name} AS TEXT)");
    format!(
        "(CASE WHEN instr({n}, '.') > 0 THEN lower(substr({n}, length(rtrim({n}, replace({n}, '.', ''))) + 1)) END)"
    )
}

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
                height = ?9, class_facts = ?10, classifier_hash = ?11, exclusion = {},
                kflags = (?4 | (coalesce(?7, 0) << 3) | (kflags & 16)), {}
         WHERE source_id = ?1 AND rel_path = ?2",
        exclusion_sql("?4", "?6", "?5", "name"),
        clear.join(", ")
    )
});

const ANALYZED_SQL: &str = "UPDATE file SET stage = 2,
        content_id = ?3, identity_kind = ?4, phash = ?5, dhash = ?6, hash_version = ?7,
        capture_ts = ?8, capture_offset_min = ?9, capture_provenance = ?10, capture_undated = ?11,
        camera_model = ?12, has_gps = ?13, metadata = ?14,
        width = coalesce(?15, width), height = coalesce(?16, height),
        capture_precision = ?17, date_version = ?18, decoder_missing = ?19, decode_failure = ?20,
        frame_id = ?21, meta_version = ?22, preview_gen = ?23, preview_reason = ?24
    WHERE source_id = ?1 AND rel_path = ?2 AND class IS NOT NULL";

/// All columns at once, for rows that arrive with results and are new (the common synthetic/import case).
static INSERT_FULL_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT INTO file (source_id, rel_path, name, size, mtime_ns, file_index,
        stage, class, reasons, format, media_kind, width, height,
        content_id, identity_kind, phash, dhash, hash_version, capture_ts, capture_offset_min,
        capture_provenance, capture_undated, camera_model, has_gps, metadata, kflags, first_seen,
        class_facts, classifier_hash, capture_precision, date_version, decoder_missing, decode_failure,
        frame_id, meta_version, preview_gen, preview_reason, exclusion)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
            ?21, ?22, ?23, ?24, ?25, ?8 | (coalesce(?11, 0) << 3), ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33,
            ?34, ?35, ?36, {})
    ON CONFLICT (source_id, rel_path) DO NOTHING",
        exclusion_sql("?8", "?10", "?9", "?3")
    )
});

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
    precision: i64,
    camera_model: Option<String>,
    has_gps: bool,
    metadata: String,
    width: Option<u32>,
    height: Option<u32>,
    frame_id: Option<String>,
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
        precision: i64::from(m.capture_date.map_or(0, |d| d.precision.code())),
        camera_model: m.fields.camera_model.clone(),
        has_gps: m.fields.gps.is_some(),
        metadata: serde_json::to_string(m)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        width: m.fields.width,
        height: m.fields.height,
        frame_id: m.fields.frame_id.as_ref().map(ps_model::FrameId::storage),
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
    c.prepare_cached(&INSERT_FULL_SQL)?.execute(params![
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
        x.precision,
        i64::from(ps_model::DATE_LOGIC_VERSION),
        a.decoder_missing.map(ps_model::DecoderKey::as_str),
        a.decode_failure
            .as_ref()
            .map(ps_model::DecodeFailure::to_storage),
        x.frame_id,
        i64::from(ps_model::META_LOGIC_VERSION),
        a.preview.map(|p| i64::from(p.generation)),
        a.preview.map(|p| p.reason.as_str()),
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

static RECLASSIFY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "UPDATE file SET class = ?3, reasons = ?4, classifier_hash = ?5, exclusion = {},
        stage = coalesce(?6, stage), kflags = (?3 | (kflags & 24)), class_facts = coalesce(?7, class_facts)
    WHERE source_id = ?1 AND rel_path = ?2 AND class IS NOT NULL",
        // A refresh re-probes (`?7`, the new facts): the category follows the new probe's format, not the
        // stored one; without new facts the stored format stands.
        exclusion_sql(
            "?3",
            "(CASE WHEN ?7 IS NULL THEN format ELSE ?8 END)",
            "?4",
            "name"
        )
    )
});

fn set_reclassified(c: &Connection, r: &Reclassification) -> rusqlite::Result<usize> {
    c.prepare_cached(&RECLASSIFY_SQL)?.execute(params![
        r.key.source.0,
        r.key.path.as_bytes(),
        r.class.code(),
        reasons_json(&r.reasons)?,
        r.classifier_hash as i64,
        r.stage.map(codes::stage),
        facts_json(r.facts.as_ref())?,
        r.facts
            .as_ref()
            .and_then(|f| f.format)
            .map(|f| f.storage_key()),
    ])
}

const REDATE_SQL: &str =
    "UPDATE file SET capture_ts = ?3, capture_offset_min = ?4, capture_provenance = ?5,
        capture_undated = ?6, capture_precision = ?7, date_version = ?8
    WHERE source_id = ?1 AND rel_path = ?2 AND metadata IS NOT NULL";

fn set_redated(c: &Connection, r: &Redate) -> rusqlite::Result<usize> {
    let d = r.capture.as_ref();
    c.prepare_cached(REDATE_SQL)?.execute(params![
        r.key.source.0,
        r.key.path.as_bytes(),
        d.map(|d| codes::capture_ms(&d.value)),
        d.and_then(|d| d.value.offset_minutes).map(i64::from),
        d.map(|d| d.provenance.as_str()),
        d.is_some_and(|d| d.undated),
        i64::from(d.map_or(0, |d| d.precision.code())),
        i64::from(r.version),
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
        x.precision,
        i64::from(ps_model::DATE_LOGIC_VERSION),
        a.decoder_missing.map(ps_model::DecoderKey::as_str),
        a.decode_failure
            .as_ref()
            .map(ps_model::DecodeFailure::to_storage),
        x.frame_id,
        i64::from(ps_model::META_LOGIC_VERSION),
        a.preview.map(|p| i64::from(p.generation)),
        a.preview.map(|p| p.reason.as_str()),
    ])
}

/// A header re-read for frame identity (ADR-0105): the frame identity, maker notes and the camera-original signal
/// merged into the stored metadata (or-ed: the full analysis may have seen more than the header shows), the
/// `frame_id` column and the version. Everything else the analysis stored stays. Only analysed rows are written.
fn set_meta_reread(c: &Connection, r: &MetaReread) -> rusqlite::Result<usize> {
    let stored: Option<String> = c
        .prepare_cached(
            "SELECT metadata FROM file
             WHERE source_id = ?1 AND rel_path = ?2 AND metadata IS NOT NULL",
        )?
        .query_row(params![r.key.source.0, r.key.path.as_bytes()], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(json) = stored else { return Ok(0) };
    let mut m: ps_model::Metadata = serde_json::from_str(&json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(e)))?;
    m.fields.set_frame_id(r.frame_id.clone());
    m.fields.has_maker_notes |= r.maker_notes;
    m.likely_camera_original |= r.camera_original;
    let json = serde_json::to_string(&m)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    c.prepare_cached(
        "UPDATE file SET metadata = ?3, frame_id = ?4, meta_version = ?5
         WHERE source_id = ?1 AND rel_path = ?2 AND metadata IS NOT NULL",
    )?
    .execute(params![
        r.key.source.0,
        r.key.path.as_bytes(),
        json,
        r.frame_id.as_ref().map(ps_model::FrameId::storage),
        i64::from(r.version),
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

fn start_run(
    c: &Connection,
    sources: &[SourceId],
    refresh: bool,
    fill: Option<&ps_model::FillScope>,
) -> rusqlite::Result<ScanRunId> {
    let ids: Vec<i64> = sources.iter().map(|s| s.0).collect();
    let json = serde_json::to_string(&ids)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let scope = fill
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    c.prepare_cached(
        "INSERT INTO scan_run (status, started_at, sources, refresh_analysis, kind, fill_scope)
         VALUES ('running', ?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        now_ms(),
        json,
        i64::from(refresh),
        if fill.is_some() { "fill" } else { "scan" },
        scope
    ])?;
    Ok(ScanRunId(c.last_insert_rowid()))
}
