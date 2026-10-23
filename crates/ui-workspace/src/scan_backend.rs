//! What the Sources, scan dashboard, Export and Reports views need from the engine (changes `scan-dashboard`
//! and `export`), behind one trait so the views run on the real `ps-service` facade ([`CatalogBackend`]),
//! the simulated engine ([`DemoBackend`], tests and screenshots) or a test fake.
//!
//! Every method blocks; views call them from background tasks, never on the UI thread (ADR-0005). The live
//! scan is pulled as [`ProgressSnapshot`]s at most every 100 ms (ADR-0019).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ps_model::{
    CompletionCounts, DeviceClass, Disposition, ExclusionReason, ExportProfile, FileId,
    ProgressSnapshot, RelPath, RunStatus, ScanRunId, SourceId, SourceStatus, Stage,
};
use ps_service::export_progress::{ExecConfig, ExportProgress, ExportSnapshot, ProfileSettings};
use ps_service::scan::Confirmation;
use ps_service::scans::{FakeScanService, RunError, RunOverview};

use crate::library_backend::CatalogBackend;

pub use ps_service::library::{disposition_reason, reason_label};

/// One source in the Sources list.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    pub id: SourceId,
    /// The folder's name ("Pictures", "Family").
    pub name: String,
    pub root: PathBuf,
    pub online: bool,
    pub device: DeviceClass,
    /// End of the last run that covered this source.
    pub last_scan_ms: Option<i64>,
    /// Files catalogued (present) and of those, finished (thumbed or skipped).
    pub files: u64,
    pub finished: u64,
}

impl SourceRow {
    /// Percent of the source's catalogued files that need no more scan work.
    pub fn completion_percent(&self) -> Option<u32> {
        (self.files > 0).then(|| ((self.finished.min(self.files) * 100) / self.files) as u32)
    }
}

/// The dry run as the Export view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanPreview {
    pub library: PathBuf,
    pub profile: ExportProfile,
    pub sidecars: bool,
    pub files: u64,
    pub bytes: u64,
    pub adopted: u64,
    pub collisions: u64,
    pub sidecar_files: u64,
    pub unavailable: u64,
    /// Offline sources holding unavailable keepers, with their counts.
    pub unavailable_sources: Vec<(String, u64)>,
    pub already_exported: u64,
    /// Library copies changed by another app since the export: reported, not copied again (ADR-0046).
    pub modified: u64,
    pub free_bytes: Option<u64>,
    pub required_bytes: u64,
    /// Why the export cannot start (insufficient space), if it cannot.
    pub blocker: Option<String>,
    /// Planned copies (source, file name, library path) to show where things go, at most
    /// [`PLAN_ROWS_LIMIT`]; `files` + `adopted` says how many there are in all.
    pub examples: Vec<PlanRow>,
    /// Files not dated by their own metadata (ADR-0043): borrowed, file-name, sidecar and undated counts.
    pub date_note: Option<String>,
    /// Files on the sources still pending against this library (the Reports view's "pending"), which the
    /// planned keepers account for together with their duplicates. `None` when not known for this library.
    pub pending_files: Option<u64>,
}

/// One planned file as the Export view lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRow {
    /// The source's name ("Laptop").
    pub source: String,
    pub name: String,
    /// Library-relative target.
    pub target: String,
}

impl PlanRow {
    pub fn new(source: &str, name: &str, target: &str) -> Self {
        Self {
            source: source.into(),
            name: name.into(),
            target: target.into(),
        }
    }
}

/// Planned files listed by "Where files go" (the rest is counted).
pub const PLAN_ROWS_LIMIT: usize = 500;

/// How an export run ended.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportOutcome {
    pub library: PathBuf,
    pub copied: u64,
    pub adopted: u64,
    pub bytes_copied: u64,
    pub sidecars: u64,
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
    pub stopped: Option<String>,
    pub elapsed_ms: u64,
}

/// One folder row of the completion report.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderRow {
    /// `None` = files directly in the source root.
    pub folder: Option<String>,
    pub counts: CompletionCounts,
    pub excluded: Vec<(ExclusionReason, u64, u64)>,
}

/// One source of the completion report.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceReportRow {
    pub id: SourceId,
    pub name: String,
    pub counts: CompletionCounts,
    pub excluded: Vec<(ExclusionReason, u64, u64)>,
    pub folders: Vec<FolderRow>,
}

/// The completion report for one library.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportData {
    pub library: PathBuf,
    pub totals: CompletionCounts,
    pub sources: Vec<SourceReportRow>,
    pub mismatches: u64,
    /// Library copies that are missing or changed (path in the library, what is wrong), at most 200.
    pub mismatch_rows: Vec<(String, String)>,
    /// Library copies were re-hashed (verify), not only checked for existence and size.
    pub verified: bool,
    /// When the copies were last re-hashed; `None`: never.
    pub verified_at_ms: Option<i64>,
}

/// One file behind a disposition count (Reports drill-down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrillRow {
    pub file: i64,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub disposition: Disposition,
    /// Why (exclusion reason, "not exported yet", the library copy ...).
    pub reason: String,
}

/// Which files to list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DrillQuery {
    pub source: SourceId,
    /// Exactly this folder (direct files), `None` = the whole source.
    pub folder: Option<Option<String>>,
    pub disposition: Disposition,
}

/// The engine side of the Sources, dashboard, Export and Reports views.
pub trait ScanBackend: Send + Sync + 'static {
    fn sources(&self) -> anyhow::Result<Vec<SourceRow>>;
    /// Register a folder (walks and fingerprints it; does not start a scan).
    fn add_source(&self, root: &Path) -> anyhow::Result<SourceId>;
    fn relink_source(&self, id: SourceId, new_root: &Path) -> anyhow::Result<()>;
    /// Forget a source's catalog records. Never touches its files (ADR-0004).
    fn remove_source(&self, id: SourceId) -> anyhow::Result<()>;

    fn start(&self, sources: &[SourceId]) -> anyhow::Result<ScanRunId>;
    fn pause(&self) -> anyhow::Result<()>;
    fn resume(&self) -> anyhow::Result<()>;
    fn cancel(&self) -> anyhow::Result<()>;
    /// After [`ScanBackend::cancel`]: wait at most `limit` for the run to stop, so the next snapshot reads
    /// cancelled; whether it has. Blocks: call it on a background task. The default does not wait.
    fn wait_stopped(&self, limit: std::time::Duration) -> bool {
        let _ = limit;
        false
    }
    /// Continue a cancelled, paused or interrupted run.
    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()>;
    /// The active (or last) run's progress. Cheap; sampled at most every 100 ms.
    fn snapshot(&self) -> Option<ProgressSnapshot>;
    /// Why the active run is paused for power (`general.pause_on_battery`), for the dashboard.
    fn power_note(&self) -> Option<String> {
        None
    }
    /// The summary of a run that has ended.
    fn overview(&self, run: ScanRunId) -> anyhow::Result<Option<RunOverview>>;
    /// The last run that ended (for the dashboard's summary when idle).
    fn last_run(&self) -> anyhow::Result<Option<RunOverview>>;
    /// Every stored error of a run (newest first).
    fn run_errors(&self, run: ScanRunId, limit: usize) -> anyhow::Result<Vec<RunError>>;

    /// The library folder used last.
    fn last_library(&self) -> Option<PathBuf>;
    /// The dry run for `settings` (profile, sidecars on or off for this export, re-copy of changed copies).
    fn export_plan(&self, library: &Path, settings: ProfileSettings)
    -> anyhow::Result<PlanPreview>;
    /// Run an export to the end (or until `cancel`), reporting into `progress`.
    fn export_run(
        &self,
        library: &Path,
        settings: ProfileSettings,
        cancel: &AtomicBool,
        progress: &ExportProgress,
    ) -> anyhow::Result<ExportOutcome>;
    /// The completion report for `library` (the last one used when `None`). `verify` re-hashes every library
    /// copy (slow); otherwise copies are checked for existence and size.
    fn report(&self, library: Option<&Path>, verify: bool) -> anyhow::Result<Option<ReportData>>;
    fn drill(
        &self,
        library: &Path,
        query: &DrillQuery,
        limit: usize,
    ) -> anyhow::Result<Vec<DrillRow>>;

    // ---- cleanup (change `quarantine-cleanup`; ADR-0024, ADR-0061) ----

    /// Every cleanup run, newest first.
    fn cleanup_runs(&self) -> anyhow::Result<Vec<CleanupRunRow>> {
        Ok(Vec::new())
    }
    /// One run with what it moves (or moved) per folder.
    fn cleanup_review(&self, run: i64) -> anyhow::Result<CleanupReview> {
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }
    /// Plan a cleanup of a whole source and store it; `None` when nothing there is exported or covered.
    /// Moves nothing.
    fn cleanup_plan(&self, source: SourceId) -> anyhow::Result<Option<CleanupReview>> {
        anyhow::bail!("source {source}: cleanup is not available here")
    }
    /// Run a confirmed plan (each file re-verified, then renamed into the quarantine folder).
    fn cleanup_run(
        &self,
        run: i64,
        network_confirmed: bool,
        cancel: &AtomicBool,
        progress: &CleanupProgress,
    ) -> anyhow::Result<CleanupOutcome> {
        let _ = (network_confirmed, cancel, progress);
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }
    /// Move a run's quarantined files back, those `selection` picks (never onto an occupied path).
    fn cleanup_undo(&self, run: i64, selection: CleanupSelection) -> anyhow::Result<UndoOutcome> {
        let _ = selection;
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }

    // ---- cleanup-selection (ADR-0068) ----

    /// Every folder of a run with its files' ids, sizes and whether they are picked: the planned and
    /// unselected moves of a plan ([`CleanupTreeKind::Plan`]), or the quarantined ones ([`CleanupTreeKind::Undo`],
    /// all picked). No file names: those load per folder ([`ScanBackend::cleanup_folder_files`]).
    fn cleanup_tree(&self, run: i64, kind: CleanupTreeKind) -> anyhow::Result<CleanupTree> {
        let _ = kind;
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }
    /// The files directly in one folder of [`ScanBackend::cleanup_tree`], with their names.
    fn cleanup_folder_files(
        &self,
        run: i64,
        folder: Option<RelPath>,
        kind: CleanupTreeKind,
    ) -> anyhow::Result<Vec<CleanupFileRow>> {
        let _ = (folder, kind);
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }
    /// Store a plan's selection (catalog rows only; moves nothing) and return the stored plan.
    fn cleanup_select(
        &self,
        run: i64,
        selection: CleanupSelection,
    ) -> anyhow::Result<CleanupReview> {
        let _ = selection;
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }
    /// Discard a plan that never moved a file (catalog rows only).
    fn cleanup_discard(&self, run: i64) -> anyhow::Result<CleanupRunRow> {
        anyhow::bail!("cleanup run {run}: cleanup is not available here")
    }
}

/// Which moves of a run a selection picks ([`ps_service::cleanup::Selection`]).
pub use ps_service::cleanup::Selection as CleanupSelection;

/// What a selection tree is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CleanupTreeKind {
    /// A plan's planned and unselected moves (narrowing before the run).
    Plan,
    /// A run's quarantined moves (undo).
    Undo,
}

/// One file of a [`CleanupTree`] folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanupTreeFile {
    pub file: FileId,
    pub size: u64,
    pub picked: bool,
}

/// The files directly in one folder (`None`: the source root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupTreeFolder {
    pub folder: Option<RelPath>,
    pub files: Vec<CleanupTreeFile>,
}

/// Every folder of a run that holds a move of the tree's kind, by path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupTree {
    pub run: i64,
    pub kind: CleanupTreeKind,
    pub folders: Vec<CleanupTreeFolder>,
}

/// A file of an expanded folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupFileRow {
    pub file: FileId,
    pub name: String,
    pub size: u64,
    pub picked: bool,
}

/// The moves a tree of `kind` is over, and whether each is picked now.
fn tree_pick(kind: CleanupTreeKind, state: ps_model::MoveState) -> Option<bool> {
    use ps_model::MoveState as S;
    match (kind, state) {
        (CleanupTreeKind::Plan, S::Planned) => Some(true),
        (CleanupTreeKind::Plan, S::Unselected) => Some(false),
        (CleanupTreeKind::Undo, S::Quarantined) => Some(true),
        _ => None,
    }
}

/// [`CleanupTree`] of `moves` (any order).
pub fn cleanup_tree_of(
    run: i64,
    kind: CleanupTreeKind,
    moves: &[ps_model::CleanupMove],
) -> CleanupTree {
    let mut by: std::collections::BTreeMap<Option<RelPath>, Vec<CleanupTreeFile>> =
        std::collections::BTreeMap::new();
    for m in moves {
        if let Some(picked) = tree_pick(kind, m.state) {
            by.entry(m.rel_path.parent())
                .or_default()
                .push(CleanupTreeFile {
                    file: m.file,
                    size: m.size,
                    picked,
                });
        }
    }
    CleanupTree {
        run,
        kind,
        folders: by
            .into_iter()
            .map(|(folder, files)| CleanupTreeFolder { folder, files })
            .collect(),
    }
}

pub use ps_model::CleanupRunState;
/// Live counters of a cleanup run (sampled by the Cleanup panel, ADR-0019).
pub use ps_service::cleanup::RunProgress as CleanupProgress;

/// One cleanup run in the history.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanupRunRow {
    pub id: i64,
    pub source: SourceId,
    pub source_name: String,
    pub state: CleanupRunState,
    pub created_at_ms: i64,
    pub planned: u64,
    pub planned_bytes: u64,
    pub quarantined: u64,
    pub quarantined_bytes: u64,
    pub skipped: u64,
    pub restored: u64,
    /// In the plan, left out by the selection (ADR-0068).
    pub unselected: u64,
    pub unselected_bytes: u64,
    /// `<source root>/.photosquirrel-quarantine/<run id>`.
    pub quarantine_dir: PathBuf,
    /// The source is a network share (running needs an extra confirmation).
    pub network: bool,
}

/// Files listed by a cleanup review (the rest is counted).
pub const CLEANUP_ROWS_LIMIT: usize = 200;

/// A run as the Cleanup panel reviews it.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanupReview {
    pub run: CleanupRunRow,
    pub root: PathBuf,
    pub library: Option<PathBuf>,
    /// Planned files per folder: (folder, files, bytes), `(source root)` for files at the root.
    pub folders: Vec<(String, u64, u64)>,
    /// Planned moves (original path, quarantine path, both relative to the source root), at most
    /// [`CLEANUP_ROWS_LIMIT`].
    pub moves: Vec<(String, String)>,
    /// Files a run did not move or an undo left in quarantine, with why, at most [`CLEANUP_ROWS_LIMIT`].
    pub notes: Vec<(String, String)>,
}

/// How a cleanup run ended.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanupOutcome {
    pub run: i64,
    pub moved: u64,
    pub bytes_moved: u64,
    /// (path, why) of files not moved.
    pub skipped: Vec<(String, String)>,
    pub cancelled: bool,
    pub not_attempted: u64,
    pub elapsed_ms: u64,
}

/// How an undo ended.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoOutcome {
    pub run: i64,
    pub restored: u64,
    pub bytes_restored: u64,
    /// (path, why) of files left in quarantine.
    pub conflicts: Vec<(String, String)>,
    pub missing: u64,
    pub changed: u64,
    pub fully_undone: bool,
}

fn cleanup_row(s: &ps_service::cleanup::RunSummary) -> CleanupRunRow {
    CleanupRunRow {
        id: s.run.id,
        source: s.run.source,
        source_name: source_name(&s.root),
        state: s.run.state,
        created_at_ms: s.run.created_at_ms,
        planned: s.counts.planned,
        planned_bytes: s.counts.planned_bytes,
        quarantined: s.counts.quarantined,
        quarantined_bytes: s.counts.quarantined_bytes,
        skipped: s.counts.skipped,
        restored: s.counts.restored,
        unselected: s.counts.unselected,
        unselected_bytes: s.counts.unselected_bytes,
        quarantine_dir: s.quarantine_dir.clone(),
        network: s.network,
    }
}

fn cleanup_review_of(
    summary: &ps_service::cleanup::RunSummary,
    moves: &[ps_model::CleanupMove],
) -> CleanupReview {
    let planned: Vec<&ps_model::CleanupMove> = moves
        .iter()
        .filter(|m| m.state == ps_model::MoveState::Planned)
        .collect();
    let totals = ps_service::cleanup::totals_of(planned.iter().copied());
    CleanupReview {
        run: cleanup_row(summary),
        root: summary.root.clone(),
        library: summary.library.clone(),
        folders: totals
            .folders
            .iter()
            .map(|f| {
                (
                    f.folder
                        .as_ref()
                        .map_or_else(|| "(source root)".to_owned(), |p| p.display().into_owned()),
                    f.files,
                    f.bytes,
                )
            })
            .collect(),
        moves: planned
            .iter()
            .take(CLEANUP_ROWS_LIMIT)
            .map(|m| {
                (
                    m.rel_path.display().into_owned(),
                    m.quarantine_rel_path.display().into_owned(),
                )
            })
            .collect(),
        notes: moves
            .iter()
            .filter(|m| m.note.is_some() && m.state != ps_model::MoveState::Restored)
            .take(CLEANUP_ROWS_LIMIT)
            .map(|m| {
                (
                    m.rel_path.display().into_owned(),
                    m.note.clone().unwrap_or_default(),
                )
            })
            .collect(),
    }
}

pub type SharedScanBackend = Arc<dyn ScanBackend>;

/// Display name of a source root.
pub fn source_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

fn excluded_rows(
    b: &std::collections::BTreeMap<ExclusionReason, (u64, u64)>,
) -> Vec<(ExclusionReason, u64, u64)> {
    b.iter().map(|(r, (n, bytes))| (*r, *n, *bytes)).collect()
}

// ---- the real engine --------------------------------------------------------------------------------------

impl ScanBackend for CatalogBackend {
    fn sources(&self) -> anyhow::Result<Vec<SourceRow>> {
        let scan = self.scan()?;
        let infos = scan.refresh_sources()?;
        let runs = scan.runs()?;
        let mut out = Vec::with_capacity(infos.len());
        for s in infos {
            let counts = scan.stage_counts(s.id)?;
            let files: u64 = counts.iter().map(|(_, n)| n).sum();
            let finished: u64 = counts
                .iter()
                .filter(|(st, _)| matches!(st, Stage::Thumbed | Stage::Skipped))
                .map(|(_, n)| n)
                .sum();
            let last_scan_ms = runs
                .iter()
                .filter(|r| r.sources.contains(&s.id))
                .filter_map(|r| r.ended_at_ms)
                .max();
            out.push(SourceRow {
                id: s.id,
                name: source_name(&s.root),
                online: s.status == SourceStatus::Online && s.root.is_dir(),
                root: s.root,
                device: s.device,
                last_scan_ms,
                files,
                finished,
            });
        }
        Ok(out)
    }

    fn add_source(&self, root: &Path) -> anyhow::Result<SourceId> {
        let added = self.scan()?.add_source(root)?;
        self.forget_sources();
        Ok(added.source.id)
    }

    fn relink_source(&self, id: SourceId, new_root: &Path) -> anyhow::Result<()> {
        self.scan()?.relink_source(id, new_root)?;
        self.forget_sources();
        Ok(())
    }

    fn remove_source(&self, id: SourceId) -> anyhow::Result<()> {
        self.scan()?.remove_source(id, Confirmation::Confirmed)?;
        self.forget_sources();
        Ok(())
    }

    fn start(&self, sources: &[SourceId]) -> anyhow::Result<ScanRunId> {
        Ok(self.scan()?.start(sources)?)
    }

    fn pause(&self) -> anyhow::Result<()> {
        self.scan()?.pause()?;
        Ok(())
    }

    fn resume(&self) -> anyhow::Result<()> {
        self.scan()?.resume()?;
        Ok(())
    }

    fn cancel(&self) -> anyhow::Result<()> {
        self.scan()?.cancel()?;
        Ok(())
    }

    fn wait_stopped(&self, limit: std::time::Duration) -> bool {
        self.scan().is_ok_and(|s| s.wait_stopped(limit))
    }

    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()> {
        self.scan()?.resume_run(run)?;
        Ok(())
    }

    fn snapshot(&self) -> Option<ProgressSnapshot> {
        self.scan().ok()?.snapshot()
    }

    fn power_note(&self) -> Option<String> {
        self.power_pause().map(|p| p.message().to_owned())
    }

    fn overview(&self, run: ScanRunId) -> anyhow::Result<Option<RunOverview>> {
        let o = self.scan()?.run_overview(run)?;
        Ok((!matches!(o.status, RunStatus::Running | RunStatus::Paused)).then_some(o))
    }

    fn last_run(&self) -> anyhow::Result<Option<RunOverview>> {
        let scan = self.scan()?;
        match scan.last_ended_run()? {
            Some(run) => Ok(Some(scan.run_overview(run)?)),
            None => Ok(None),
        }
    }

    fn run_errors(&self, run: ScanRunId, limit: usize) -> anyhow::Result<Vec<RunError>> {
        Ok(self.scan()?.run_errors(run, limit)?)
    }

    fn last_library(&self) -> Option<PathBuf> {
        self.scan().ok()?.exports().last_library().ok().flatten()
    }

    fn export_plan(
        &self,
        library: &Path,
        settings: ProfileSettings,
    ) -> anyhow::Result<PlanPreview> {
        let scan = self.scan()?;
        let profile = settings.profile;
        let (plan, input) = scan.exports().plan(library, settings)?;
        let mut offline: HashMap<String, u64> = HashMap::new();
        for u in &plan.unavailable {
            *offline.entry(u.source_name.clone()).or_default() += 1;
        }
        let mut unavailable_sources: Vec<(String, u64)> = offline.into_iter().collect();
        unavailable_sources.sort();
        let examples = plan
            .files()
            .take(PLAN_ROWS_LIMIT)
            .map(|f| PlanRow {
                source: input
                    .source(f.source)
                    .map(|s| s.name.clone())
                    .unwrap_or_default(),
                name: f
                    .source_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                target: f.target.display().into_owned(),
            })
            .collect();
        Ok(PlanPreview {
            library: plan.library_root.clone(),
            profile,
            sidecars: settings.sidecars,
            files: plan.totals.files,
            bytes: plan.totals.bytes,
            adopted: plan.totals.adopted,
            collisions: plan.totals.collisions,
            sidecar_files: plan.totals.sidecars,
            unavailable: plan.totals.unavailable,
            unavailable_sources,
            already_exported: plan.totals.already_exported,
            modified: plan.totals.modified,
            free_bytes: plan.free_bytes,
            required_bytes: plan.required_bytes,
            blocker: plan.blocker().map(|b| b.to_string()),
            examples,
            date_note: plan.totals.dates.note(),
            pending_files: {
                // The same dispositions the Reports view and the Library's filter count, when this plan is for
                // the library they are derived against (or before any export).
                let last = scan.exports().last_library()?;
                last.as_deref()
                    .is_none_or(|l| l == plan.library_root.as_path())
                    .then(|| self.disposition_matches(&[Disposition::Pending]))
                    .and_then(|r| r.ok())
                    .map(|s| s.len() as u64)
            },
        })
    }

    fn export_run(
        &self,
        library: &Path,
        settings: ProfileSettings,
        cancel: &AtomicBool,
        progress: &ExportProgress,
    ) -> anyhow::Result<ExportOutcome> {
        let scan = self.scan()?;
        let cfg = ExecConfig::default();
        let result = scan.export_with_progress(library, settings, &cfg, cancel, progress);
        self.forget_dispositions();
        let (plan, report) = result?;
        Ok(ExportOutcome {
            library: plan.library_root,
            copied: report.copied,
            adopted: report.adopted,
            bytes_copied: report.bytes_copied,
            sidecars: report.sidecars,
            failed: report
                .failed
                .iter()
                .map(|f| (f.target.display().into_owned(), f.error.clone()))
                .collect(),
            cancelled: matches!(
                report.stopped,
                Some(ps_service::export_progress::StopReason::Cancelled)
            ),
            stopped: report.stopped.as_ref().map(ToString::to_string),
            elapsed_ms: report.elapsed.as_millis() as u64,
        })
    }

    fn report(&self, library: Option<&Path>, verify: bool) -> anyhow::Result<Option<ReportData>> {
        let scan = self.scan()?;
        let ex = scan.exports();
        if library.is_none() && ex.last_library()?.is_none() {
            return Ok(None);
        }
        let c = ex.completion(library, verify)?;
        let names: HashMap<SourceId, String> = scan
            .sources()?
            .into_iter()
            .map(|s| (s.id, source_name(&s.root)))
            .collect();
        Ok(Some(ReportData {
            library: c.library,
            totals: c.totals,
            mismatches: c.report.mismatches.len() as u64,
            mismatch_rows: c
                .report
                .mismatches
                .iter()
                .take(200)
                .map(|(p, check)| (p.display().into_owned(), copy_check_label(*check)))
                .collect(),
            verified: c.verified,
            verified_at_ms: c.verified_at_ms,
            sources: c
                .report
                .sources
                .iter()
                .map(|s| SourceReportRow {
                    id: s.source,
                    name: names
                        .get(&s.source)
                        .cloned()
                        .unwrap_or_else(|| format!("Source {}", s.source.0)),
                    counts: s.counts.clone(),
                    excluded: excluded_rows(&s.excluded),
                    folders: s
                        .folders
                        .iter()
                        .map(|f| FolderRow {
                            folder: f.folder.as_ref().map(|p| p.display().into_owned()),
                            counts: f.counts.clone(),
                            excluded: excluded_rows(&f.excluded),
                        })
                        .collect(),
                })
                .collect(),
        }))
    }

    fn drill(&self, library: &Path, q: &DrillQuery, limit: usize) -> anyhow::Result<Vec<DrillRow>> {
        let scan = self.scan()?;
        let (files, d, _) = scan.exports().dispositions(library, false)?;
        let root = scan
            .sources()?
            .into_iter()
            .find(|s| s.id == q.source)
            .map(|s| s.root)
            .unwrap_or_default();
        Ok(files
            .iter()
            .zip(d)
            .filter(|(f, x)| {
                f.source == q.source
                    && x.disposition == q.disposition
                    && q.folder.as_ref().is_none_or(|want| {
                        f.rel_path.parent().map(|p| p.display().into_owned()) == *want
                    })
            })
            .take(limit)
            .map(|(f, x)| DrillRow {
                file: f.file.0,
                name: String::from_utf8_lossy(f.rel_path.file_name()).into_owned(),
                path: f.rel_path.under(&root).display().to_string(),
                size: f.size,
                disposition: x.disposition,
                reason: disposition_reason(
                    x.disposition,
                    x.reason,
                    x.library_path.as_ref(),
                    x.quarantine_path.as_ref(),
                ),
            })
            .collect())
    }

    fn cleanup_runs(&self) -> anyhow::Result<Vec<CleanupRunRow>> {
        let scan = self.scan()?;
        if !scan.is_observer() {
            scan.cleanups().reconcile()?;
        }
        Ok(scan.cleanups().runs()?.iter().map(cleanup_row).collect())
    }

    fn cleanup_review(&self, run: i64) -> anyhow::Result<CleanupReview> {
        self.cleanup_review_now(run)
    }

    fn cleanup_plan(&self, source: SourceId) -> anyhow::Result<Option<CleanupReview>> {
        let plan = self.scan()?.cleanups().plan(source, None, None)?;
        match plan.summary {
            Some(s) => Ok(Some(self.cleanup_review_now(s.run.id)?)),
            None => Ok(None),
        }
    }

    fn cleanup_run(
        &self,
        run: i64,
        network_confirmed: bool,
        cancel: &AtomicBool,
        progress: &CleanupProgress,
    ) -> anyhow::Result<CleanupOutcome> {
        let r = self.scan()?.cleanups().run(
            run,
            ps_service::cleanup::RunOptions { network_confirmed },
            cancel,
            progress,
        );
        self.forget_dispositions();
        let r = r?;
        Ok(CleanupOutcome {
            run,
            moved: r.moved,
            bytes_moved: r.bytes_moved,
            skipped: r
                .skipped
                .iter()
                .map(|s| (s.rel_path.display().into_owned(), s.text.clone()))
                .collect(),
            cancelled: r.cancelled,
            not_attempted: r.not_attempted,
            elapsed_ms: r.elapsed_ms,
        })
    }

    fn cleanup_undo(&self, run: i64, selection: CleanupSelection) -> anyhow::Result<UndoOutcome> {
        let r = self.scan()?.cleanups().undo(run, &selection);
        self.forget_dispositions();
        let r = r?;
        Ok(UndoOutcome {
            run,
            restored: r.restored,
            bytes_restored: r.bytes_restored,
            conflicts: r
                .conflicts
                .iter()
                .map(|c| (c.rel_path.display().into_owned(), c.text.clone()))
                .collect(),
            missing: r.missing.len() as u64,
            changed: r.changed.len() as u64,
            fully_undone: r.run_fully_undone,
        })
    }

    fn cleanup_tree(&self, run: i64, kind: CleanupTreeKind) -> anyhow::Result<CleanupTree> {
        self.cleanup_tree_now(run, kind)
    }

    fn cleanup_folder_files(
        &self,
        run: i64,
        folder: Option<RelPath>,
        kind: CleanupTreeKind,
    ) -> anyhow::Result<Vec<CleanupFileRow>> {
        self.cleanup_folder_files_now(run, folder, kind)
    }

    fn cleanup_select(
        &self,
        run: i64,
        selection: CleanupSelection,
    ) -> anyhow::Result<CleanupReview> {
        self.scan()?.cleanups().select(run, &selection)?;
        self.cleanup_review_now(run)
    }

    fn cleanup_discard(&self, run: i64) -> anyhow::Result<CleanupRunRow> {
        let s = self.scan()?.cleanups().discard(run)?;
        Ok(cleanup_row(&s))
    }
}

impl CatalogBackend {
    fn cleanup_tree_now(&self, run: i64, kind: CleanupTreeKind) -> anyhow::Result<CleanupTree> {
        let (_, moves) = self.scan()?.cleanups().run_detail(run)?;
        Ok(cleanup_tree_of(run, kind, &moves))
    }

    fn cleanup_folder_files_now(
        &self,
        run: i64,
        folder: Option<RelPath>,
        kind: CleanupTreeKind,
    ) -> anyhow::Result<Vec<CleanupFileRow>> {
        let moves = self.scan()?.cleanups().folder_moves(run, folder.as_ref())?;
        Ok(moves
            .iter()
            .filter_map(|m| {
                Some(CleanupFileRow {
                    picked: tree_pick(kind, m.state)?,
                    file: m.file,
                    name: String::from_utf8_lossy(m.rel_path.file_name()).into_owned(),
                    size: m.size,
                })
            })
            .collect())
    }

    fn cleanup_review_now(&self, run: i64) -> anyhow::Result<CleanupReview> {
        let (summary, moves) = self.scan()?.cleanups().run_detail(run)?;
        Ok(cleanup_review_of(&summary, &moves))
    }
}

/// What is wrong with a library copy.
pub fn copy_check_label(c: ps_service::export_progress::CopyCheck) -> String {
    ps_service::export_progress::copy_check_text(c)
}

// ---- the simulated engine ---------------------------------------------------------------------------------

/// A complete, I/O-free backend: a [`FakeScanService`] for scans plus a scripted export and report. Used by
/// UI tests and by `--demo-scan` screenshots.
pub struct DemoBackend {
    pub scan: FakeScanService,
    pub library: Mutex<Option<PathBuf>>,
    pub report: Mutex<Option<ReportData>>,
    /// Drill-down rows by source and folder.
    pub drill_rows: Mutex<Vec<(SourceId, Option<String>, DrillRow)>>,
    /// Export calls received (tests).
    pub exports_started: Mutex<Vec<(PathBuf, ProfileSettings)>>,
    pub removed: Mutex<Vec<SourceId>>,
    pub relinked: Mutex<Vec<(SourceId, PathBuf)>>,
    pub added: Mutex<Vec<PathBuf>>,
    pub started: Mutex<Vec<Vec<SourceId>>>,
    /// Bytes of free space reported by the plan.
    pub free_bytes: Mutex<Option<u64>>,
    /// Whether an export run returns at once (tests) or waits for cancel.
    pub export_blocks: AtomicBool,
    /// A fixed snapshot returned instead of the simulation's (canned screenshots, rendering tests).
    pub pinned: Mutex<Option<ProgressSnapshot>>,
    /// Simulates a fresh app start after the run was interrupted: no live snapshot until the run is resumed.
    pub restarted: AtomicBool,
    /// Source roots of the real catalog the simulation mirrors (screenshots of a seeded demo library).
    pub roots: Mutex<HashMap<SourceId, PathBuf>>,
    /// An in-memory cleanup plan (UI tests of the selection tree); nothing on disk.
    pub cleanup: Mutex<Option<DemoCleanup>>,
}

/// An in-memory cleanup run for [`DemoBackend`]: selecting and discarding change only these rows.
#[derive(Clone, Debug)]
pub struct DemoCleanup {
    pub run: CleanupRunRow,
    pub moves: Vec<ps_model::CleanupMove>,
    /// Selections received (tests).
    pub selections: Vec<CleanupSelection>,
}

impl DemoCleanup {
    /// A planned run of `folders` folders (`Folder NNNNN/IMG_NNNN.JPG` paths) with `files` photos each.
    pub fn synthetic(run: i64, folders: usize, files: usize) -> Self {
        let mut moves = Vec::with_capacity(folders * files);
        for f in 0..folders {
            for i in 0..files {
                let path = format!("Folder {f:05}/IMG_{i:04}.JPG");
                let quarantine = format!(".photosquirrel-quarantine/{run}/{path}");
                let (Some(rel), Some(quarantine_rel_path)) = (
                    RelPath::from_bytes(path.into_bytes()),
                    RelPath::from_bytes(quarantine.into_bytes()),
                ) else {
                    continue;
                };
                moves.push(ps_model::CleanupMove {
                    file: FileId((f * files + i) as i64 + 1),
                    quarantine_rel_path,
                    rel_path: rel,
                    size: 1_000_000 + (i as u64) * 1000,
                    mtime_ns: 0,
                    file_index: None,
                    content: ps_model::ContentId::full([0; 32]),
                    disposition: Disposition::Exported,
                    covering_library_path: RelPath::from_bytes(b"lib.jpg".to_vec())
                        .unwrap_or_else(|| unreachable!("a valid path")),
                    state: ps_model::MoveState::Planned,
                    note: None,
                    at_ms: None,
                });
            }
        }
        let mut c = DemoCleanup {
            run: CleanupRunRow {
                id: run,
                source: SourceId(1),
                source_name: "Demo Drive".into(),
                state: CleanupRunState::Planned,
                created_at_ms: now_ms(),
                planned: 0,
                planned_bytes: 0,
                quarantined: 0,
                quarantined_bytes: 0,
                skipped: 0,
                restored: 0,
                unselected: 0,
                unselected_bytes: 0,
                quarantine_dir: PathBuf::from(format!("/demo/.photosquirrel-quarantine/{run}")),
                network: false,
            },
            moves,
            selections: Vec::new(),
        };
        c.recount();
        c
    }

    fn recount(&mut self) {
        use ps_model::MoveState as S;
        let r = &mut self.run;
        (r.planned, r.planned_bytes, r.unselected, r.unselected_bytes) = (0, 0, 0, 0);
        (r.quarantined, r.quarantined_bytes) = (0, 0);
        for m in &self.moves {
            match m.state {
                S::Planned => {
                    r.planned += 1;
                    r.planned_bytes += m.size;
                }
                S::Unselected => {
                    r.unselected += 1;
                    r.unselected_bytes += m.size;
                }
                S::Quarantined => {
                    r.quarantined += 1;
                    r.quarantined_bytes += m.size;
                }
                _ => {}
            }
        }
    }

    fn review(&self) -> CleanupReview {
        let planned: Vec<&ps_model::CleanupMove> = self
            .moves
            .iter()
            .filter(|m| m.state == ps_model::MoveState::Planned)
            .collect();
        let totals = ps_service::cleanup::totals_of(planned.iter().copied());
        CleanupReview {
            run: self.run.clone(),
            root: PathBuf::from("/demo"),
            library: None,
            folders: totals
                .folders
                .iter()
                .map(|f| {
                    (
                        f.folder.as_ref().map_or_else(
                            || "(source root)".to_owned(),
                            |p| p.display().into_owned(),
                        ),
                        f.files,
                        f.bytes,
                    )
                })
                .collect(),
            moves: planned
                .iter()
                .take(CLEANUP_ROWS_LIMIT)
                .map(|m| {
                    (
                        m.rel_path.display().into_owned(),
                        m.quarantine_rel_path.display().into_owned(),
                    )
                })
                .collect(),
            notes: Vec::new(),
        }
    }
}

impl DemoBackend {
    pub fn new(scan: FakeScanService) -> Self {
        Self {
            scan,
            library: Mutex::new(None),
            report: Mutex::new(None),
            drill_rows: Mutex::new(Vec::new()),
            exports_started: Mutex::new(Vec::new()),
            removed: Mutex::new(Vec::new()),
            relinked: Mutex::new(Vec::new()),
            added: Mutex::new(Vec::new()),
            started: Mutex::new(Vec::new()),
            free_bytes: Mutex::new(Some(812_000_000_000)),
            export_blocks: AtomicBool::new(false),
            pinned: Mutex::new(None),
            restarted: AtomicBool::new(false),
            roots: Mutex::new(HashMap::new()),
            cleanup: Mutex::new(None),
        }
    }

    /// [`DemoBackend::screenshot`] mirroring a real catalog: the simulated run scans the catalog's own sources
    /// (names, roots, file counts), so the Sources view agrees with the navigator and the status bar of the
    /// same seeded library. Falls back to the scripted sources when `real` is empty.
    pub fn screenshot_of(mode: &str, library: PathBuf, real: &[SourceRow]) -> Self {
        use ps_service::scans::{FakeConfig, FakeSource};
        if real.is_empty() {
            return Self::screenshot(mode, library);
        }
        let devices = [
            DeviceClass::SolidState,
            DeviceClass::Rotational,
            DeviceClass::Network,
        ];
        let sources: Vec<FakeSource> = real
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let s = FakeSource::new(r.id.0, &r.name, devices[i % 3], r.files.max(1));
                if i == 0 {
                    // The first drive is a rescan: most files unchanged.
                    s.rescan(r.files / 8, r.files / 40, 1)
                } else {
                    s
                }
            })
            .collect();
        let work: u64 = real.iter().map(|r| r.files).sum::<u64>().max(1);
        let config = FakeConfig {
            sources,
            // About three minutes of work, so the "running" state (100 s in) is a little past half way.
            files_per_s: work as f64 / 170.0,
            bytes_per_file: 600_000,
            walk_ms: 6_000,
            tick_ms: 100,
            error_every: (work / 3).max(1),
            skip_percent: 12,
            cancel_ms: 0,
        };
        let scan = FakeScanService::new(config);
        let all: Vec<SourceId> = real.iter().map(|r| r.id).collect();
        let backend = match mode {
            "paused" => {
                scan.start_at(&all, 100_000, RunStatus::Paused);
                Self::new(scan)
            }
            "interrupted" => {
                scan.start_at(&all, 64_000, RunStatus::Interrupted);
                let b = Self::new(scan);
                b.restarted.store(true, Ordering::SeqCst);
                b
            }
            "complete" => {
                scan.start_at(&all, 354_200, RunStatus::Running);
                scan.advance(1_000);
                Self::new(scan)
            }
            _ => {
                scan.start_at(&all, 100_000, RunStatus::Running);
                Self::new(scan)
            }
        };
        *Self::lock(&backend.library) = Some(library);
        *Self::lock(&backend.roots) = real.iter().map(|r| (r.id, r.root.clone())).collect();
        backend
    }

    /// The scripted states of `photosquirrel --demo-scan MODE` (screenshots): three sources (an SSD rescan,
    /// a NAS, and a USB drive that went offline) and a library for Export and Reports.
    /// - `running`: a run 1 min 40 s in, analysing, with some unreadable files;
    /// - `paused`: the same run, paused;
    /// - `interrupted`: the app restarted after the run was interrupted (no live run, Resume offered);
    /// - `complete`: the run finished (summary).
    pub fn screenshot(mode: &str, library: PathBuf) -> Self {
        use ps_service::scans::{FakeConfig, FakeSource};
        let mut usb = FakeSource::new(3, "USB · Backup 2014", DeviceClass::Rotational, 9_000);
        usb.online = false;
        let config = FakeConfig {
            sources: vec![
                FakeSource::new(1, "Pictures", DeviceClass::SolidState, 61_204)
                    .rescan(1_240, 85, 12),
                FakeSource::new(2, "NAS · Family", DeviceClass::Network, 38_417),
                usb,
            ],
            files_per_s: 140.0,
            bytes_per_file: 3_800_000,
            walk_ms: 6_000,
            tick_ms: 100,
            error_every: 1_900,
            skip_percent: 12,
            cancel_ms: 0,
        };
        let scan = FakeScanService::new(config);
        let all = [SourceId(1), SourceId(2), SourceId(3)];
        let backend = match mode {
            "paused" => {
                scan.start_at(&all, 100_000, RunStatus::Paused);
                Self::new(scan)
            }
            "interrupted" => {
                scan.start_at(&all, 64_000, RunStatus::Interrupted);
                let b = Self::new(scan);
                b.restarted.store(true, Ordering::SeqCst);
                b
            }
            "complete" => {
                scan.start_at(&all, 354_200, RunStatus::Running);
                scan.advance(1_000);
                Self::new(scan)
            }
            _ => {
                scan.start_at(&all, 100_000, RunStatus::Running);
                Self::new(scan)
            }
        };
        *Self::lock(&backend.library) = Some(library);
        backend
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A report over the fake sources: the first fully accounted for, the others with pending files.
    pub fn sample_report(&self, library: &Path) -> ReportData {
        let mut sources = Vec::new();
        for (i, s) in self.scan.sources().iter().enumerate() {
            let mut folders = Vec::new();
            let names = ["2019/Holiday", "2020/Garden", "2021/Birthday", "Downloads"];
            for (j, name) in names.iter().enumerate() {
                let mut c = CompletionCounts::default();
                let base = 40 + (j as u64) * 25 + (i as u64) * 7;
                for _ in 0..base {
                    c.add(Disposition::Exported, 3_400_000);
                }
                for _ in 0..base / 2 {
                    c.add(Disposition::Covered, 900_000);
                }
                for _ in 0..(j as u64 + 1) * 3 {
                    c.add(Disposition::Excluded, 40_000);
                }
                if i > 0 && j == 3 {
                    for _ in 0..12 {
                        c.add(Disposition::Pending, 2_800_000);
                    }
                }
                if i > 0 && j == 1 {
                    c.add(Disposition::Unreadable, 1_200_000);
                }
                folders.push(FolderRow {
                    folder: Some((*name).to_owned()),
                    excluded: vec![
                        (
                            ExclusionReason::Junk,
                            c.files_of(Disposition::Excluded) / 2,
                            0,
                        ),
                        (
                            ExclusionReason::Graphic,
                            c.files_of(Disposition::Excluded)
                                - c.files_of(Disposition::Excluded) / 2,
                            0,
                        ),
                    ],
                    counts: c,
                });
            }
            let mut counts = CompletionCounts::default();
            for f in &folders {
                counts.merge(&f.counts);
            }
            let excluded = vec![
                (
                    ExclusionReason::Junk,
                    folders.iter().map(|f| f.excluded[0].1).sum(),
                    0,
                ),
                (
                    ExclusionReason::Graphic,
                    folders.iter().map(|f| f.excluded[1].1).sum(),
                    0,
                ),
            ];
            sources.push(SourceReportRow {
                id: s.id,
                name: s.name.clone(),
                counts,
                excluded,
                folders,
            });
        }
        let mut totals = CompletionCounts::default();
        for s in &sources {
            totals.merge(&s.counts);
        }
        ReportData {
            library: library.to_path_buf(),
            totals,
            sources,
            mismatches: 0,
            mismatch_rows: Vec::new(),
            verified: false,
            verified_at_ms: Some(now_ms() - 3 * 86_400_000),
        }
    }
}

impl ScanBackend for DemoBackend {
    fn cleanup_runs(&self) -> anyhow::Result<Vec<CleanupRunRow>> {
        Ok(Self::lock(&self.cleanup)
            .as_ref()
            .map(|c| vec![c.run.clone()])
            .unwrap_or_default())
    }

    fn cleanup_review(&self, run: i64) -> anyhow::Result<CleanupReview> {
        match Self::lock(&self.cleanup).as_ref() {
            Some(c) if c.run.id == run => Ok(c.review()),
            _ => anyhow::bail!("unknown cleanup run {run}"),
        }
    }

    fn cleanup_tree(&self, run: i64, kind: CleanupTreeKind) -> anyhow::Result<CleanupTree> {
        match Self::lock(&self.cleanup).as_ref() {
            Some(c) if c.run.id == run => Ok(cleanup_tree_of(run, kind, &c.moves)),
            _ => anyhow::bail!("unknown cleanup run {run}"),
        }
    }

    fn cleanup_folder_files(
        &self,
        run: i64,
        folder: Option<RelPath>,
        kind: CleanupTreeKind,
    ) -> anyhow::Result<Vec<CleanupFileRow>> {
        let g = Self::lock(&self.cleanup);
        let Some(c) = g.as_ref().filter(|c| c.run.id == run) else {
            anyhow::bail!("unknown cleanup run {run}");
        };
        Ok(c.moves
            .iter()
            .filter(|m| m.rel_path.parent() == folder)
            .filter_map(|m| {
                Some(CleanupFileRow {
                    picked: tree_pick(kind, m.state)?,
                    file: m.file,
                    name: String::from_utf8_lossy(m.rel_path.file_name()).into_owned(),
                    size: m.size,
                })
            })
            .collect())
    }

    fn cleanup_select(
        &self,
        run: i64,
        selection: CleanupSelection,
    ) -> anyhow::Result<CleanupReview> {
        use ps_model::MoveState as S;
        let mut g = Self::lock(&self.cleanup);
        let Some(c) = g.as_mut().filter(|c| c.run.id == run) else {
            anyhow::bail!("unknown cleanup run {run}");
        };
        if !c
            .moves
            .iter()
            .any(|m| matches!(m.state, S::Planned | S::Unselected) && selection.picks(m))
        {
            anyhow::bail!(
                "this selection leaves no file of cleanup run {run} to move: discard the plan instead"
            );
        }
        for m in &mut c.moves {
            if matches!(m.state, S::Planned | S::Unselected) {
                m.state = if selection.picks(m) {
                    S::Planned
                } else {
                    S::Unselected
                };
            }
        }
        c.selections.push(selection);
        c.recount();
        Ok(c.review())
    }

    fn cleanup_discard(&self, run: i64) -> anyhow::Result<CleanupRunRow> {
        let mut g = Self::lock(&self.cleanup);
        let Some(c) = g.as_mut().filter(|c| c.run.id == run) else {
            anyhow::bail!("unknown cleanup run {run}");
        };
        for m in &mut c.moves {
            if m.state == ps_model::MoveState::Planned {
                m.state = ps_model::MoveState::Unselected;
            }
        }
        c.run.state = CleanupRunState::Discarded;
        c.recount();
        Ok(c.run.clone())
    }

    fn sources(&self) -> anyhow::Result<Vec<SourceRow>> {
        Ok(self
            .scan
            .sources()
            .into_iter()
            .map(|s| SourceRow {
                id: s.id,
                name: s.name.clone(),
                root: Self::lock(&self.roots)
                    .get(&s.id)
                    .cloned()
                    .unwrap_or_else(|| demo_root(&s.name)),
                online: s.online,
                device: s.device,
                last_scan_ms: Some(now_ms() - s.id.0 * 86_400_000 * 3),
                files: s.files,
                finished: s.files - s.files / (3 + s.id.0 as u64 * 4),
            })
            .collect())
    }

    fn add_source(&self, root: &Path) -> anyhow::Result<SourceId> {
        Self::lock(&self.added).push(root.to_path_buf());
        Ok(SourceId(99))
    }

    fn relink_source(&self, id: SourceId, new_root: &Path) -> anyhow::Result<()> {
        Self::lock(&self.relinked).push((id, new_root.to_path_buf()));
        Ok(())
    }

    fn remove_source(&self, id: SourceId) -> anyhow::Result<()> {
        Self::lock(&self.removed).push(id);
        Ok(())
    }

    fn start(&self, sources: &[SourceId]) -> anyhow::Result<ScanRunId> {
        Self::lock(&self.started).push(sources.to_vec());
        Ok(self.scan.start(sources)?)
    }

    fn pause(&self) -> anyhow::Result<()> {
        self.scan.pause()?;
        Ok(())
    }

    fn resume(&self) -> anyhow::Result<()> {
        self.scan.resume()?;
        Ok(())
    }

    fn cancel(&self) -> anyhow::Result<()> {
        self.scan.cancel()?;
        Ok(())
    }

    /// On the simulated engine's own clock: the GPUI clock does not move.
    fn wait_stopped(&self, limit: std::time::Duration) -> bool {
        self.scan.wait_stopped(limit.as_millis() as u64)
    }

    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()> {
        self.scan.resume_run(run)?;
        self.restarted.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn snapshot(&self) -> Option<ProgressSnapshot> {
        if self.restarted.load(Ordering::SeqCst) {
            return None;
        }
        let snap = self.scan.snapshot();
        Self::lock(&self.pinned).clone().or(snap)
    }

    fn overview(&self, run: ScanRunId) -> anyhow::Result<Option<RunOverview>> {
        Ok(self.scan.overview().filter(|o| o.run == run))
    }

    fn last_run(&self) -> anyhow::Result<Option<RunOverview>> {
        Ok(self.scan.overview())
    }

    fn run_errors(&self, _: ScanRunId, limit: usize) -> anyhow::Result<Vec<RunError>> {
        let snap = self.scan.peek();
        Ok(snap
            .map(|s| {
                s.recent_errors
                    .iter()
                    .rev()
                    .take(limit)
                    .map(|e| RunError {
                        source: e.source,
                        path: e.path.clone(),
                        stage: e.stage,
                        message: e.message.clone(),
                        at_ms: 0,
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn last_library(&self) -> Option<PathBuf> {
        Self::lock(&self.library).clone()
    }

    fn export_plan(
        &self,
        library: &Path,
        settings: ProfileSettings,
    ) -> anyhow::Result<PlanPreview> {
        let profile = settings.profile;
        let free = *Self::lock(&self.free_bytes);
        let bytes = 41_872_000_000;
        let required = bytes + (1 << 20);
        Ok(PlanPreview {
            library: library.to_path_buf(),
            profile,
            sidecars: settings.sidecars,
            files: 12_418,
            pending_files: Some(31_007),
            bytes,
            adopted: 37,
            collisions: 214,
            sidecar_files: if settings.sidecars { 12_418 } else { 0 },
            unavailable: 96,
            unavailable_sources: vec![("USB · Backup 2014".into(), 96)],
            already_exported: 3_051,
            modified: 0,
            free_bytes: free,
            required_bytes: required,
            blocker: free.filter(|f| *f < required).map(|f| {
                format!(
                    "not enough free space in the library: {required} bytes needed, {f} free ({} short)",
                    required - f
                )
            }),
            examples: vec![
                PlanRow::new("Pictures", "IMG_0042.JPG", "2011/2011-08-14/IMG_0042.JPG"),
                PlanRow::new("Pictures", "DSC_1180.NEF", "2016/2016-05-02/DSC_1180.NEF"),
                PlanRow::new(
                    "NAS · Family",
                    "IMG_0042.JPG",
                    "2012/2012-12-24/IMG_0042 (2).JPG",
                ),
                PlanRow::new("Pictures", "IMG_0051.JPG", "2011/2011-08-15/IMG_0051.JPG"),
                PlanRow::new("NAS · Family", "IMG_2210.HEIC", "2019/2019-07-04/IMG_2210.HEIC"),
                PlanRow::new("NAS · Family", "IMG_2210.MOV", "2019/2019-07-04/IMG_2210.MOV"),
                PlanRow::new("Pictures", "DSC_1181.NEF", "2016/2016-05-02/DSC_1181.NEF"),
            ],
            date_note: ps_service::export_progress::DateTotals {
                own: 11_977,
                other_copy: 318,
                file_name: 41,
                sidecar: 0,
                undated: 82,
            }
            .note(),
        })
    }

    fn export_run(
        &self,
        library: &Path,
        settings: ProfileSettings,
        cancel: &AtomicBool,
        _progress: &ExportProgress,
    ) -> anyhow::Result<ExportOutcome> {
        Self::lock(&self.exports_started).push((library.to_path_buf(), settings));
        *Self::lock(&self.library) = Some(library.to_path_buf());
        while self.export_blocks.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let cancelled = cancel.load(Ordering::SeqCst);
        Ok(ExportOutcome {
            library: library.to_path_buf(),
            copied: if cancelled { 1_204 } else { 12_381 },
            adopted: 37,
            bytes_copied: 41_700_000_000,
            sidecars: 12_381,
            failed: Vec::new(),
            cancelled,
            stopped: cancelled.then(|| "Cancelled".into()),
            elapsed_ms: 1_284_000,
        })
    }

    fn report(&self, library: Option<&Path>, verify: bool) -> anyhow::Result<Option<ReportData>> {
        if let Some(mut r) = Self::lock(&self.report).clone() {
            r.verified = verify;
            return Ok(Some(r));
        }
        let lib = library
            .map(Path::to_path_buf)
            .or_else(|| Self::lock(&self.library).clone());
        Ok(lib.map(|l| {
            let mut r = self.sample_report(&l);
            r.verified = verify;
            r
        }))
    }

    fn drill(&self, _: &Path, q: &DrillQuery, limit: usize) -> anyhow::Result<Vec<DrillRow>> {
        Ok(Self::lock(&self.drill_rows)
            .iter()
            .filter(|(s, f, r)| {
                *s == q.source
                    && r.disposition == q.disposition
                    && q.folder.as_ref().is_none_or(|want| want == f)
            })
            .take(limit)
            .map(|(_, _, r)| r.clone())
            .collect())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Where a simulated source lives ("NAS · Family" -> `/mnt/nas/Family`).
fn demo_root(name: &str) -> PathBuf {
    let leaf = name.rsplit(" · ").next().unwrap_or(name);
    match name.split(" · ").next() {
        Some("NAS") => PathBuf::from("/mnt/nas").join(leaf),
        Some("USB") => PathBuf::from("/media/usb").join(leaf),
        _ => PathBuf::from("/home/me").join(leaf),
    }
}

/// A sampled export (for the view): the counters plus the plan totals.
pub type ExportSample = ExportSnapshot;

/// Thumbnails for the simulated engine's recent finds: the fake run names photos by synthetic content ids;
/// this source answers them with real thumbnails from `inner` (round robin over `real`), and passes every
/// other call through. Only `--demo-scan` and tests use it.
pub struct DemoThumbs {
    pub inner: ui_gallery::source::SharedSource,
    pub real: Vec<ui_gallery::source::ThumbKey>,
}

impl DemoThumbs {
    /// Answer the simulated run's photos with the first thumbnails of `inner`'s library (none when empty).
    pub fn over(inner: ui_gallery::source::SharedSource) -> Self {
        let query = crate::views::library::default_query();
        let real = inner
            .id_list(&query)
            .ok()
            .map(|list| list.ids.into_iter().take(96).collect::<Vec<_>>())
            .and_then(|ids| inner.summaries(&ids).ok())
            .map(|rows| rows.into_iter().filter_map(|r| r.thumb).collect())
            .unwrap_or_default();
        Self { inner, real }
    }

    /// The fake engine's synthetic digest for photo `n`, or `None` for a real key.
    fn synthetic_index(key: &ui_gallery::source::ThumbKey) -> Option<u64> {
        (key.0[8] == 0xfa && key.0[9..].iter().all(|b| *b == 0)).then(|| {
            let mut n = [0u8; 8];
            n.copy_from_slice(&key.0[..8]);
            u64::from_le_bytes(n)
        })
    }
}

impl ui_gallery::source::GallerySource for DemoThumbs {
    fn id_list(&self, q: &ui_gallery::source::GalleryQuery) -> anyhow::Result<ps_model::IdList> {
        self.inner.id_list(q)
    }
    fn summaries(
        &self,
        ids: &[ps_model::ItemId],
    ) -> anyhow::Result<Vec<ui_gallery::source::ItemSummary>> {
        self.inner.summaries(ids)
    }
    fn thumbnail(&self, key: &ui_gallery::source::ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        match Self::synthetic_index(key) {
            Some(n) if !self.real.is_empty() => {
                let real = self.real[(n as usize) % self.real.len()];
                self.inner.thumbnail(&real)
            }
            _ => self.inner.thumbnail(key),
        }
    }
    fn preview(&self, id: ps_model::ItemId) -> anyhow::Result<ui_gallery::source::Preview> {
        self.inner.preview(id)
    }
    fn stack_members(&self, keeper: ps_model::ItemId) -> anyhow::Result<Vec<ps_model::ItemId>> {
        self.inner.stack_members(keeper)
    }
    fn detail(
        &self,
        id: ps_model::ItemId,
    ) -> anyhow::Result<Option<ui_gallery::source::ItemDetail>> {
        self.inner.detail(id)
    }
    fn set_decision(
        &self,
        id: ps_model::ItemId,
        decision: Option<ui_gallery::source::Decision>,
        reason: Option<String>,
    ) -> anyhow::Result<()> {
        self.inner.set_decision(id, decision, reason)
    }
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        self.inner.camera_models()
    }
    fn excluded(&self) -> anyhow::Result<Vec<ui_gallery::source::ExcludedRow>> {
        self.inner.excluded()
    }
}

/// The shell's backend for `--demo-scan` screenshots: the real library, with the navigator's and status bar's
/// source count taken from the simulated scan engine, so the shell and the Sources view agree.
pub struct DemoShellBackend {
    inner: Arc<dyn crate::service::LibraryBackend>,
    sources: u32,
}

impl DemoShellBackend {
    pub fn new(inner: Arc<dyn crate::service::LibraryBackend>, demo: &DemoBackend) -> Self {
        let sources = ScanBackend::sources(demo).map_or(0, |s| s.len() as u32);
        Self { inner, sources }
    }
}

impl crate::service::LibraryBackend for DemoShellBackend {
    fn summary(&self) -> anyhow::Result<crate::service::CatalogSummary> {
        let mut s = self.inner.summary()?;
        s.sources = self.sources;
        Ok(s)
    }
    fn interrupted_run(&self) -> anyhow::Result<Option<crate::service::InterruptedRun>> {
        self.inner.interrupted_run()
    }
    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()> {
        self.inner.resume_run(run)
    }
    fn rescan(&self) -> anyhow::Result<()> {
        self.inner.rescan()
    }
    fn add_source(&self, path: PathBuf) -> anyhow::Result<()> {
        self.inner.add_source(path)
    }
    fn activity(&self) -> Option<crate::service::ActivitySnapshot> {
        self.inner.activity()
    }
    fn gallery(self: Arc<Self>) -> Arc<dyn ui_gallery::source::GallerySource> {
        self.inner.clone().gallery()
    }
    fn scans(self: Arc<Self>) -> Option<SharedScanBackend> {
        self.inner.clone().scans()
    }
    fn review(self: Arc<Self>) -> Option<Arc<dyn crate::review_source::ReviewSource>> {
        self.inner.clone().review()
    }
}
