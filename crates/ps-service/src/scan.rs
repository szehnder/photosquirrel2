//! Sources and scans (ADR-0027 facade over `ps-scan`; specs `sources`, `scanning`, `incremental-rescan`,
//! `scan-control`).
//!
//! [`ScanService`] owns the scan engine for one catalog. Every call is cheap and non-blocking except
//! [`ScanService::wait`], adding/re-linking a source (which walk and fingerprint the folder) and
//! [`ScanService::upgrade_hashes`]; the UI calls those from a background executor. Progress is pulled as
//! [`ProgressSnapshot`]s (at most every 100 ms, ADR-0019), never pushed per file.

use std::path::Path;

pub use ps_catalog::RunRecord;
pub use ps_model::{ProgressSnapshot, RunStatus, ScanRunId, SourceId, SourceStatus};
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
}

impl ScanService {
    /// Open the catalog in `data_dir`. Runs a crash left `running` become `interrupted`
    /// ([`ScanService::interrupted_runs`]) and can be resumed.
    pub fn open(data_dir: &Path, options: ScanOptions) -> Result<ScanService> {
        Ok(ScanService {
            engine: ps_scan::Scanner::open(data_dir, options)?,
        })
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
        self.engine.add_source(root)
    }

    pub fn sources(&self) -> Result<Vec<SourceInfo>> {
        self.engine.sources()
    }

    /// Re-check which sources are online.
    pub fn refresh_sources(&self) -> Result<Vec<SourceInfo>> {
        self.engine.refresh_sources()
    }

    pub fn relink_source(&self, id: SourceId, new_root: &Path) -> Result<SourceInfo> {
        self.engine.relink_source(id, new_root)
    }

    /// Remove a source's catalog records (requires [`Confirmation::Confirmed`]); never touches disk.
    pub fn remove_source(&self, id: SourceId, confirm: Confirmation) -> Result<()> {
        self.engine.remove_source(id, confirm)
    }

    // ---- scans ----

    /// Start scanning `sources` (all when empty).
    pub fn start(&self, sources: &[SourceId]) -> Result<ScanRunId> {
        self.engine.start_scan(sources)
    }

    pub fn pause(&self) -> Result<ScanRunId> {
        self.engine.pause()
    }

    /// Continue the paused active run.
    pub fn resume(&self) -> Result<ScanRunId> {
        self.engine.resume()
    }

    /// Resume a paused, cancelled or interrupted run (re-walks, processes unfinished files only).
    pub fn resume_run(&self, run: ScanRunId) -> Result<ScanRunId> {
        self.engine.resume_run(run)
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

    /// Block until the current run ends (call off the UI thread).
    pub fn wait(&self) -> Option<RunSummary> {
        self.engine.wait()
    }

    /// Every run, oldest first.
    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        self.engine.runs()
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
