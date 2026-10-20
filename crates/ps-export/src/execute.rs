//! Run a confirmed plan (ADR-0022, ADR-0023): recovery of a crashed run, verified copies, adoption, sidecars,
//! manifest lines and ledger rows.
//!
//! Order per file: ledger `in_progress` (durable) -> partial copy, verify, rename -> manifest line -> ledger
//! `done`. A crash at any instant leaves at most one stale partial per worker with an `in_progress` row;
//! the next run removes it (recovery) and the next plan either copies the file again or, when the rename had
//! already happened, finds the identical file at its target and adopts it. So an interrupted export
//! resumes to a library holding every keeper exactly once.
//!
//! Errors of one file (hash mismatch, source changed, unreadable) abort that file and the export continues;
//! a full library volume stops the export (completed files stay recorded).

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot_free::Mutex;
use ps_model::{ContentId, ContentIdKind, FileId, LedgerEntry, LedgerKind, LedgerState, RelPath};
use serde::Serialize;

use crate::copy::{
    CopyConfig, CopyError, CopyHooks, CopyOutcome, CopyRequest, copy_verified, write_new_file,
};
use crate::input::ExportInput;
use crate::ledger::Ledger;
use crate::library::{LibraryRoot, full_hash_of, partial_of};
use crate::manifest::{Manifest, ManifestRecord};
use crate::plan::{Plan, PlanBlocker, PlannedAction, PlannedBundle, PlannedFile};
use crate::rename::RenameMethod;
use crate::xmp;

/// `std::sync::Mutex` without poisoning noise.
mod parking_lot_free {
    pub struct Mutex<T>(std::sync::Mutex<T>);

    impl<T> Mutex<T> {
        pub fn new(t: T) -> Self {
            Self(std::sync::Mutex::new(t))
        }

        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|e| e.into_inner())
        }

        pub fn into_inner(self) -> T {
            self.0.into_inner().unwrap_or_else(|e| e.into_inner())
        }
    }
}

/// Executor settings.
#[derive(Clone, Copy, Debug)]
pub struct ExecConfig {
    pub copy: CopyConfig,
    /// Bundles copied concurrently. 1 suits one HDD; 2-4 help SSD/NAS libraries.
    pub threads: usize,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            copy: CopyConfig::default(),
            threads: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FailedFile {
    pub file: Option<FileId>,
    pub target: RelPath,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "message")]
pub enum StopReason {
    DiskFull(String),
    Cancelled,
    Ledger(String),
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::DiskFull(m) => write!(f, "the library volume is full ({m})"),
            StopReason::Cancelled => f.write_str("cancelled"),
            StopReason::Ledger(m) => write!(f, "the export ledger could not be written ({m})"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    pub copied: u64,
    /// Files renamed into place whose post-rename verification failed (ADR-0040): kept in the library,
    /// ledger `suspect`, not counted as exported.
    pub suspect: Vec<FailedFile>,
    pub adopted: u64,
    pub bytes_copied: u64,
    pub sidecars: u64,
    pub stale_partials_removed: u64,
    pub failed: Vec<FailedFile>,
    pub stopped: Option<StopReason>,
    pub renames: BTreeMap<RenameMethod, u64>,
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("{0}")]
    Blocked(PlanBlocker),
    #[error("library {}: {source}", path.display())]
    Library {
        path: std::path::PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("export ledger: {0}")]
    Ledger(io::Error),
}

struct Shared<'a> {
    ledger: &'a mut dyn Ledger,
    manifest: Manifest,
    report: ExportReport,
}

struct Ctx<'a> {
    plan: &'a Plan,
    input: &'a ExportInput,
    lib: LibraryRoot,
    cfg: ExecConfig,
    hooks: &'a dyn CopyHooks,
    cancel: &'a AtomicBool,
    stop: AtomicBool,
    shared: Mutex<Shared<'a>>,
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Ctx<'_> {
    fn entry(&self, f: &PlannedFile, state: LedgerState, full: Option<[u8; 32]>) -> LedgerEntry {
        LedgerEntry {
            content: f.content,
            full_hash: full,
            file: f.file,
            library_rel_path: f.target.clone(),
            size: f.size,
            state,
            profile: self.plan.settings.profile,
            at_ms: crate::now_ms(),
            library_mtime_ns: None,
            kind: LedgerKind::File,
        }
    }

    /// The modification time of a library file now (recorded with `done` rows, ADR-0044).
    fn library_mtime(&self, rel: &RelPath) -> Option<i64> {
        std::fs::symlink_metadata(self.lib.path(rel))
            .ok()
            .filter(|m| m.file_type().is_file())
            .and_then(|m| crate::library::mtime_ns_of(&m))
    }

    fn halt(&self, s: &mut Shared<'_>, why: StopReason) {
        self.stop.store(true, Ordering::SeqCst);
        if s.report.stopped.is_none() {
            s.report.stopped = Some(why);
        }
    }

    fn record(&self, e: &LedgerEntry) -> bool {
        let mut s = self.shared.lock();
        match s.ledger.record(e) {
            Ok(()) => true,
            Err(err) => {
                self.halt(&mut s, StopReason::Ledger(err.to_string()));
                false
            }
        }
    }

    /// Source path of a copy of `unit` (the copy a borrowed filing date came from).
    fn copy_path(&self, unit: usize, file: FileId) -> Option<String> {
        let c = self.input.units[unit]
            .copies
            .iter()
            .find(|c| c.file == file)?;
        self.input
            .source_path(c.source, &c.rel_path)
            .map(|p| path_string(&p))
    }

    fn source_paths(&self, unit: usize) -> Vec<String> {
        let u = &self.input.units[unit];
        let mut v: Vec<String> = u
            .copies
            .iter()
            .filter_map(|c| self.input.source_path(c.source, &c.rel_path))
            .map(|p| path_string(&p))
            .collect();
        for f in &u.files {
            if let Some(p) = self.input.source_path(f.source, &f.rel_path) {
                let p = path_string(&p);
                if !v.contains(&p) {
                    v.push(p);
                }
            }
        }
        v
    }

    /// The full hash of the file at an adoption target if it still is the planned content (regular file, size,
    /// hash; the sampled identity too for sampled contents), else `None`.
    fn reverify_adopt(&self, f: &PlannedFile) -> std::io::Result<Option<[u8; 32]>> {
        let path = self.lib.path(&f.target);
        let m = std::fs::symlink_metadata(&path)?;
        if !m.file_type().is_file() || m.len() != f.size {
            return Ok(None);
        }
        let (full, n) = full_hash_of(std::fs::File::open(&path)?)?;
        let same = n == f.size
            && match f.content.kind {
                ContentIdKind::Full => full == f.content.digest,
                ContentIdKind::Sampled => {
                    ps_hash::identity_from_reader(std::fs::File::open(&path)?, f.size)? == f.content
                }
            };
        Ok(same.then_some(full))
    }

    /// Manifest line then ledger `done`.
    fn finish(&self, f: &PlannedFile, full: [u8; 32], adopted: bool, sidecar: Option<&RelPath>) {
        let rec = ManifestRecord {
            library_path: f.target.display().into_owned(),
            full_hash: ContentId::full(full).to_hex(),
            content_id: f.content.to_hex(),
            identity_kind: match f.content.kind {
                ContentIdKind::Full => "full".into(),
                ContentIdKind::Sampled => "sampled".into(),
            },
            size: f.size,
            file_id: f.file.0,
            source_path: path_string(&f.source_path),
            source_paths: self.source_paths(f.unit),
            keeper_reasons: self.input.units[f.unit].keeper_reasons.clone(),
            profile: self.plan.settings.profile,
            at_ms: crate::now_ms(),
            adopted,
            sidecar: sidecar.map(|s| s.display().into_owned()),
            date_source: Some(f.date_origin.describe()),
            date_from: f.date_origin.copy.and_then(|c| self.copy_path(f.unit, c)),
        };
        let mtime = self.library_mtime(&f.target);
        let mut s = self.shared.lock();
        // An adoption after a crash between this file's manifest line and its `done` row would repeat the
        // line: the manifest already records this path with these bytes, so it is not appended again.
        let recorded = adopted && s.manifest.contains(&rec.library_path, &rec.full_hash);
        if recorded {
            log::info!("{} is already in the manifest", f.target);
        } else if let Err(e) = s.manifest.append(&rec) {
            // The copy is in place and verified; a manifest failure must not lose the ledger row.
            log::error!("manifest append for {}: {e}", f.target);
            s.report.failed.push(FailedFile {
                file: Some(f.file),
                target: crate::manifest::Manifest::rel_path(),
                error: format!("manifest: {e}"),
            });
        }
        let mut e = self.entry(f, LedgerState::Done, Some(full));
        e.library_mtime_ns = mtime;
        if let Err(err) = s.ledger.record(&e) {
            self.halt(&mut s, StopReason::Ledger(err.to_string()));
        }
    }

    fn fail(&self, f: Option<&PlannedFile>, target: &RelPath, err: &CopyError) {
        if let Some(f) = f {
            self.record(&self.entry(f, LedgerState::Failed, None));
        }
        let mut s = self.shared.lock();
        log::warn!("export of {target} aborted: {err}");
        if err.is_disk_full() {
            self.halt(&mut s, StopReason::DiskFull(err.to_string()));
        }
        s.report.failed.push(FailedFile {
            file: f.map(|f| f.file),
            target: target.clone(),
            error: err.to_string(),
        });
    }

    fn stopped(&self) -> bool {
        if self.cancel.load(Ordering::Relaxed) && !self.stop.load(Ordering::Relaxed) {
            let mut s = self.shared.lock();
            self.halt(&mut s, StopReason::Cancelled);
        }
        self.stop.load(Ordering::Relaxed)
    }

    fn run_bundle(&self, b: &PlannedBundle) {
        let mut done: Vec<(&PlannedFile, [u8; 32])> = Vec::new();
        for f in &b.files {
            if self.stopped() {
                return;
            }
            match f.action {
                PlannedAction::Adopt => {
                    // Re-verify now: the plan saw an identical file, but time passed since (the plan may
                    // have been shown for minutes); only a file that is still this content is recorded.
                    match self.reverify_adopt(f) {
                        Ok(None) => {
                            let msg = "the identical file found at this path while planning has changed \
                                       since; nothing was recorded (export again to plan it anew)";
                            log::warn!("adoption of {}: {msg}", f.target);
                            self.record(&self.entry(f, LedgerState::Failed, None));
                            self.shared.lock().report.failed.push(FailedFile {
                                file: Some(f.file),
                                target: f.target.clone(),
                                error: msg.into(),
                            });
                        }
                        Ok(Some(full)) => {
                            self.finish(f, full, true, b.sidecar.as_ref());
                            self.shared.lock().report.adopted += 1;
                            done.push((f, full));
                        }
                        Err(e) => self.fail(
                            Some(f),
                            &f.target,
                            &CopyError::Io {
                                op: "re-verify",
                                path: self.lib.path(&f.target),
                                source: e,
                            },
                        ),
                    }
                }
                PlannedAction::Copy => {
                    if !self.record(&self.entry(f, LedgerState::InProgress, None)) {
                        return;
                    }
                    let req = CopyRequest {
                        file: f.file,
                        source: &f.source_path,
                        target: &f.target,
                        content: f.content,
                        size: f.size,
                        mtime_ns: f.mtime_ns,
                    };
                    match copy_verified(&self.lib, &req, &self.cfg.copy, self.hooks) {
                        Ok(CopyOutcome {
                            suspect: Some((found, n)),
                            ..
                        }) => {
                            self.record(&self.entry(f, LedgerState::Suspect, Some(found)));
                            let msg = format!(
                                "the file at this library path changed right after it was copied \
                                 ({n} bytes, BLAKE3 {}); it was kept and marked suspect, and the photo \
                                 will be copied again under a free name",
                                ContentId::full(found).to_hex()
                            );
                            log::error!("export of {}: {msg}", f.target);
                            self.shared.lock().report.suspect.push(FailedFile {
                                file: Some(f.file),
                                target: f.target.clone(),
                                error: msg,
                            });
                        }
                        Ok(o) => {
                            self.finish(f, o.full_hash, false, b.sidecar.as_ref());
                            let mut s = self.shared.lock();
                            s.report.copied += 1;
                            s.report.bytes_copied += o.bytes;
                            *s.report.renames.entry(o.rename).or_default() += 1;
                            done.push((f, o.full_hash));
                        }
                        Err(e) => self.fail(Some(f), &f.target, &e),
                    }
                }
            }
        }
        // The sidecar lists the files copied now and those already in the library (a sidecar-only bundle).
        let mut listed: Vec<SideFile> = done
            .iter()
            .map(|(f, h)| SideFile {
                target: f.target.clone(),
                content: f.content,
                file: f.file,
                full: *h,
            })
            .collect();
        listed.extend(b.already.iter().map(|a| SideFile {
            target: a.library_rel_path.clone(),
            content: a.content,
            file: a.file,
            full: a.full_hash.unwrap_or(match a.content.kind {
                ContentIdKind::Full => a.content.digest,
                ContentIdKind::Sampled => [0; 32],
            }),
        }));
        if let (Some(rel), false) = (&b.sidecar, listed.is_empty())
            && !self.stopped()
        {
            self.write_sidecar(b, rel, &listed);
        }
    }

    fn sidecar_entry(&self, primary: &SideFile, rel: &RelPath, state: LedgerState) -> LedgerEntry {
        LedgerEntry {
            content: primary.content,
            full_hash: None,
            file: primary.file,
            library_rel_path: rel.clone(),
            size: 0,
            state,
            profile: self.plan.settings.profile,
            at_ms: crate::now_ms(),
            library_mtime_ns: None,
            kind: LedgerKind::Sidecar,
        }
    }

    /// Write the bundle's sidecar with a ledger row around it (ADR-0045): `in_progress` before its partial
    /// file exists (so recovery removes a crashed write's partial), `done` once it is in place.
    fn write_sidecar(&self, b: &PlannedBundle, rel: &RelPath, done: &[SideFile]) {
        let units: Vec<_> = b.units.iter().map(|&u| &self.input.units[u]).collect();
        let (subjects, description, people, pets) = xmp::subjects_of(&units);
        let mut sources = Vec::new();
        for &u in &b.units {
            for p in self.source_paths(u) {
                if !sources.contains(&p) {
                    sources.push(p);
                }
            }
        }
        let primary = &done[0];
        let full = primary.full;
        let side = xmp::Sidecar {
            subjects,
            description,
            content_hash: primary.content.to_hex(),
            identity_kind: match primary.content.kind {
                ContentIdKind::Full => "full".into(),
                ContentIdKind::Sampled => "sampled".into(),
            },
            full_hash: Some(ContentId::full(full).to_hex()),
            files: done
                .iter()
                .map(|f| {
                    format!(
                        "{} {}",
                        String::from_utf8_lossy(f.target.file_name()),
                        ContentId::full(f.full).to_hex()
                    )
                })
                .collect(),
            sources,
            people,
            pets,
        };
        let bytes = xmp::render(&side);
        if let Err(e) = xmp::parse(&bytes) {
            self.shared.lock().report.failed.push(FailedFile {
                file: None,
                target: rel.clone(),
                error: e.to_string(),
            });
            return;
        }
        if !self.record(&self.sidecar_entry(primary, rel, LedgerState::InProgress)) {
            return;
        }
        match write_new_file(
            &self.lib,
            rel,
            &bytes,
            &self.cfg.copy,
            self.hooks,
            primary.file,
        ) {
            Ok(m) => {
                let mut e = self.sidecar_entry(primary, rel, LedgerState::Done);
                e.full_hash = Some(*blake3::hash(&bytes).as_bytes());
                e.size = bytes.len() as u64;
                e.library_mtime_ns = self.library_mtime(rel);
                self.record(&e);
                let mut s = self.shared.lock();
                s.report.sidecars += 1;
                *s.report.renames.entry(m).or_default() += 1;
            }
            // A sidecar that appeared meanwhile is not ours: it is left as it is.
            Err(CopyError::TargetExists { .. }) => {
                self.record(&self.sidecar_entry(primary, rel, LedgerState::Failed));
            }
            Err(e) => {
                self.record(&self.sidecar_entry(primary, rel, LedgerState::Failed));
                self.fail(None, rel, &e);
            }
        }
    }
}

/// A file a sidecar lists.
struct SideFile {
    target: RelPath,
    content: ContentId,
    file: FileId,
    full: [u8; 32],
}

/// Remove the partial files of `in_progress` rows (a crashed run) and mark those rows failed. Returns the
/// number of partial files removed. Only `*.ps-partial` regular files inside the library are ever removed.
pub fn recover(lib: &LibraryRoot, ledger: &mut dyn Ledger) -> Result<u64, ExecError> {
    let mut removed = 0;
    for mut e in ledger.entries().map_err(ExecError::Ledger)? {
        if e.state != LedgerState::InProgress {
            continue;
        }
        let partial = partial_of(&e.library_rel_path);
        match lib.remove_partial(&partial) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(source) => {
                return Err(ExecError::Library {
                    path: lib.path(&partial),
                    source,
                });
            }
        }
        e.state = LedgerState::Failed;
        e.at_ms = crate::now_ms();
        ledger.record(&e).map_err(ExecError::Ledger)?;
    }
    Ok(removed)
}

/// Run a confirmed plan. Refuses a blocked plan (insufficient space). `cancel` stops after the current files.
pub fn execute(
    plan: &Plan,
    input: &ExportInput,
    ledger: &mut dyn Ledger,
    cfg: &ExecConfig,
    hooks: &dyn CopyHooks,
    cancel: &AtomicBool,
) -> Result<ExportReport, ExecError> {
    let t0 = Instant::now();
    if let Some(b) = plan.blocker() {
        return Err(ExecError::Blocked(b));
    }
    let lib_err = |source| ExecError::Library {
        path: plan.library_root.clone(),
        source,
    };
    let lib = LibraryRoot::open_or_create(&plan.library_root).map_err(lib_err)?;
    let removed = recover(&lib, ledger)?;
    let manifest = Manifest::open(&lib).map_err(lib_err)?;
    let ctx = Ctx {
        plan,
        input,
        lib,
        cfg: *cfg,
        hooks,
        cancel,
        stop: AtomicBool::new(false),
        shared: Mutex::new(Shared {
            ledger,
            manifest,
            report: ExportReport {
                stale_partials_removed: removed,
                ..ExportReport::default()
            },
        }),
    };
    let threads = cfg.threads.clamp(1, 16).min(plan.bundles.len().max(1));
    if threads == 1 {
        for b in &plan.bundles {
            if ctx.stopped() {
                break;
            }
            ctx.run_bundle(b);
        }
    } else {
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= plan.bundles.len() || ctx.stopped() {
                            break;
                        }
                        ctx.run_bundle(&plan.bundles[i]);
                    }
                });
            }
        });
    }
    let mut report = ctx.shared.into_inner().report;
    report.elapsed = t0.elapsed();
    Ok(report)
}
