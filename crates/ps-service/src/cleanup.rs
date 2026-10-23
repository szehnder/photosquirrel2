//! Reversible quarantine cleanup (change `quarantine-cleanup`; ADR-0004, ADR-0023, ADR-0024, ADR-0034,
//! ADR-0061).
//!
//! [`Cleanups`] plans a cleanup of one source (or one folder of it) from the derived dispositions, stores the
//! plan in the catalog (`cleanup_run`, `cleanup_move`), runs a confirmed plan through
//! [`ps_export::quarantine::run`] with the catalog as the durable half of the journal ([`CatalogJournal`]),
//! undoes whole runs or a folder or files of one, and reconciles moves a crash left mid-way. Every command
//! reconciles first, so a restart after a crash always resolves the journal before anything else happens.
//!
//! Each command holds the service's operation slot ([`crate::ops`], ADR-0065): no scan, export or other cleanup
//! command runs meanwhile, and reconciliation never touches a run that is live in this process. A run is tied
//! to its source by identity (id and `added_at`, ADR-0063): when that source is gone the run acts on the root
//! stored with it, never on whatever source has its id now.
//!
//! Nothing here touches a source tree directly: only `ps-export::quarantine` renames files, and only after
//! re-verifying them. Nothing is ever deleted.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use ps_catalog::{Catalog, NewCleanupRun};
use ps_export::quarantine::{
    self, CleanupJournal, NoQuarantineHooks, QuarantineHooks, RunConfig, Stat,
};
use ps_model::{
    CleanupMove, CleanupRunRecord, CleanupRunState, DeviceClass, FileId, MoveState, RelPath,
    SourceId, SourceStatus, StatFingerprint, path_from_bytes,
};

use crate::export::ExportError;

use crate::scan::ScanService;
pub use ps_export::quarantine::{
    FolderTotals, MoveTotals, ReconcileReport, RunProgress, RunReport, SkipReason, Skipped,
    UndoNote, UndoReport,
};

/// Errors of the cleanup facade. None of them leaves a file moved without its journal entries.
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error(transparent)]
    Catalog(#[from] ps_catalog::CatalogError),
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error(transparent)]
    Scan(#[from] crate::scan::ScanError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("unknown source {0}")]
    UnknownSource(i64),
    #[error("unknown cleanup run {0}")]
    UnknownRun(i64),
    #[error("source {id} ({}) is offline: connect it first", root.display())]
    SourceOffline { id: i64, root: PathBuf },
    #[error("the export library {} is not available: connect it first", .0.display())]
    LibraryOffline(PathBuf),
    #[error("no export library is registered yet: export first")]
    NoLibrary,
    #[error("a scan is running: wait for it to finish (or cancel it) before cleaning up")]
    ScanRunning,
    #[error("{0} is running: wait for it to finish first")]
    Busy(crate::ops::Operation),
    #[error(
        "source {0} is a network share, where a rename may not be atomic: confirm cleanup on a network source"
    )]
    NetworkNotConfirmed(i64),
    #[error("cleanup run {id} cannot run: {why}")]
    NotRunnable { id: i64, why: String },
    #[error("the catalog is open read-only (another PhotoSquirrel process owns it)")]
    ReadOnly,
}

pub type Result<T, E = CleanupError> = std::result::Result<T, E>;

/// The catalog half of the journal for one run: each record is committed before it returns.
#[derive(Debug, Clone)]
pub struct CatalogJournal {
    catalog: Arc<Catalog>,
    run: i64,
}

impl CleanupJournal for CatalogJournal {
    fn record(&mut self, file: FileId, state: MoveState, note: Option<String>) -> io::Result<()> {
        self.catalog
            .writer()
            .cleanup_set_move(self.run, file, state, note)
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

/// Counts of a run's moves.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MoveCounts {
    pub planned: u64,
    pub quarantined: u64,
    pub skipped: u64,
    pub restored: u64,
    /// Moving or restoring (only between a crash and its reconciliation).
    pub in_flight: u64,
    /// In the plan but left out by the selection (ADR-0068).
    pub unselected: u64,
    pub planned_bytes: u64,
    pub quarantined_bytes: u64,
    pub unselected_bytes: u64,
}

impl MoveCounts {
    pub fn of(moves: &[CleanupMove]) -> Self {
        let mut c = MoveCounts::default();
        for m in moves {
            match m.state {
                MoveState::Planned => {
                    c.planned += 1;
                    c.planned_bytes += m.size;
                }
                MoveState::Quarantined => {
                    c.quarantined += 1;
                    c.quarantined_bytes += m.size;
                }
                MoveState::Skipped => c.skipped += 1,
                MoveState::Restored => c.restored += 1,
                MoveState::Unselected => {
                    c.unselected += 1;
                    c.unselected_bytes += m.size;
                }
                MoveState::Moving | MoveState::Restoring => c.in_flight += 1,
            }
        }
        c
    }
}

/// One run for lists and history.
#[derive(Clone, Debug, Serialize)]
pub struct RunSummary {
    pub run: CleanupRunRecord,
    pub root: PathBuf,
    pub library: Option<PathBuf>,
    /// `<root>/.photosquirrel-quarantine/<run id>`.
    pub quarantine_dir: PathBuf,
    pub counts: MoveCounts,
    pub network: bool,
}

/// A stored plan: the run, every move with its quarantine path, and totals per folder.
#[derive(Clone, Debug, Serialize)]
pub struct CleanupPlan {
    /// `None`: nothing in the source (or folder) is eligible, so no run was stored.
    pub summary: Option<RunSummary>,
    pub source: SourceId,
    pub root: PathBuf,
    pub library: PathBuf,
    pub folder: Option<RelPath>,
    pub moves: Vec<CleanupMove>,
    pub totals: MoveTotals,
    /// The source is a network share: running needs [`RunOptions::network_confirmed`].
    pub network: bool,
}

/// Totals per folder of some moves ([`ps_export::quarantine::totals`]).
pub fn totals_of<'m>(moves: impl IntoIterator<Item = &'m CleanupMove>) -> MoveTotals {
    quarantine::totals(moves)
}

/// Options of [`Cleanups::run`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RunOptions {
    /// The user confirmed cleanup on a network source (renames there may not be atomic).
    pub network_confirmed: bool,
}

/// What to undo of a run.
#[derive(Clone, Debug, Default)]
pub struct UndoSelection {
    /// Only moves under this folder.
    pub folder: Option<RelPath>,
    /// Only these files (empty: every file).
    pub files: Vec<FileId>,
}

impl UndoSelection {
    pub fn all() -> Self {
        Self::default()
    }

    fn picks(&self, m: &CleanupMove) -> bool {
        self.folder.as_ref().is_none_or(|f| m.rel_path.is_within(f))
            && (self.files.is_empty() || self.files.contains(&m.file))
    }
}

/// A source as a run sees it now.
#[derive(Clone, Debug)]
struct CurrentSource {
    root: PathBuf,
    online: bool,
    network: bool,
    added_at_ms: i64,
}

/// The cleanup facade of one [`ScanService`].
#[derive(Clone, Copy, Debug)]
pub struct Cleanups<'a> {
    svc: &'a ScanService,
}

impl ScanService {
    /// Cleanup planning, runs, undo and history.
    pub fn cleanups(&self) -> Cleanups<'_> {
        Cleanups { svc: self }
    }
}

fn uid() -> String {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{ns:x}-{:x}", std::process::id())
}

impl<'a> Cleanups<'a> {
    fn catalog(&self) -> &'a Arc<Catalog> {
        self.svc.engine().catalog()
    }

    fn writable(&self) -> Result<()> {
        if self.catalog().is_observer() {
            Err(CleanupError::ReadOnly)
        } else {
            Ok(())
        }
    }

    /// Take the operation slot ([`crate::ops`]).
    fn begin(&self, op: crate::ops::Operation) -> Result<crate::ops::OpGuard<'a>> {
        self.svc.begin(op).map_err(|b| match b {
            crate::ops::Busy::Scan(_) => CleanupError::ScanRunning,
            crate::ops::Busy::Operation(o) => CleanupError::Busy(o),
        })
    }

    /// The current root of a source, whether it is online, whether it is a network share and when it was
    /// added (`None` when the source was removed or folded into another).
    fn source(&self, id: SourceId) -> Result<Option<CurrentSource>> {
        let folded = self
            .catalog()
            .read()
            .sources()?
            .into_iter()
            .any(|s| s.id == id && s.folded_into.is_some());
        if folded {
            return Ok(None);
        }
        Ok(self
            .svc
            .sources()?
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| CurrentSource {
                online: s.status == SourceStatus::Online && s.root.is_dir(),
                network: s.device == DeviceClass::Network,
                added_at_ms: s.added_at_ms,
                root: s.root,
            }))
    }

    /// The run's own source, if it is still registered: the source with the run's id *and* `added_at`. A
    /// source that only has the id (the run's source was removed and the id handed out again) is not it.
    fn run_source(&self, run: &CleanupRunRecord) -> Result<Option<CurrentSource>> {
        Ok(self
            .source(run.source)?
            .filter(|s| run.source_added_at_ms == Some(s.added_at_ms)))
    }

    fn library_root(&self, id: i64) -> Result<Option<PathBuf>> {
        Ok(self
            .catalog()
            .read()
            .export_libraries()?
            .into_iter()
            .find(|l| l.id == id)
            .map(|l| path_from_bytes(&l.root)))
    }

    /// The root a run acts on: its own source's current root (a re-linked source keeps its files), else the
    /// root stored with the run (its source was removed, folded or replaced by another with the same id).
    fn run_root(&self, run: &CleanupRunRecord) -> Result<(PathBuf, bool)> {
        Ok(match self.run_source(run)? {
            Some(s) => (s.root, s.network),
            None => (path_from_bytes(&run.root), false),
        })
    }

    fn summary(&self, run: CleanupRunRecord, moves: &[CleanupMove]) -> Result<RunSummary> {
        let (root, network) = self.run_root(&run)?;
        Ok(RunSummary {
            quarantine_dir: quarantine::run_dir_rel(run.id).under(&root),
            library: self.library_root(run.library_id)?,
            counts: MoveCounts::of(moves),
            root,
            network,
            run,
        })
    }

    /// Every run, newest first.
    pub fn runs(&self) -> Result<Vec<RunSummary>> {
        let r = self.catalog().read();
        r.cleanup_runs()?
            .into_iter()
            .map(|run| {
                let moves = r.cleanup_moves(run.id)?;
                self.summary(run, &moves)
            })
            .collect()
    }

    /// One run and its moves (by path).
    pub fn run_detail(&self, id: i64) -> Result<(RunSummary, Vec<CleanupMove>)> {
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        let moves = r.cleanup_moves(id)?;
        Ok((self.summary(run, &moves)?, moves))
    }

    /// Plan a cleanup of `source` (or of `folder` in it) against `library` (default: the last library
    /// exported to). Stores the plan as a `planned` run and returns it; nothing on disk is touched (the
    /// dispositions are checked without re-hashing; the run re-verifies every file).
    pub fn plan(
        &self,
        source: SourceId,
        folder: Option<RelPath>,
        library: Option<&Path>,
    ) -> Result<CleanupPlan> {
        self.writable()?;
        let _op = self.begin(crate::ops::Operation::CleanupPlan)?;
        self.reconcile_locked()?;
        let CurrentSource {
            root,
            online,
            network,
            added_at_ms,
        } = self
            .source(source)?
            .ok_or(CleanupError::UnknownSource(source.0))?;
        if !online {
            return Err(CleanupError::SourceOffline { id: source.0, root });
        }
        let ex = self.svc.exports();
        let library = match library {
            Some(l) => ps_export::library::normalize(l).map_err(CleanupError::Io)?,
            None => ex.last_library()?.ok_or(CleanupError::NoLibrary)?,
        };
        let (files, dispositions, _) = ex.dispositions(&library, false)?;
        let mut stats: HashMap<FileId, StatFingerprint> = HashMap::new();
        self.catalog().read().for_each_file(|f| {
            if f.key.source == source {
                stats.insert(f.id, f.stat);
            }
        })?;
        let rows: Vec<_> = files
            .into_iter()
            .zip(dispositions)
            .filter(|(f, d)| f.source == source && quarantine::eligible(f, d))
            .filter_map(|(f, d)| {
                let s = stats.get(&f.file)?;
                Some((
                    f,
                    d,
                    Stat {
                        size: s.size,
                        mtime_ns: s.mtime_ns,
                        file_index: s.file_index,
                    },
                ))
            })
            .collect();
        let preview =
            quarantine::plan_moves(0, folder.as_ref(), rows.iter().map(|(f, d, s)| (f, d, *s)));
        if preview.is_empty() {
            return Ok(CleanupPlan {
                summary: None,
                source,
                root,
                library,
                folder,
                moves: Vec::new(),
                totals: MoveTotals::default(),
                network,
            });
        }
        let library_id = ex.library_id(&library)?;
        let f2 = folder.clone();
        let id = self.catalog().writer().cleanup_create_run(NewCleanupRun {
            source,
            source_added_at_ms: added_at_ms,
            root: ps_model::path_to_bytes(&root),
            library_id,
            folder: folder.clone(),
            uid: uid(),
            moves: Box::new(move |id| {
                quarantine::plan_moves(id, f2.as_ref(), rows.iter().map(|(f, d, s)| (f, d, *s)))
            }),
        })?;
        let (summary, moves) = self.run_detail(id)?;
        Ok(CleanupPlan {
            totals: quarantine::totals(&moves),
            summary: Some(summary),
            source,
            root,
            library,
            folder,
            moves,
            network,
        })
    }

    fn config<'c>(
        run: &'c CleanupRunRecord,
        root: &'c Path,
        library: &'c Path,
        ledger: &'c [ps_model::LedgerEntry],
    ) -> RunConfig<'c> {
        RunConfig {
            run: run.id,
            uid: &run.uid,
            source_root: root,
            library_root: library,
            ledger,
        }
    }

    /// Reconcile every run an interruption may have left out of step with the file system: moves `moving` or
    /// `restoring`, runs `running`, and every run with a `journal.jsonl` (whose records can be newer than the
    /// catalog's after a power loss). Skipped (empty result) while a scan, an export or another cleanup
    /// command runs in this process: a live run is never reconciled; the next command does it.
    pub fn reconcile(&self) -> Result<Vec<(i64, ReconcileReport)>> {
        if self.catalog().is_observer() {
            return Ok(Vec::new());
        }
        let Ok(_op) = self.begin(crate::ops::Operation::CleanupReconcile) else {
            return Ok(Vec::new());
        };
        self.reconcile_locked()
    }

    /// [`Cleanups::reconcile`] while the caller holds the operation slot.
    fn reconcile_locked(&self) -> Result<Vec<(i64, ReconcileReport)>> {
        let r = self.catalog().read();
        let interrupted: std::collections::BTreeSet<i64> =
            r.cleanup_runs_to_reconcile()?.into_iter().collect();
        let mut ids = interrupted.clone();
        for run in r.cleanup_runs()? {
            if run.state == CleanupRunState::Undone || ids.contains(&run.id) {
                continue;
            }
            let (root, _) = self.run_root(&run)?;
            let journal = quarantine::run_dir_rel(run.id)
                .under(&root)
                .join(quarantine::JOURNAL_FILE);
            if journal.is_file() {
                ids.insert(run.id);
            }
        }
        let mut out = Vec::new();
        for id in ids {
            let report = self.reconcile_run(id)?;
            if !report.resolved.is_empty() || interrupted.contains(&id) {
                out.push((id, report));
            }
        }
        Ok(out)
    }

    fn reconcile_run(&self, id: i64) -> Result<ReconcileReport> {
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        let moves = r.cleanup_moves(id)?;
        let (root, _) = self.run_root(&run)?;
        if !root.is_dir() {
            // The drive is not connected: its files cannot be found now, so nothing is decided until it is.
            log::info!(
                "cleanup run {id}: {} is offline; reconciled when it is back",
                root.display()
            );
            return Ok(ReconcileReport::default());
        }
        let mut journal = CatalogJournal {
            catalog: self.catalog().clone(),
            run: id,
        };
        let report = quarantine::reconcile(
            &Self::config(&run, &root, Path::new(""), &[]),
            &moves,
            &mut journal,
        )?;
        if run.state == CleanupRunState::Running {
            self.catalog()
                .writer()
                .cleanup_set_run_state(id, CleanupRunState::Completed)?;
        }
        Ok(report)
    }

    /// Run a planned cleanup: re-verify and quarantine every move still planned. `cancel` stops between
    /// files (run again to continue). Requires the source and the library to be online and no scan running.
    pub fn run(
        &self,
        id: i64,
        opts: RunOptions,
        cancel: &AtomicBool,
        progress: &RunProgress,
    ) -> Result<RunReport> {
        self.run_with_hooks(id, opts, &NoQuarantineHooks, cancel, progress)
    }

    /// [`Cleanups::run`] with fault-injection hooks (tests).
    pub fn run_with_hooks(
        &self,
        id: i64,
        opts: RunOptions,
        hooks: &dyn QuarantineHooks,
        cancel: &AtomicBool,
        progress: &RunProgress,
    ) -> Result<RunReport> {
        self.writable()?;
        let _op = self.begin(crate::ops::Operation::CleanupRun(id))?;
        self.reconcile_locked()?;
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        let moves = r.cleanup_moves(id)?;
        let not_runnable = |why: &str| CleanupError::NotRunnable {
            id,
            why: why.to_owned(),
        };
        if run.state == CleanupRunState::Undone {
            return Err(not_runnable("it was undone"));
        }
        if !moves.iter().any(|m| m.state == MoveState::Planned) {
            return Err(not_runnable("no planned file is left to move"));
        }
        let CurrentSource {
            root,
            online,
            network,
            ..
        } = self.run_source(&run)?.ok_or_else(|| {
            not_runnable(&format!(
                "its source {} ({}) was removed since it was planned",
                run.source,
                path_from_bytes(&run.root).display()
            ))
        })?;
        if !online {
            return Err(CleanupError::SourceOffline {
                id: run.source.0,
                root,
            });
        }
        if network && !opts.network_confirmed {
            return Err(CleanupError::NetworkNotConfirmed(run.source.0));
        }
        let library = self
            .library_root(run.library_id)?
            .ok_or(CleanupError::NoLibrary)?;
        if !library.is_dir() {
            return Err(CleanupError::LibraryOffline(library));
        }
        let ledger = r.ledger(run.library_id)?;
        self.catalog()
            .writer()
            .cleanup_set_run_state(id, CleanupRunState::Running)?;
        let mut journal = CatalogJournal {
            catalog: self.catalog().clone(),
            run: id,
        };
        let result = quarantine::run(
            &Self::config(&run, &root, &library, &ledger),
            &moves,
            &mut journal,
            hooks,
            cancel,
            progress,
        );
        let state = match &result {
            Ok(rep) if rep.cancelled => CleanupRunState::Planned,
            _ => CleanupRunState::Completed,
        };
        self.catalog().writer().cleanup_set_run_state(id, state)?;
        Ok(result?)
    }

    /// Undo a run, or the part `selection` picks: rename quarantined files back to their paths, never onto
    /// an occupied path (reported, left in quarantine). The run is `undone` once none of its files is in
    /// quarantine any more.
    pub fn undo(&self, id: i64, selection: &UndoSelection) -> Result<UndoReport> {
        self.writable()?;
        let _op = self.begin(crate::ops::Operation::CleanupUndo(id))?;
        self.reconcile_locked()?;
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        let moves = r.cleanup_moves(id)?;
        let (root, _) = self.run_root(&run)?;
        if !root.is_dir() {
            return Err(CleanupError::SourceOffline {
                id: run.source.0,
                root,
            });
        }
        // The run folder there must be this run's (its journal header carries the run's uid).
        let journal = quarantine::run_dir_rel(id)
            .under(&root)
            .join(quarantine::JOURNAL_FILE);
        if let Some(h) = quarantine::journal::read_header(&journal)?
            && (h.uid.as_deref() != Some(run.uid.as_str()) || h.run != id)
        {
            return Err(CleanupError::NotRunnable {
                id,
                why: format!(
                    "{} belongs to another cleanup run; nothing was moved",
                    journal.display()
                ),
            });
        }
        let mut journal = CatalogJournal {
            catalog: self.catalog().clone(),
            run: id,
        };
        let report = quarantine::undo(
            &Self::config(&run, &root, Path::new(""), &[]),
            &moves,
            &|m| selection.picks(m),
            &mut journal,
        )?;
        let left = self
            .catalog()
            .read()
            .cleanup_moves(id)?
            .iter()
            .any(|m| m.state.is_quarantined());
        if !left && moves.iter().any(|m| m.state.is_quarantined()) {
            self.catalog()
                .writer()
                .cleanup_set_run_state(id, CleanupRunState::Undone)?;
        }
        Ok(report)
    }

    /// Quarantined files and bytes per source (for reports).
    pub fn quarantined_by_source(&self) -> Result<BTreeMap<SourceId, (u64, u64)>> {
        let mut out: BTreeMap<SourceId, (u64, u64)> = BTreeMap::new();
        for s in self.runs()? {
            let e = out.entry(s.run.source).or_default();
            e.0 += s.counts.quarantined;
            e.1 += s.counts.quarantined_bytes;
        }
        Ok(out)
    }
}
