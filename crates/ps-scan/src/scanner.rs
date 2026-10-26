//! The engine API (ADR-0027 wraps it in `ps-service`): sources, runs and their control.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use ps_catalog::{Catalog, Command, NewSource, RunRecord, SourceRecord, ThumbStore};
use ps_model::{
    DeviceClass, ProgressSnapshot, RelPath, RunStatus, ScanRunId, SourceId, SourceStatus,
    path_from_bytes, path_to_bytes,
};

use crate::control::{Control, ControlState};
use crate::error::{Result, ScanError};
use crate::memory::ByteBudget;
use crate::options::ScanOptions;
use crate::pipeline::{RunCtx, RunSource, execute, normalize_root};
use crate::post::{GroupEngine, RegroupReport};
use crate::progress::Progress;
use crate::report::RunSummary;
use crate::source::{self, Overlap, VolumeInfo};
use crate::walk::WalkError;

/// A source as the engine reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    pub id: SourceId,
    pub root: PathBuf,
    pub status: SourceStatus,
    pub volume_uuid: Option<String>,
    /// Device class detected when the source was added.
    pub device: DeviceClass,
    pub total_bytes: Option<u64>,
    /// Files sampled into the content fingerprint (at most 64).
    pub fingerprint_samples: usize,
    pub added_at_ms: i64,
}

impl SourceInfo {
    fn from_record(r: &SourceRecord) -> SourceInfo {
        SourceInfo {
            id: r.id,
            root: path_from_bytes(&r.root),
            status: r.status,
            volume_uuid: r.volume_uuid.clone(),
            device: r
                .device_hint
                .as_deref()
                .and_then(VolumeInfo::class_from_hint)
                .unwrap_or_default(),
            total_bytes: r.total_bytes,
            fingerprint_samples: r
                .fingerprint
                .as_deref()
                .map_or(0, |f| source::decode(f).len()),
            added_at_ms: r.added_at_ms,
        }
    }
}

/// The result of adding a source.
#[derive(Clone, Debug, PartialEq)]
pub struct AddedSource {
    pub source: SourceInfo,
    /// Existing sources inside the new root that were folded into it (ADR-0006).
    pub folded: Vec<SourceId>,
}

/// Explicit confirmation for destructive-adjacent catalog operations (ADR-0004).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    Confirmed,
    NotConfirmed,
}

struct ActiveRun {
    run: ScanRunId,
    ctx: Arc<RunCtx>,
    thread: Mutex<Option<JoinHandle<RunSummary>>>,
    summary: Mutex<Option<RunSummary>>,
}

impl ActiveRun {
    fn finished(&self) -> bool {
        self.summary.lock().is_some()
            || self
                .thread
                .lock()
                .as_ref()
                .is_none_or(JoinHandle::is_finished)
    }

    fn join(&self) -> Option<RunSummary> {
        if let Some(t) = self.thread.lock().take() {
            match t.join() {
                Ok(s) => *self.summary.lock() = Some(s),
                Err(_) => log::error!("scan run {} thread panicked", self.run),
            }
        }
        self.summary.lock().clone()
    }
}

/// The scan engine over one catalog.
pub struct Scanner {
    catalog: Arc<Catalog>,
    thumbs: Arc<ThumbStore>,
    /// Read when a run starts ([`Scanner::set_options`] never reaches a running one).
    opts: Mutex<ScanOptions>,
    active: Mutex<Option<Arc<ActiveRun>>>,
    recovered: Vec<ScanRunId>,
    groups: Arc<GroupEngine>,
}

impl std::fmt::Debug for Scanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scanner")
            .field("catalog", &self.catalog.path())
            .field("opts", &*self.opts.lock())
            .finish_non_exhaustive()
    }
}

impl Scanner {
    /// Open the catalog and thumbnail store in `data_dir` and recover from a crash: runs left `running`
    /// become `interrupted` (ADR-0008).
    pub fn open(data_dir: &Path, opts: ScanOptions) -> Result<Scanner> {
        let catalog = Arc::new(Catalog::open(data_dir)?);
        let thumbs = Arc::new(ThumbStore::open(data_dir)?);
        Scanner::with_catalog(catalog, thumbs, opts)
    }

    /// Open the catalog in `data_dir` for read-only use: as an observer (listing, status and reports work,
    /// writes fail with `CatalogError::ReadOnly`, no run is marked interrupted, the owner lock is never
    /// taken), or as the owner only when the catalog must first be created or migrated
    /// (`Catalog::open_reader`). Only the owner, which holds the catalog's exclusive lock, ever recovers runs.
    pub fn open_or_observe(data_dir: &Path, opts: ScanOptions) -> Result<Scanner> {
        let catalog = Arc::new(Catalog::open_reader(
            data_dir,
            ps_catalog::OpenOptions::default(),
        )?);
        let thumbs = Arc::new(ThumbStore::open(data_dir)?);
        Scanner::with_catalog(catalog, thumbs, opts)
    }

    /// Use an already open catalog and store (one engine per catalog).
    pub fn with_catalog(
        catalog: Arc<Catalog>,
        thumbs: Arc<ThumbStore>,
        opts: ScanOptions,
    ) -> Result<Scanner> {
        // Only the owner (the process holding the catalog's exclusive lock, ADR-0005) marks runs left
        // `running` as interrupted: an observer may be looking at another process's live run.
        let before: Vec<ScanRunId> = if opts.recover_interrupted && !catalog.is_observer() {
            catalog
                .read()
                .runs()?
                .into_iter()
                .filter(|r| r.status == RunStatus::Running)
                .map(|r| r.id)
                .collect()
        } else {
            Vec::new()
        };
        if !before.is_empty() {
            catalog.writer().submit(Command::InterruptRunning)?;
            catalog.writer().flush()?;
        }
        Ok(Scanner {
            catalog,
            thumbs,
            opts: Mutex::new(opts),
            active: Mutex::new(None),
            recovered: before,
            groups: Arc::new(GroupEngine::new()),
        })
    }

    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    pub fn thumbs(&self) -> &Arc<ThumbStore> {
        &self.thumbs
    }

    /// The options the next run starts with.
    pub fn options(&self) -> ScanOptions {
        self.opts.lock().clone()
    }

    /// Replace the options for runs started from now on (settings changes, ADR-0051). A running or paused run
    /// keeps the options it started with; a resumed interrupted or cancelled run is a new start and gets these.
    pub fn set_options(&self, opts: ScanOptions) {
        *self.opts.lock() = opts;
    }

    /// Change some options for runs started from now on (see [`Scanner::set_options`]).
    pub fn update_options(&self, f: impl FnOnce(&mut ScanOptions)) {
        f(&mut self.opts.lock());
    }

    /// The post-scan regroup engine.
    pub fn groups(&self) -> &Arc<GroupEngine> {
        &self.groups
    }

    /// Bring the stored duplicate groups up to date now (after constraint changes, source removal or folding,
    /// hash upgrades). Runs after each completed scan automatically.
    pub fn regroup(&self) -> Result<RegroupReport> {
        self.groups.refresh(&self.catalog)
    }

    /// Runs found `running` at open and marked `interrupted` (offered for resume).
    pub fn recovered_runs(&self) -> &[ScanRunId] {
        &self.recovered
    }

    // ---- sources ----------------------------------------------------------------------------------------

    fn records(&self) -> Result<Vec<SourceRecord>> {
        Ok(self
            .catalog
            .read()
            .sources()?
            .into_iter()
            .filter(|s| s.folded_into.is_none())
            .collect())
    }

    fn record(&self, id: SourceId) -> Result<SourceRecord> {
        self.records()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or(ScanError::NoSuchSource(id))
    }

    /// Sources (folded ones are part of their outer source and not listed).
    pub fn sources(&self) -> Result<Vec<SourceInfo>> {
        Ok(self
            .records()?
            .iter()
            .map(SourceInfo::from_record)
            .collect())
    }

    /// Add a folder as a source (ADR-0006): records its root, volume identity, total size and a fingerprint
    /// of up to 64 sampled files. Reads the tree, never writes to it. A root inside an existing source is
    /// refused; existing sources inside the new root are folded into it.
    /// A registered export library, a folder inside one or a folder containing one is never a source: the
    /// library holds the copies PhotoSquirrel made, and the export refuses to write into a source.
    /// Libraries are found by the catalog's registry and on disk (`.photosquirrel/manifest.jsonl`), so one made by
    /// another catalog is refused too.
    fn refuse_export_library(&self, root: &Path) -> Result<()> {
        if let Some(library) = crate::walk::find_export_library(&self.options().fs, root) {
            return Err(ScanError::ExportLibrary {
                path: root.to_path_buf(),
                library,
            });
        }
        for lib in self.catalog.read().export_libraries()? {
            let library = path_from_bytes(&lib.root);
            if source::overlap(root, &library) != Overlap::Disjoint {
                return Err(ScanError::ExportLibrary {
                    path: root.to_path_buf(),
                    library,
                });
            }
        }
        Ok(())
    }

    pub fn add_source(&self, path: &Path) -> Result<AddedSource> {
        let meta = match self.options().fs.metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ScanError::SourceMissing(path.to_path_buf()));
            }
            Err(e) => {
                return Err(ScanError::Io {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        };
        if !meta.is_dir() {
            return Err(ScanError::NotAFolder(path.to_path_buf()));
        }
        let root = normalize_root(path).map_err(|e| ScanError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        self.refuse_export_library(&root)?;
        let mut fold = Vec::new();
        for s in self.records()? {
            // Roots stored before verbatim prefixes were dropped (Windows) still compare by their plain form.
            let existing = ps_model::path::simplify_canonical(path_from_bytes(&s.root));
            match source::overlap(&root, &existing) {
                Overlap::Same => {
                    return Err(ScanError::AlreadySource {
                        path: root,
                        id: s.id,
                    });
                }
                Overlap::Inside(_) => {
                    return Err(ScanError::AlreadyCovered {
                        path: root,
                        by: s.id,
                        by_root: existing,
                    });
                }
                Overlap::Contains(prefix) => fold.push((s.id, prefix)),
                Overlap::Disjoint => {}
            }
        }
        if let Some(a) = self.active_unfinished()
            && fold
                .iter()
                .any(|(id, _)| a.ctx.sources.iter().any(|s| s.id == *id))
        {
            return Err(ScanError::RunActive(a.run));
        }
        // Folding re-keys the inner source's files under the outer one: its cleanup runs could no longer find
        // the files they moved (ADR-0063).
        for (inner, _) in &fold {
            let files = self.catalog.read().quarantined_count(*inner)?;
            if files > 0 {
                return Err(ScanError::HasQuarantinedFiles {
                    id: *inner,
                    files,
                    what: "adding a folder that contains it",
                });
            }
        }
        let m = source::measure(&self.options().fs, &root).map_err(|e| walk_err(&root, e))?;
        let mut volume = m.volume.clone();
        if let Some(f) = &self.options().device_class {
            volume.class = f(&root);
        }
        let id = self.catalog.writer().add_source(NewSource {
            root: path_to_bytes(&root),
            volume_uuid: volume.uuid.clone(),
            device_hint: Some(volume.hint()),
            total_bytes: Some(m.total_bytes),
            fingerprint: Some(source::encode(&m.samples)),
        })?;
        for (inner, prefix) in &fold {
            self.catalog.writer().submit(Command::FoldSource {
                inner: *inner,
                outer: id,
                prefix: prefix.clone(),
            })?;
        }
        self.catalog.writer().flush()?;
        if !fold.is_empty() {
            // Folded files changed source and path: their grouping inputs changed.
            self.groups.refresh(&self.catalog)?;
        }
        Ok(AddedSource {
            source: SourceInfo::from_record(&self.record(id)?),
            folded: fold.into_iter().map(|(id, _)| id).collect(),
        })
    }

    /// Whether a source's root is reachable and is the same volume it was.
    fn online(&self, r: &SourceRecord) -> bool {
        let root = path_from_bytes(&r.root);
        if !self.options().fs.metadata(&root).is_ok_and(|m| m.is_dir()) {
            return false;
        }
        // An empty mount point left behind by an unplugged drive is not the drive.
        match (&r.volume_uuid, source::volume_info(&root).uuid) {
            (Some(want), Some(now)) => *want == now,
            _ => true,
        }
    }

    /// Re-check every source's root and record online/offline (ADR-0006). Offline sources keep their
    /// files, metadata and thumbnails.
    pub fn refresh_sources(&self) -> Result<Vec<SourceInfo>> {
        let mut changed = false;
        for r in self.records()? {
            let status = if self.online(&r) {
                SourceStatus::Online
            } else {
                SourceStatus::Offline
            };
            if status != r.status {
                self.catalog.writer().submit(Command::SetSourceStatus {
                    source: r.id,
                    status,
                })?;
                changed = true;
            }
        }
        if changed {
            self.catalog.writer().flush()?;
        }
        self.sources()
    }

    /// Re-link a source to a new root, confirmed only when the sampled content fingerprint matches
    /// (ADR-0006). The source keeps its id and history.
    pub fn relink_source(&self, id: SourceId, new_root: &Path) -> Result<SourceInfo> {
        let rec = self.record(id)?;
        if let Some(a) = self.active_unfinished()
            && a.ctx.sources.iter().any(|s| s.id == id)
        {
            return Err(ScanError::RunActive(a.run));
        }
        let samples = rec
            .fingerprint
            .as_deref()
            .map(source::decode)
            .unwrap_or_default();
        if samples.is_empty() {
            return Err(ScanError::NoFingerprint(id));
        }
        if !self
            .options()
            .fs
            .metadata(new_root)
            .is_ok_and(|m| m.is_dir())
        {
            return Err(ScanError::SourceMissing(new_root.to_path_buf()));
        }
        let root = normalize_root(new_root).map_err(|e| ScanError::Io {
            path: new_root.to_path_buf(),
            source: e,
        })?;
        self.refuse_export_library(&root)?;
        let wanted: std::collections::HashSet<[u8; 32]> =
            samples.iter().map(|s| s.path_hash).collect();
        let mut known: HashMap<[u8; 32], RelPath> = HashMap::new();
        let mut after: Option<RelPath> = None;
        loop {
            let page = self.catalog.read().source_files(id, after.as_ref(), 4096)?;
            for f in &page {
                let h = source::path_hash(&f.key.path);
                if wanted.contains(&h) {
                    known.insert(h, f.key.path.clone());
                }
            }
            if page.len() < 4096 {
                break;
            }
            after = page.last().map(|f| f.key.path.clone());
        }
        let check = source::check(&self.options().fs, &root, &samples, &known)
            .map_err(|e| walk_err(&root, e))?;
        if !check.accepted() {
            return Err(ScanError::FingerprintMismatch {
                path: root,
                mismatched: check.mismatched,
                samples: check.samples,
            });
        }
        let v = source::volume_info(&root);
        self.catalog.writer().submit(Command::RelinkSource {
            source: id,
            root: path_to_bytes(&root),
            volume_uuid: v.uuid.clone(),
            device_hint: Some(v.hint()),
        })?;
        self.catalog.writer().flush()?;
        Ok(SourceInfo::from_record(&self.record(id)?))
    }

    /// Remove a source's catalog records after explicit confirmation. Files on disk are never touched.
    pub fn remove_source(&self, id: SourceId, confirm: Confirmation) -> Result<()> {
        if confirm != Confirmation::Confirmed {
            return Err(ScanError::NotConfirmed);
        }
        self.record(id)?;
        if let Some(a) = self.active_unfinished()
            && a.ctx.sources.iter().any(|s| s.id == id)
        {
            return Err(ScanError::RunActive(a.run));
        }
        // Its cleanup runs must stay undoable (ADR-0063).
        let files = self.catalog.read().quarantined_count(id)?;
        if files > 0 {
            return Err(ScanError::HasQuarantinedFiles {
                id,
                files,
                what: "removing the source",
            });
        }
        self.catalog
            .writer()
            .submit(Command::RemoveSource { source: id })?;
        self.catalog.writer().flush()?;
        // The removed files leave their groups (nothing on disk changes).
        self.groups.refresh(&self.catalog)?;
        Ok(())
    }

    // ---- runs -------------------------------------------------------------------------------------------

    fn active_unfinished(&self) -> Option<Arc<ActiveRun>> {
        self.active.lock().clone().filter(|a| !a.finished())
    }

    /// The active (running or paused) run.
    pub fn active_run(&self) -> Option<ScanRunId> {
        self.active_unfinished().map(|a| a.run)
    }

    /// Start a scan of `sources` (all sources when empty). Refused while another run is active.
    pub fn start_scan(&self, sources: &[SourceId]) -> Result<ScanRunId> {
        self.start_scan_with(sources, false)
    }

    /// [`Scanner::start_scan`], with `refresh_analysis` also refreshing the analysis of unchanged files without
    /// stored class facts in this run (ADR-0071; as [`ScanOptions::refresh_analysis`] does for every run).
    pub fn start_scan_with(
        &self,
        sources: &[SourceId],
        refresh_analysis: bool,
    ) -> Result<ScanRunId> {
        let mut slot = self.active.lock();
        self.ensure_idle(&slot)?;
        let records = self.run_sources(sources)?;
        let ids: Vec<SourceId> = records.iter().map(|r| r.id).collect();
        let refresh = refresh_analysis || self.options().refresh_analysis;
        let run = self.catalog.writer().start_run_with(ids, refresh)?;
        *slot = Some(self.spawn(run, records, refresh)?);
        Ok(run)
    }

    /// The zone runs make instants local with (ADR-0070): the configured one, else the machine's.
    pub fn zone(&self) -> Arc<dyn ps_media::LocalZone> {
        zone_of(&self.opts.lock())
    }

    /// Resume a paused, cancelled or interrupted run (or a completed one that left sources incomplete):
    /// the sources are walked again and only unfinished files are processed (ADR-0008). A run paused in
    /// this process is simply un-paused.
    pub fn resume_run(&self, run: ScanRunId) -> Result<ScanRunId> {
        let mut slot = self.active.lock();
        if let Some(a) = slot.as_ref().filter(|a| !a.finished()) {
            if a.run == run {
                drop(slot);
                self.resume()?;
                return Ok(run);
            }
            return Err(ScanError::RunActive(a.run));
        }
        self.ensure_idle(&slot)?;
        let rec = self
            .catalog
            .read()
            .run(run)?
            .ok_or(ScanError::NoSuchRun(run))?;
        let incomplete =
            RunSummary::from_json(&rec.stats).is_some_and(|s| !s.incomplete_sources.is_empty());
        let resumable = matches!(
            rec.status,
            RunStatus::Paused | RunStatus::Cancelled | RunStatus::Interrupted
        ) || (rec.status == RunStatus::Completed && incomplete);
        if !resumable {
            return Err(ScanError::NotResumable {
                run,
                status: rec.status,
            });
        }
        let all = self.records()?;
        let records: Vec<SourceRecord> = all
            .into_iter()
            .filter(|s| rec.sources.contains(&s.id))
            .collect();
        if records.is_empty() {
            return Err(ScanError::NothingToScan);
        }
        self.catalog.writer().submit(Command::SetRunStatus {
            run,
            status: RunStatus::Running,
            stats: None,
        })?;
        self.catalog.writer().flush()?;
        // A refresh run resumes as a refresh run (ADR-0082).
        *slot = Some(self.spawn(run, records, rec.refresh_analysis)?);
        Ok(run)
    }

    fn ensure_idle(&self, slot: &Option<Arc<ActiveRun>>) -> Result<()> {
        if let Some(a) = slot.as_ref().filter(|a| !a.finished()) {
            return Err(ScanError::RunActive(a.run));
        }
        // Another process may be scanning this catalog.
        if let Some(r) = self
            .catalog
            .read()
            .runs()?
            .into_iter()
            .find(|r| r.status == RunStatus::Running)
        {
            return Err(ScanError::RunActive(r.id));
        }
        Ok(())
    }

    fn run_sources(&self, wanted: &[SourceId]) -> Result<Vec<SourceRecord>> {
        let all = self.records()?;
        let out: Vec<SourceRecord> = if wanted.is_empty() {
            all
        } else {
            let mut v = Vec::new();
            for id in wanted {
                v.push(
                    all.iter()
                        .find(|s| s.id == *id)
                        .cloned()
                        .ok_or(ScanError::NoSuchSource(*id))?,
                );
            }
            v
        };
        if out.is_empty() {
            return Err(ScanError::NothingToScan);
        }
        Ok(out)
    }

    fn spawn(
        &self,
        run: ScanRunId,
        records: Vec<SourceRecord>,
        refresh: bool,
    ) -> Result<Arc<ActiveRun>> {
        let opts = self.options();
        let mut sources = Vec::with_capacity(records.len());
        for r in &records {
            let root = path_from_bytes(&r.root);
            let online = self.online(r);
            if !online && r.status == SourceStatus::Online {
                self.catalog.writer().submit(Command::SetSourceStatus {
                    source: r.id,
                    status: SourceStatus::Offline,
                })?;
            } else if online && r.status == SourceStatus::Offline {
                self.catalog.writer().submit(Command::SetSourceStatus {
                    source: r.id,
                    status: SourceStatus::Online,
                })?;
            }
            let device = match &opts.device_class {
                Some(f) => f(&root),
                None => source::volume_info(&root).class,
            };
            sources.push(RunSource {
                id: r.id,
                root,
                device,
                offline: !online,
                lost: AtomicBool::new(false),
                symlinks_skipped: AtomicU64::new(0),
                walk_errors: AtomicU64::new(0),
                hidden_dirs_skipped: AtomicU64::new(0),
            });
        }
        let ids: Vec<SourceId> = sources.iter().map(|s| s.id).collect();
        let ctx = Arc::new(RunCtx {
            run,
            catalog: self.catalog.clone(),
            thumbs: self.thumbs.clone(),
            opts: opts.clone(),
            ctl: Arc::new(Control::new()),
            progress: Arc::new(Progress::new(run, &ids)),
            budget: ByteBudget::new(opts.memory_cap),
            decode_budget: ByteBudget::new(opts.decode_memory_cap),
            sources,
            changed_during_scan: AtomicU64::new(0),
            retries_exhausted: AtomicU64::new(0),
            groups: self.groups.clone(),
            classifier_hash: opts.classifier.stable_hash(),
            zone: zone_of(&opts),
            refresh_analysis: opts.refresh_analysis || refresh,
        });
        let thread = std::thread::Builder::new()
            .name(format!("ps-scan-run-{run}"))
            .spawn({
                let ctx = ctx.clone();
                move || execute(ctx)
            })
            .map_err(|e| ScanError::Thread(e.to_string()))?;
        Ok(Arc::new(ActiveRun {
            run,
            ctx,
            thread: Mutex::new(Some(thread)),
            summary: Mutex::new(None),
        }))
    }

    fn current(&self) -> Result<Arc<ActiveRun>> {
        self.active_unfinished().ok_or(ScanError::NoActiveRun)
    }

    /// Pause the active run: no new file starts after at most 250 ms; in-flight results are flushed.
    pub fn pause(&self) -> Result<ScanRunId> {
        let a = self.current()?;
        if a.ctx.ctl.state() == ControlState::Running {
            a.ctx.ctl.pause();
            a.ctx.progress.set_status(RunStatus::Paused);
            self.catalog.writer().submit(Command::SetRunStatus {
                run: a.run,
                status: RunStatus::Paused,
                stats: None,
            })?;
            self.catalog.writer().flush()?;
        }
        Ok(a.run)
    }

    /// Continue a paused run.
    pub fn resume(&self) -> Result<ScanRunId> {
        let a = self.current()?;
        if a.ctx.ctl.state() == ControlState::Paused {
            self.catalog.writer().submit(Command::SetRunStatus {
                run: a.run,
                status: RunStatus::Running,
                stats: None,
            })?;
            a.ctx.progress.set_status(RunStatus::Running);
            a.ctx.ctl.resume();
        }
        Ok(a.run)
    }

    /// Cancel the active run (returns at once; [`Scanner::wait`] returns once every thread stopped and the
    /// run is recorded `cancelled`).
    pub fn cancel(&self) -> Result<ScanRunId> {
        let a = self.current()?;
        a.ctx.ctl.cancel();
        Ok(a.run)
    }

    /// Wait at most `limit` for the current (or last) run to stop, without taking its summary: after
    /// [`Scanner::cancel`], so the run reads `cancelled` in the next snapshot. Whether it has stopped (`true` when
    /// there is no run).
    pub fn wait_stopped(&self, limit: Duration) -> bool {
        let Some(a) = self.active.lock().clone() else {
            return true;
        };
        let start = Instant::now();
        loop {
            if a.finished() {
                return true;
            }
            if start.elapsed() >= limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Block until the current (or last) run ends; its summary.
    pub fn wait(&self) -> Option<RunSummary> {
        let a = self.active.lock().clone()?;
        a.join()
    }

    /// The progress snapshot of the active (or last) run (ADR-0019).
    pub fn snapshot(&self) -> Option<ProgressSnapshot> {
        let a = self.active.lock().clone()?;
        Some(
            a.ctx
                .progress
                .snapshot(a.ctx.ctl.busy_count(), a.ctx.budget.in_flight()),
        )
    }

    /// Peak buffered bytes and peak buffers per reader of the active (or last) run (memory instrumentation).
    pub fn buffer_peaks(&self) -> Option<(u64, u64)> {
        let a = self.active.lock().clone()?;
        Some((a.ctx.budget.peak_bytes(), a.ctx.budget.peak_per_reader()))
    }

    /// All runs, oldest first.
    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        Ok(self.catalog.read().runs()?)
    }

    /// The stored summary of a run.
    pub fn summary(&self, run: ScanRunId) -> Result<Option<RunSummary>> {
        let rec = self
            .catalog
            .read()
            .run(run)?
            .ok_or(ScanError::NoSuchRun(run))?;
        Ok(RunSummary::from_json(&rec.stats))
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        if let Some(a) = self.active.get_mut().take() {
            a.ctx.ctl.cancel();
            a.join();
        }
    }
}

fn walk_err(root: &Path, e: WalkError) -> ScanError {
    match e {
        WalkError::Cancelled => ScanError::Thread("cancelled".into()),
        WalkError::RootUnavailable(source) if source.kind() == std::io::ErrorKind::NotFound => {
            ScanError::SourceMissing(root.to_path_buf())
        }
        WalkError::RootUnavailable(source) => ScanError::Io {
            path: root.to_path_buf(),
            source,
        },
    }
}

/// The configured zone, else the machine's (read through the OS once per call).
fn zone_of(opts: &ScanOptions) -> Arc<dyn ps_media::LocalZone> {
    opts.zone
        .clone()
        .unwrap_or_else(|| Arc::new(ps_media::date::os::OsZone))
}
