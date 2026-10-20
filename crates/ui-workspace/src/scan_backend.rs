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
    CompletionCounts, DeviceClass, Disposition, ExclusionReason, ExportProfile, ProgressSnapshot,
    RunStatus, ScanRunId, SourceId, SourceStatus, Stage,
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
    pub free_bytes: Option<u64>,
    pub required_bytes: u64,
    /// Why the export cannot start (insufficient space), if it cannot.
    pub blocker: Option<String>,
    /// A few planned copies (source path -> library path) to show where things go.
    pub examples: Vec<(String, String)>,
}

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
    /// Continue a cancelled, paused or interrupted run.
    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()>;
    /// The active (or last) run's progress. Cheap; sampled at most every 100 ms.
    fn snapshot(&self) -> Option<ProgressSnapshot>;
    /// The summary of a run that has ended.
    fn overview(&self, run: ScanRunId) -> anyhow::Result<Option<RunOverview>>;
    /// The last run that ended (for the dashboard's summary when idle).
    fn last_run(&self) -> anyhow::Result<Option<RunOverview>>;
    /// Every stored error of a run (newest first).
    fn run_errors(&self, run: ScanRunId, limit: usize) -> anyhow::Result<Vec<RunError>>;

    /// The library folder used last.
    fn last_library(&self) -> Option<PathBuf>;
    fn export_plan(&self, library: &Path, profile: ExportProfile) -> anyhow::Result<PlanPreview>;
    /// Run an export to the end (or until `cancel`), reporting into `progress`.
    fn export_run(
        &self,
        library: &Path,
        profile: ExportProfile,
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
}

pub type SharedScanBackend = Arc<dyn ScanBackend>;

/// Display name of a source root.
pub fn source_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

fn profile_settings(profile: ExportProfile) -> ProfileSettings {
    ProfileSettings::defaults(profile)
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

    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()> {
        self.scan()?.resume_run(run)?;
        Ok(())
    }

    fn snapshot(&self) -> Option<ProgressSnapshot> {
        self.scan().ok()?.snapshot()
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

    fn export_plan(&self, library: &Path, profile: ExportProfile) -> anyhow::Result<PlanPreview> {
        let scan = self.scan()?;
        let settings = profile_settings(profile);
        let (plan, input) = scan.exports().plan(library, settings)?;
        let mut offline: HashMap<String, u64> = HashMap::new();
        for u in &plan.unavailable {
            *offline.entry(u.source_name.clone()).or_default() += 1;
        }
        let mut unavailable_sources: Vec<(String, u64)> = offline.into_iter().collect();
        unavailable_sources.sort();
        let examples = plan
            .files()
            .take(6)
            .map(|f| {
                let src = input
                    .source(f.source)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                (
                    format!(
                        "{src}/{}",
                        f.source_path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    f.target.display().into_owned(),
                )
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
            free_bytes: plan.free_bytes,
            required_bytes: plan.required_bytes,
            blocker: plan.blocker().map(|b| b.to_string()),
            examples,
        })
    }

    fn export_run(
        &self,
        library: &Path,
        profile: ExportProfile,
        cancel: &AtomicBool,
        progress: &ExportProgress,
    ) -> anyhow::Result<ExportOutcome> {
        let scan = self.scan()?;
        let cfg = ExecConfig::default();
        let result =
            scan.export_with_progress(library, profile_settings(profile), &cfg, cancel, progress);
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
            stopped: report.stopped.as_ref().map(|s| format!("{s:?}")),
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
                reason: disposition_reason(x.disposition, x.reason, x.library_path.as_ref()),
            })
            .collect())
    }
}

/// What is wrong with a library copy.
pub fn copy_check_label(c: ps_service::export_progress::CopyCheck) -> String {
    use ps_service::export_progress::CopyCheck;
    match c {
        CopyCheck::Ok => "ok".into(),
        CopyCheck::Missing => "missing from the library".into(),
        CopyCheck::SizeMismatch { expected, found } => {
            format!("size changed ({expected} bytes recorded, {found} found)")
        }
        CopyCheck::HashMismatch => "contents changed since it was copied".into(),
    }
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
    pub exports_started: Mutex<Vec<(PathBuf, ExportProfile)>>,
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
        }
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
                for k in 0..6 {
                    counts.files[k] += f.counts.files[k];
                    counts.bytes[k] += f.counts.bytes[k];
                }
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
            for k in 0..6 {
                totals.files[k] += s.counts.files[k];
                totals.bytes[k] += s.counts.bytes[k];
            }
        }
        ReportData {
            library: library.to_path_buf(),
            totals,
            sources,
            mismatches: 0,
            mismatch_rows: Vec::new(),
            verified: false,
        }
    }
}

impl ScanBackend for DemoBackend {
    fn sources(&self) -> anyhow::Result<Vec<SourceRow>> {
        Ok(self
            .scan
            .sources()
            .into_iter()
            .map(|s| SourceRow {
                id: s.id,
                name: s.name.clone(),
                root: demo_root(&s.name),
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

    fn export_plan(&self, library: &Path, profile: ExportProfile) -> anyhow::Result<PlanPreview> {
        let settings = profile_settings(profile);
        let free = *Self::lock(&self.free_bytes);
        let bytes = 41_872_000_000;
        let required = bytes + (1 << 20);
        Ok(PlanPreview {
            library: library.to_path_buf(),
            profile,
            sidecars: settings.sidecars,
            files: 12_418,
            bytes,
            adopted: 37,
            collisions: 214,
            sidecar_files: if settings.sidecars { 12_418 } else { 0 },
            unavailable: 96,
            unavailable_sources: vec![("USB · Backup 2014".into(), 96)],
            already_exported: 3_051,
            free_bytes: free,
            required_bytes: required,
            blocker: free.filter(|f| *f < required).map(|f| {
                format!(
                    "not enough free space in the library: {required} bytes needed, {f} free ({} short)",
                    required - f
                )
            }),
            examples: vec![
                (
                    "Pictures/IMG_0042.JPG".into(),
                    "2011/2011-08-14/IMG_0042.JPG".into(),
                ),
                (
                    "Pictures/DSC_1180.NEF".into(),
                    "2016/2016-05-02/DSC_1180.NEF".into(),
                ),
                (
                    "Family/IMG_0042.JPG".into(),
                    "2012/2012-12-24/IMG_0042 (2).JPG".into(),
                ),
            ],
        })
    }

    fn export_run(
        &self,
        library: &Path,
        profile: ExportProfile,
        cancel: &AtomicBool,
        _progress: &ExportProgress,
    ) -> anyhow::Result<ExportOutcome> {
        Self::lock(&self.exports_started).push((library.to_path_buf(), profile));
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
