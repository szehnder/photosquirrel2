//! Sources and scans (ADR-0027 facade over `ps-scan`; specs `sources`, `scanning`, `incremental-rescan`,
//! `scan-control`).
//!
//! [`ScanService`] owns the scan engine for one catalog. Every call is cheap and non-blocking except
//! [`ScanService::wait`], adding/re-linking a source (which walk and fingerprint the folder) and
//! [`ScanService::upgrade_hashes`]; the UI calls those from a background executor. Progress is pulled as
//! [`ProgressSnapshot`]s (at most every 100 ms, ADR-0019), never pushed per file.

use std::path::Path;
use std::sync::Arc;

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
    /// This machine's decoders (ADR-0089), detected off the caller's thread.
    decoders: Arc<crate::decoders::DecoderHub>,
}

impl ScanService {
    /// Open the catalog in `data_dir`. Runs a crash left `running` become `interrupted`
    /// ([`ScanService::interrupted_runs`]) and can be resumed.
    pub fn open(data_dir: &Path, options: ScanOptions) -> Result<ScanService> {
        Ok(ScanService {
            engine: ps_scan::Scanner::open(data_dir, options)?,
            ops: crate::ops::Ops::default(),
            decoders: crate::decoders::DecoderHub::global(),
        })
    }

    /// Use `decoders` instead of the detected ones (tests: the fake HEIC decoder, or none).
    pub fn with_decoders(mut self, decoders: ps_media::Decoders) -> ScanService {
        self.decoders = crate::decoders::DecoderHub::ready(decoders);
        self
    }

    /// Use the decoders of `hub` (detection shared with other services of the process).
    pub fn with_decoder_hub(mut self, hub: Arc<crate::decoders::DecoderHub>) -> ScanService {
        self.decoders = hub;
        self
    }

    /// This machine's decoders, waiting for detection at most 5 s (call off the UI thread).
    pub fn decoders(&self) -> Arc<ps_media::Decoders> {
        self.decoders.get()
    }

    /// The decoder detection, for callers that must not wait ([`crate::decoders::DecoderHub::try_get`]).
    pub fn decoder_hub(&self) -> &Arc<crate::decoders::DecoderHub> {
        &self.decoders
    }

    /// The decoder report with the poster-frame tool's entry (`doctor`, the scan summary, the inspector).
    pub fn decoder_report(&self, poster_tool: Option<&Path>) -> ps_model::DecoderReport {
        ps_media::decoder_report(&self.decoders(), poster_tool)
    }

    /// Files of `sources` (all when empty) still waiting for a decoder, per format (ADR-0091): one aggregate
    /// query, with whether the decoder is available now and the reason and fix when it is not.
    pub fn undecoded(&self, sources: &[SourceId]) -> Result<Vec<ps_model::Undecoded>> {
        let rows = self
            .engine
            .catalog()
            .read()
            .undecoded_counts(sources)
            .map_err(ScanError::from)?;
        Ok(ps_scan::undecoded_from(&rows, &self.decoders()))
    }

    /// Hand the detected decoders to the engine before a run starts (it waits for detection).
    fn prime_decoders(&self) {
        let d = self.decoders();
        self.engine.update_options(|o| o.decoders = d);
    }

    /// Open the catalog in `data_dir` read-only, as an observer that never takes the owner lock (so it never
    /// makes the app or a starting scan fail), unless the catalog must first be created or migrated. For
    /// read-only commands (status, reports, listing groups, export dry-runs); writes through an observer fail
    /// with `CatalogError::ReadOnly`.
    pub fn open_or_observe(data_dir: &Path, options: ScanOptions) -> Result<ScanService> {
        Ok(ScanService {
            engine: ps_scan::Scanner::open_or_observe(data_dir, options)?,
            ops: crate::ops::Ops::default(),
            decoders: crate::decoders::DecoderHub::global(),
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
        self.prime_decoders();
        self.unless_busy(|e| e.start_scan(sources))
    }

    /// Start scanning `sources` (all when empty) and refresh the analysis of their unchanged files analysed
    /// before classification facts were stored: their first 256 KB are re-read, read-only, and they are
    /// re-classified (ADR-0071). The scan summary's `needs_refresh` counts such files.
    pub fn start_refreshing_analysis(&self, sources: &[SourceId]) -> Result<ScanRunId> {
        self.prime_decoders();
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
        self.prime_decoders();
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

    // ---- previews (preview-placeholders, ADR-0109) ----

    /// The waiting rule as far as this process knows it now (never waits for decoder detection: while detection
    /// has not answered, no HEIC decoder counts as present). For the grid's status marks and the Library notice.
    pub fn waiting_rule(&self) -> ps_catalog::WaitingRule {
        let mut rule = self.engine.waiting_rule();
        if let Some(d) = self.decoders.try_get() {
            rule.decodable = ps_model::DecoderKey::ALL
                .into_iter()
                .filter(|k| d.can_decode(*k))
                .collect();
        }
        rule
    }

    /// Placeholders of `sources` (all when empty) by format and reason, with how many can be filled in now: one
    /// aggregate query (waits for decoder detection, so call off the UI thread).
    pub fn preview_status(&self, sources: &[SourceId]) -> Result<Vec<ps_model::PreviewStatusRow>> {
        self.prime_decoders();
        self.engine.preview_status(sources)
    }

    /// Start a Fill in previews job over `sources` (all when empty) for `scope` (ADR-0109): a run like a scan
    /// (progress, pause, resume, cancel) that reads only the waiting files, once each, and walks no folder.
    /// Refused while a scan, an export or a cleanup runs.
    pub fn fill_previews(
        &self,
        scope: ps_model::FillScope,
        sources: &[SourceId],
    ) -> Result<ScanRunId> {
        self.prime_decoders();
        self.unless_busy(|e| e.start_fill(scope, sources))
    }

    /// Recompute stale perceptual hashes from the thumbnail store (no source access).
    pub fn upgrade_hashes(&self) -> Result<HashUpgradeReport> {
        ps_scan::upgrade_hashes_current(self.engine.catalog(), self.engine.thumbs())
    }
}
