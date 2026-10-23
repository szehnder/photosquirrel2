//! Sources and scans (ADR-0027 facade over `ps-scan`; specs `sources`, `scanning`, `incremental-rescan`,
//! `scan-control`).
//!
//! [`ScanService`] owns the scan engine for one catalog. Every call is cheap and non-blocking except
//! [`ScanService::wait`], adding/re-linking a source (which walk and fingerprint the folder) and
//! [`ScanService::upgrade_hashes`]; the UI calls those from a background executor. Progress is pulled as
//! [`ProgressSnapshot`]s (at most every 100 ms, ADR-0019), never pushed per file.

use std::path::Path;

pub use ps_catalog::{CATALOG_FILE, RunRecord};
pub use ps_model::{ProgressSnapshot, RunStatus, ScanRunId, SourceId, SourceStatus, Stage};
pub use ps_scan::{
    AddedSource, Confirmation, HashUpgradeReport, RunSummary, ScanError, ScanOptions, SourceInfo,
};

/// Result type of the scan facade.
pub type Result<T, E = ScanError> = std::result::Result<T, E>;

/// A run with its stored summary (absent while the run has not ended once).
#[derive(Clone, Debug, PartialEq)]
pub struct RunReport {
    pub record: RunRecord,
    pub summary: Option<RunSummary>,
}

/// The scan engine of one catalog.
#[derive(Debug)]
pub struct ScanService {
    engine: ps_scan::Scanner,
    /// Exports and cleanup operations hold this; scans and source changes wait for it ([`crate::ops`]).
    ops: crate::ops::Ops,
}

impl ScanService {
    /// Open the catalog in `data_dir`. Runs a crash left `running` become `interrupted`
    /// ([`ScanService::interrupted_runs`]) and can be resumed.
    pub fn open(data_dir: &Path, options: ScanOptions) -> Result<ScanService> {
        Ok(ScanService {
            engine: ps_scan::Scanner::open(data_dir, options)?,
            ops: crate::ops::Ops::default(),
        })
    }

    /// Open the catalog in `data_dir` read-only, as an observer that never takes the owner lock (so it never
    /// makes the app or a starting scan fail), unless the catalog must first be created or migrated. For
    /// read-only commands (status, reports, listing groups, export dry-runs); writes through an observer fail
    /// with `CatalogError::ReadOnly`.
    pub fn open_or_observe(data_dir: &Path, options: ScanOptions) -> Result<ScanService> {
        Ok(ScanService {
            engine: ps_scan::Scanner::open_or_observe(data_dir, options)?,
            ops: crate::ops::Ops::default(),
        })
    }

    /// The catalog is open read-only (another process owns it).
    pub fn is_observer(&self) -> bool {
        self.engine.catalog().is_observer()
    }

    /// Open the catalog in the default data directory (`PHOTOSQUIRREL_DATA_DIR` or the per-user one).
    pub fn open_default(options: ScanOptions) -> Result<ScanService> {
        ScanService::open(&ps_catalog::data_dir()?, options)
    }

    /// The engine, for crates that need lower-level access (CLI diagnostics, tests).
    pub fn engine(&self) -> &ps_scan::Scanner {
        &self.engine
    }

    /// Runs found `running` at open (crash) and now `interrupted`, offered for resume.
    pub fn interrupted_runs(&self) -> &[ScanRunId] {
        self.engine.recovered_runs()
    }

    // ---- sources ----

    pub fn add_source(&self, root: &Path) -> Result<AddedSource> {
        self.unless_busy(|e| e.add_source(root))
    }

    pub fn sources(&self) -> Result<Vec<SourceInfo>> {
        self.engine.sources()
    }

    pub(crate) fn ops(&self) -> &crate::ops::Ops {
        &self.ops
    }

    /// Run `f` (a scan start or a source change) unless an export or a cleanup operation is running; the
    /// slot stays locked while `f` runs, so none can start meanwhile ([`crate::ops`]).
    fn unless_busy<T>(&self, f: impl FnOnce(&ps_scan::Scanner) -> Result<T>) -> Result<T> {
        let cur = self.ops.lock();
        if let Some(op) = *cur {
            return Err(ScanError::Busy(op.to_string()));
        }
        f(&self.engine)
    }

    /// Re-check which sources are online.
    pub fn refresh_sources(&self) -> Result<Vec<SourceInfo>> {
        self.engine.refresh_sources()
    }

    pub fn relink_source(&self, id: SourceId, new_root: &Path) -> Result<SourceInfo> {
        self.unless_busy(|e| e.relink_source(id, new_root))
    }

    /// Remove a source's catalog records (requires [`Confirmation::Confirmed`]); never touches disk. Refused
    /// while the source has files in its quarantine folder (undo its cleanup runs first, ADR-0063).
    pub fn remove_source(&self, id: SourceId, confirm: Confirmation) -> Result<()> {
        self.unless_busy(|e| e.remove_source(id, confirm))
    }

    // ---- scans ----

    /// Start scanning `sources` (all when empty).
    pub fn start(&self, sources: &[SourceId]) -> Result<ScanRunId> {
        self.unless_busy(|e| e.start_scan(sources))
    }

    /// Start scanning `sources` (all when empty) and refresh the analysis of their unchanged files analysed
    /// before classification facts were stored: their first 256 KB are re-read, read-only, and they are
    /// re-classified (ADR-0071). The scan summary's `needs_refresh` counts such files.
    pub fn start_refreshing_analysis(&self, sources: &[SourceId]) -> Result<ScanRunId> {
        self.unless_busy(|e| e.start_scan_with(sources, true))
    }

    pub fn pause(&self) -> Result<ScanRunId> {
        self.engine.pause()
    }

    /// Continue the paused active run.
    pub fn resume(&self) -> Result<ScanRunId> {
        self.unless_busy(|e| e.resume())
    }

    /// Resume a paused, cancelled or interrupted run (re-walks, processes unfinished files only).
    pub fn resume_run(&self, run: ScanRunId) -> Result<ScanRunId> {
        self.unless_busy(|e| e.resume_run(run))
    }

    pub fn cancel(&self) -> Result<ScanRunId> {
        self.engine.cancel()
    }

    /// The active run, if any.
    pub fn active(&self) -> Option<ScanRunId> {
        self.engine.active_run()
    }

    /// Progress of the active (or last) run.
    pub fn snapshot(&self) -> Option<ProgressSnapshot> {
        self.engine.snapshot()
    }

    /// Wait at most `limit` for the current run to stop (after [`ScanService::cancel`]); whether it has. Call
    /// off the UI thread.
    pub fn wait_stopped(&self, limit: std::time::Duration) -> bool {
        self.engine.wait_stopped(limit)
    }

    /// Block until the current run ends (call off the UI thread).
    pub fn wait(&self) -> Option<RunSummary> {
        self.engine.wait()
    }

    /// Every run, oldest first.
    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        self.engine.runs()
    }

    /// How many of `source`'s files are at each pipeline stage (the Sources view's progress).
    pub fn stage_counts(&self, source: SourceId) -> Result<Vec<(Stage, u64)>> {
        Ok(self.engine.catalog().read().stage_counts(source)?)
    }

    /// The most recent run left `interrupted` (crash or cancel before the end), if any.
    pub fn last_interrupted_run(&self) -> Result<Option<RunRecord>> {
        Ok(self
            .engine
            .runs()?
            .into_iter()
            .rev()
            .find(|r| r.status == RunStatus::Interrupted))
    }

    /// One run and its summary.
    pub fn report(&self, run: ScanRunId) -> Result<RunReport> {
        let record = self
            .engine
            .runs()?
            .into_iter()
            .find(|r| r.id == run)
            .ok_or(ScanError::NoSuchRun(run))?;
        Ok(RunReport {
            summary: RunSummary::from_json(&record.stats),
            record,
        })
    }

    /// Recompute stale perceptual hashes from the thumbnail store (no source access).
    pub fn upgrade_hashes(&self) -> Result<HashUpgradeReport> {
        ps_scan::upgrade_hashes_current(self.engine.catalog(), self.engine.thumbs())
    }
}
