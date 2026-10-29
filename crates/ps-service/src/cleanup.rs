//! Reversible quarantine cleanup (change `quarantine-cleanup`; ADR-0004, ADR-0023, ADR-0024, ADR-0034,
//! ADR-0061).
//!
//! [`Cleanups`] plans a cleanup of one source (or one folder of it) from the derived dispositions, stores the
//! plan in the catalog (`cleanup_run`, `cleanup_move`), runs a confirmed plan through
//! [`ps_export::quarantine::run`] with the catalog as the durable half of the journal ([`CatalogJournal`]),
//! undoes whole runs or a folder or files of one, and reconciles moves a crash left mid-way. A stored plan can
//! be narrowed to a [`Selection`] before it runs (the moves left out become `unselected`) and a plan that
//! never moved a file can be discarded (change `cleanup-selection`, ADR-0068); both write catalog rows only. Every command
//! reconciles first, so a restart after a crash always resolves the journal before anything else happens.
//!
//! Each command holds the service's operation slot ([`crate::ops`], ADR-0065): no scan, export or other cleanup
//! command runs meanwhile, and reconciliation never touches a run that is live in this process. A run is tied
//! to its source by identity (id and `added_at`, ADR-0063): when that source is gone the run acts on the root
//! stored with it, never on whatever source has its id now.
//!
//! Nothing here touches a source tree directly: only `ps-export::quarantine` renames files, and only after
//! re-verifying them. Nothing is ever deleted.

use std::collections::{BTreeMap, HashMap, HashSet};
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
        "a keeper, group or exclusion change is being saved: start the cleanup again in a moment"
    )]
    EditSaving,
    #[error(
        "source {0} is a network share, where a rename may not be atomic: confirm cleanup on a network source"
    )]
    NetworkNotConfirmed(i64),
    #[error("cleanup run {id} cannot run: {why}")]
    NotRunnable { id: i64, why: String },
    #[error(
        "this selection leaves no file of cleanup run {id} to move: discard the plan instead (cleanup discard {id})"
    )]
    EmptySelection { id: i64 },
    #[error("cleanup run {id} cannot be discarded: {why}")]
    NotDiscardable { id: i64, why: String },
    #[error("cleanup run {id} cannot be narrowed: {why}")]
    NotNarrowable { id: i64, why: String },
    #[error(
        "{files} analysed files bearing on this cleanup were catalogued by an earlier version and are not re-checked \
         yet (burst frame numbers, RAW files cut short), so their groups are not trusted: rescan the source (and any \
         source sharing groups with it) first"
    )]
    RescanFirst { files: u64 },
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

/// A running cleanup's view of the catalog's constraints (ADR-0099): the generation, and the holds of the files
/// still to move derived again by [`Cleanups::holds`] when it changed.
struct RunRecheck<'c, 'a> {
    cleanups: &'c Cleanups<'a>,
    run: &'c CleanupRunRecord,
    library: &'c Path,
}

impl quarantine::Recheck for RunRecheck<'_, '_> {
    fn generation(&self) -> u64 {
        self.cleanups.catalog().writer().constraints_generation()
    }

    fn holds(&self, planned: &[&CleanupMove]) -> io::Result<HashMap<FileId, SkipReason>> {
        self.cleanups
            .holds(self.run, planned.iter().copied(), self.library)
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
    /// Files of the source (or folder) that would be eligible but are kept back, with why: copies better than
    /// a keeper the user chose (ADR-0096) and damaged originals (ADR-0115). Never planned.
    pub held: Vec<Skipped>,
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

/// Which moves of a run a selection picks: for narrowing a plan ([`Cleanups::select`]) and for undo
/// ([`Cleanups::undo`]). Folders mean their whole subtree ([`RelPath::is_within`]), as `cleanup plan --folder`.
///
/// A move is picked when (`folders` and `files` are both empty, or it is within one of `folders`, or it is one
/// of `files`) and it is neither within one of `except_folders` nor one of `except_files`. The inclusion lists
/// serve the CLI; the exclusion lists serve the app, which starts with everything ticked and records what the
/// user unticks, so a selection stays small however big the plan is. The empty selection picks everything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Selection {
    pub folders: Vec<RelPath>,
    pub files: Vec<FileId>,
    pub except_folders: Vec<RelPath>,
    pub except_files: Vec<FileId>,
}

impl Selection {
    /// Every move.
    pub fn all() -> Self {
        Self::default()
    }

    /// Only the moves under `folder`.
    pub fn folder(folder: RelPath) -> Self {
        Self {
            folders: vec![folder],
            ..Self::default()
        }
    }

    /// Whether the selection picks `m` (only its path and file id are looked at).
    pub fn picks(&self, m: &CleanupMove) -> bool {
        let included = (self.folders.is_empty() && self.files.is_empty())
            || self.folders.iter().any(|f| m.rel_path.is_within(f))
            || self.files.contains(&m.file);
        included
            && !self.except_folders.iter().any(|f| m.rel_path.is_within(f))
            && !self.except_files.contains(&m.file)
    }

    /// A [`Selection::picks`] that is fast for long file lists (the app's exclusions can name many files).
    fn picker(&self) -> impl Fn(&CleanupMove) -> bool + '_ {
        let files: std::collections::HashSet<FileId> = self.files.iter().copied().collect();
        let except: std::collections::HashSet<FileId> = self.except_files.iter().copied().collect();
        move |m| {
            let included = (self.folders.is_empty() && files.is_empty())
                || self.folders.iter().any(|f| m.rel_path.is_within(f))
                || files.contains(&m.file);
            included
                && !self.except_folders.iter().any(|f| m.rel_path.is_within(f))
                && !except.contains(&m.file)
        }
    }
}

/// A term of a [`Selection`] that picks none of the moves it was matched against (change `cleanup-selection`,
/// gate review): `cleanup select` and `cleanup undo` warn about each one instead of ignoring it silently.
/// Matching stays exact and case-sensitive; `did_you_mean` only names a folder of the run that equals the term
/// ignoring case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnmatchedTerm {
    /// The command-line flag without its dashes: `folder`, `file`, `except-folder` or `except-file`.
    pub flag: &'static str,
    /// The folder path or file id as given.
    pub term: String,
    /// A folder (or file path) of the run equal to `term` ignoring case.
    pub did_you_mean: Option<String>,
}

impl Selection {
    /// The terms of this selection that pick none of `moves` (each folder must contain one of them, each file id
    /// must be one of them), in the order folders, files, except-folders, except-files.
    pub fn unmatched(&self, moves: &[&CleanupMove]) -> Vec<UnmatchedTerm> {
        let ids: std::collections::HashSet<FileId> = moves.iter().map(|m| m.file).collect();
        let folder = |flag: &'static str, f: &RelPath| {
            (!moves.iter().any(|m| m.rel_path.is_within(f))).then(|| UnmatchedTerm {
                flag,
                term: f.display().into_owned(),
                did_you_mean: same_ignoring_case(moves, f),
            })
        };
        let file = |flag: &'static str, id: &FileId| {
            (!ids.contains(id)).then(|| UnmatchedTerm {
                flag,
                term: id.0.to_string(),
                did_you_mean: None,
            })
        };
        let mut out: Vec<UnmatchedTerm> = Vec::new();
        out.extend(self.folders.iter().filter_map(|f| folder("folder", f)));
        out.extend(self.files.iter().filter_map(|f| file("file", f)));
        out.extend(
            self.except_folders
                .iter()
                .filter_map(|f| folder("except-folder", f)),
        );
        out.extend(
            self.except_files
                .iter()
                .filter_map(|f| file("except-file", f)),
        );
        out
    }
}

/// The first folder (or file path) of `moves`, in path order, that equals `term` ignoring case.
fn same_ignoring_case(moves: &[&CleanupMove], term: &RelPath) -> Option<String> {
    let want = term.display().to_lowercase();
    let mut found: Option<String> = None;
    for m in moves {
        let path = m.rel_path.display();
        let ends = path
            .char_indices()
            .filter(|(_, c)| *c == '/')
            .map(|(i, _)| i)
            .chain(std::iter::once(path.len()));
        for end in ends {
            let candidate = &path[..end];
            if candidate.to_lowercase() == want && found.as_deref().is_none_or(|f| candidate < f) {
                found = Some(candidate.to_owned());
            }
        }
    }
    found
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
            crate::ops::Busy::Cleanup(id) => {
                CleanupError::Busy(crate::ops::Operation::CleanupRun(id))
            }
            crate::ops::Busy::Edit => CleanupError::EditSaving,
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

    /// The moves of run `id` whose file lies directly in `folder` (`None`: the source root), by path.
    pub fn folder_moves(&self, id: i64, folder: Option<&RelPath>) -> Result<Vec<CleanupMove>> {
        Ok(self.catalog().read().cleanup_moves_in_folder(id, folder)?)
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
        let in_scope = |f: &ps_export::FileFacts| {
            f.source == source && folder.as_ref().is_none_or(|dir| f.rel_path.is_within(dir))
        };
        // ADR-0108: groups formed before frame identity are not trusted for a move.
        let stale = ex.rescan_needed(&files, &[&in_scope])?[0];
        if stale > 0 {
            return Err(CleanupError::RescanFirst { files: stale });
        }
        let mut stats: HashMap<FileId, StatFingerprint> = HashMap::new();
        self.catalog().read().for_each_file(|f| {
            if f.key.source == source {
                stats.insert(f.id, f.stat);
            }
        })?;
        let mut held: Vec<Skipped> = files
            .iter()
            .zip(&dispositions)
            .filter(|(f, d)| {
                in_scope(f)
                    && d.better_than_keeper
                    && quarantine::eligible(
                        f,
                        &ps_export::FileDisposition {
                            better_than_keeper: false,
                            ..(*d).clone()
                        },
                    )
            })
            .map(|(f, _)| Skipped::of(f.file, f.rel_path.clone(), SkipReason::BetterThanKeeper))
            .collect();
        // Damaged originals stay where they are (ADR-0115): the medium may still hold the rest of the photo.
        held.extend(
            files
                .iter()
                .zip(&dispositions)
                .filter(|(f, d)| {
                    in_scope(f)
                        && f.damaged
                        && !d.better_than_keeper
                        && quarantine::eligible_but_damaged(f, d)
                })
                .map(|(f, _)| Skipped::of(f.file, f.rel_path.clone(), SkipReason::Damaged)),
        );
        held.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
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
                held,
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
            held,
        })
    }

    fn config<'c>(
        run: &'c CleanupRunRecord,
        root: &'c Path,
        library: &'c Path,
        ledger: &'c [ps_model::LedgerEntry],
        holds: &'c HashMap<FileId, SkipReason>,
    ) -> RunConfig<'c> {
        RunConfig {
            run: run.id,
            uid: &run.uid,
            source_root: root,
            library_root: library,
            ledger,
            holds,
            recheck: None,
        }
    }

    /// The planned moves of `run` that are no longer safe by the dispositions derived now (ADR-0096), with why:
    /// a keeper change, an exclusion or a lost library copy since planning makes a stored plan stale, and the
    /// run must not trust it. Library copies are checked by size here (re-hashed by the run itself).
    fn holds<'m>(
        &self,
        run: &CleanupRunRecord,
        moves: impl Iterator<Item = &'m CleanupMove>,
        library: &Path,
    ) -> Result<HashMap<FileId, SkipReason>> {
        let (files, dispositions, _) = self.svc.exports().dispositions(library, false)?;
        let now: HashMap<FileId, (&ps_export::FileFacts, &ps_export::FileDisposition)> = files
            .iter()
            .zip(&dispositions)
            .map(|(f, d)| (f.file, (f, d)))
            .collect();
        let mut out = HashMap::new();
        for m in moves.filter(|m| m.state == MoveState::Planned) {
            let why = match now.get(&m.file) {
                Some((f, d)) if f.source == run.source && f.rel_path == m.rel_path => {
                    quarantine::unsafe_now(f, d, Some(&m.covering_library_path))
                }
                _ => Some(SkipReason::NoLongerSafe(
                    "its catalog record changed since planning; plan again".into(),
                )),
            };
            if let Some(w) = why {
                out.insert(m.file, w);
            }
        }
        Ok(out)
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
            &Self::config(&run, &root, Path::new(""), &[], &HashMap::new()),
            &moves,
            &mut journal,
        )?;
        let adopted = report
            .resolved
            .iter()
            .any(|(_, s, _)| s.is_quarantined() || *s == MoveState::Restored);
        if run.state == CleanupRunState::Running {
            self.catalog()
                .writer()
                .cleanup_set_run_state(id, CleanupRunState::Completed)?;
        } else if run.state == CleanupRunState::Discarded && adopted {
            // Its journal file records a move the catalog had lost (ADR-0068): the run did move files, so it
            // is a completed run again and can be undone.
            log::warn!(
                "cleanup run {id} was discarded, but its journal records moved files: it is a completed run \
                 again (undo it to put them back)"
            );
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
        if run.state == CleanupRunState::Discarded {
            return Err(not_runnable("it was discarded"));
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
        // ADR-0108: groups formed before frame identity are not trusted for a move (a plan made before the upgrade).
        if !r.files_awaiting_meta_reread()?.is_empty() {
            let planned: HashSet<FileId> = moves
                .iter()
                .filter(|m| m.state == MoveState::Planned)
                .map(|m| m.file)
                .collect();
            let ex = self.svc.exports();
            let (files, _, _) = ex.dispositions(&library, false)?;
            let stale = ex.rescan_needed(
                &files,
                &[&|f: &ps_export::FileFacts| planned.contains(&f.file)],
            )?[0];
            if stale > 0 {
                return Err(CleanupError::RescanFirst { files: stale });
            }
        }
        let ledger = r.ledger(run.library_id)?;
        // Read before the holds are derived: a change committed meanwhile makes the run derive them again.
        let generation = self.catalog().writer().constraints_generation();
        let holds = self.holds(&run, moves.iter(), &library)?;
        let recheck = RunRecheck {
            cleanups: self,
            run: &run,
            library: &library,
        };
        self.catalog()
            .writer()
            .cleanup_set_run_state(id, CleanupRunState::Running)?;
        let mut journal = CatalogJournal {
            catalog: self.catalog().clone(),
            run: id,
        };
        let cfg = RunConfig {
            recheck: Some((&recheck, generation)),
            ..Self::config(&run, &root, &library, &ledger, &holds)
        };
        let result = quarantine::run(&cfg, &moves, &mut journal, hooks, cancel, progress);
        let state = match &result {
            Ok(rep) if rep.cancelled => CleanupRunState::Planned,
            _ => CleanupRunState::Completed,
        };
        self.catalog().writer().cleanup_set_run_state(id, state)?;
        Ok(result?)
    }

    /// Narrow (or widen again) a stored plan to the moves `selection` picks (ADR-0068): of the moves still
    /// planned or unselected, the picked ones become planned and the others unselected, in one durable
    /// transaction. Only a `planned` run (also one that was stopped part-way) can be selected; a selection that
    /// leaves nothing planned is refused ([`CleanupError::EmptySelection`]). Writes catalog rows only: the
    /// source need not be connected.
    pub fn select(&self, id: i64, selection: &Selection) -> Result<RunSummary> {
        self.writable()?;
        let _op = self.begin(crate::ops::Operation::CleanupSelect(id))?;
        self.reconcile_locked()?;
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        let not = |why: String| CleanupError::NotNarrowable { id, why };
        match run.state {
            CleanupRunState::Planned => {}
            CleanupRunState::Discarded => {
                return Err(not("it was discarded; plan again instead".into()));
            }
            other => {
                return Err(not(format!(
                    "it is {}; only a planned run can be narrowed",
                    other.as_str()
                )));
            }
        }
        let moves = r.cleanup_moves(id)?;
        let picks = selection.picker();
        let picked: Vec<FileId> = moves
            .iter()
            .filter(|m| matches!(m.state, MoveState::Planned | MoveState::Unselected))
            .filter(|m| picks(m))
            .map(|m| m.file)
            .collect();
        if picked.is_empty() {
            return Err(CleanupError::EmptySelection { id });
        }
        self.catalog().writer().cleanup_select_moves(id, picked)?;
        Ok(self.run_detail(id)?.0)
    }

    /// Discard a run that never moved a file (ADR-0068): it stays in the history as `discarded`, its planned
    /// moves become unselected, and it can no longer be run, selected or undone. Refused when any of its moves
    /// is or was moving, quarantined, restoring or restored, and when its quarantine folder holds a journal
    /// that records a move (reconciliation runs first, so such a move is adopted and named). Touches no file
    /// or folder; works with the drive disconnected (the catalog half of the journal is durable before every
    /// rename, ADR-0064, and reconciliation keeps watching discarded runs).
    pub fn discard(&self, id: i64) -> Result<RunSummary> {
        self.writable()?;
        let _op = self.begin(crate::ops::Operation::CleanupDiscard(id))?;
        self.reconcile_locked()?;
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        let refuse = |why: String| CleanupError::NotDiscardable { id, why };
        if run.state == CleanupRunState::Discarded {
            return Err(refuse("it was already discarded".into()));
        }
        let moves = r.cleanup_moves(id)?;
        let moved: Vec<&CleanupMove> = moves
            .iter()
            .filter(|m| {
                matches!(
                    m.state,
                    MoveState::Moving
                        | MoveState::Quarantined
                        | MoveState::Restoring
                        | MoveState::Restored
                )
            })
            .collect();
        if !moved.is_empty() {
            let names: Vec<String> = moved
                .iter()
                .take(3)
                .map(|m| format!("{} ({})", m.rel_path, m.state.as_str()))
                .collect();
            let more = if moved.len() > 3 {
                format!(" and {} more", moved.len() - 3)
            } else {
                String::new()
            };
            return Err(refuse(format!(
                "it moved {} files: {}{more}. Only a plan that never moved a file can be discarded; the run \
                 stays in the history (undo puts quarantined files back)",
                moved.len(),
                names.join(", ")
            )));
        }
        if run.state != CleanupRunState::Planned {
            return Err(refuse(format!(
                "it is {}; only a plan that never moved a file can be discarded",
                run.state.as_str()
            )));
        }
        let (root, _) = self.run_root(&run)?;
        let journal = quarantine::run_dir_rel(id)
            .under(&root)
            .join(quarantine::JOURNAL_FILE);
        if root.is_dir() && journal.is_file() {
            let lines = quarantine::read_journal(&journal).map_err(|e| {
                refuse(format!(
                    "its journal {} could not be read ({e})",
                    journal.display()
                ))
            })?;
            let moving = lines.iter().filter(|l| {
                matches!(
                    l.event,
                    quarantine::JournalEvent::Intent
                        | quarantine::JournalEvent::Done
                        | quarantine::JournalEvent::UndoIntent
                        | quarantine::JournalEvent::Restored
                )
            });
            if let Some(l) = moving.clone().next() {
                return Err(refuse(format!(
                    "its journal {} records {} move(s), e.g. of {}",
                    journal.display(),
                    moving.count(),
                    l.original.as_deref().unwrap_or("a file")
                )));
            }
        }
        if !self.catalog().writer().cleanup_discard_run(id)? {
            return Err(refuse("it changed while it was being discarded".into()));
        }
        Ok(self.run_detail(id)?.0)
    }

    /// Undo a run, or the part `selection` picks: rename quarantined files back to their paths, never onto
    /// an occupied path (reported, left in quarantine). The run is `undone` once none of its files is in
    /// quarantine any more.
    pub fn undo(&self, id: i64, selection: &Selection) -> Result<UndoReport> {
        self.writable()?;
        let _op = self.begin(crate::ops::Operation::CleanupUndo(id))?;
        self.reconcile_locked()?;
        let r = self.catalog().read();
        let run = r.cleanup_run(id)?.ok_or(CleanupError::UnknownRun(id))?;
        if run.state == CleanupRunState::Discarded {
            return Err(CleanupError::NotRunnable {
                id,
                why: "it was discarded and moved nothing; there is nothing to undo".into(),
            });
        }
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
            &Self::config(&run, &root, Path::new(""), &[], &HashMap::new()),
            &moves,
            &selection.picker(),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn rel(s: &str) -> RelPath {
        RelPath::from_bytes(s.as_bytes().to_vec()).unwrap()
    }

    fn mv(file: i64, path: &str) -> CleanupMove {
        CleanupMove {
            file: FileId(file),
            rel_path: rel(path),
            quarantine_rel_path: quarantine::quarantine_rel_path(1, &rel(path)),
            size: 1,
            mtime_ns: 0,
            file_index: None,
            content: ps_model::ContentId::full([0; 32]),
            disposition: ps_model::Disposition::Exported,
            covering_library_path: rel("lib.jpg"),
            state: MoveState::Planned,
            note: None,
            at_ms: None,
        }
    }

    fn moves() -> Vec<CleanupMove> {
        vec![
            mv(1, "root.jpg"),
            mv(2, "DCIM/a.jpg"),
            mv(3, "DCIM/100/b.jpg"),
            mv(4, "DCIM/100/c.jpg"),
            mv(5, "DCIMX/d.jpg"),
            mv(6, "Backup/2008/e.jpg"),
        ]
    }

    fn picked(s: &Selection) -> Vec<i64> {
        let by_picks: Vec<i64> = moves()
            .iter()
            .filter(|m| s.picks(m))
            .map(|m| m.file.0)
            .collect();
        let fast = s.picker();
        let by_picker: Vec<i64> = moves()
            .iter()
            .filter(|m| fast(m))
            .map(|m| m.file.0)
            .collect();
        assert_eq!(by_picks, by_picker, "picks and picker agree");
        by_picks
    }

    #[test]
    fn selection_terms_that_match_nothing_are_named_with_a_case_insensitive_hint() {
        let all = moves();
        let refs: Vec<&CleanupMove> = all.iter().collect();
        let s = Selection {
            folders: vec![rel("dcim/100"), rel("DCIM")],
            files: vec![FileId(2), FileId(99)],
            except_folders: vec![rel("backup"), rel("Nowhere")],
            except_files: vec![FileId(98)],
        };
        let u = s.unmatched(&refs);
        let got: Vec<(&str, &str, Option<&str>)> = u
            .iter()
            .map(|t| (t.flag, t.term.as_str(), t.did_you_mean.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("folder", "dcim/100", Some("DCIM/100")),
                ("file", "99", None),
                ("except-folder", "backup", Some("Backup")),
                ("except-folder", "Nowhere", None),
                ("except-file", "98", None),
            ]
        );
        // Matching itself stays case-sensitive: the lower-case folder picks nothing.
        assert!(picked(&Selection::folder(rel("dcim"))).is_empty());
        assert!(Selection::all().unmatched(&refs).is_empty());
        // A folder whose files are not among the moves matched against (already quarantined, say) is named.
        let planned: Vec<&CleanupMove> = refs.iter().copied().filter(|m| m.file.0 != 6).collect();
        assert_eq!(
            Selection::folder(rel("Backup")).unmatched(&planned)[0].did_you_mean,
            None
        );
    }

    #[test]
    fn empty_selection_picks_everything() {
        assert_eq!(picked(&Selection::all()), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn selection_inclusion_by_folder_and_file() {
        // A folder means its subtree, and never a sibling that only shares a prefix (DCIMX).
        assert_eq!(picked(&Selection::folder(rel("DCIM"))), vec![2, 3, 4]);
        let s = Selection {
            folders: vec![rel("DCIM/100"), rel("Backup")],
            files: vec![FileId(1)],
            ..Selection::default()
        };
        assert_eq!(picked(&s), vec![1, 3, 4, 6]);
        let files_only = Selection {
            files: vec![FileId(5), FileId(42)],
            ..Selection::default()
        };
        assert_eq!(picked(&files_only), vec![5], "an unknown id picks nothing");
    }

    #[test]
    fn selection_exclusion_by_folder_and_file() {
        let s = Selection {
            except_folders: vec![rel("DCIM/100")],
            except_files: vec![FileId(6)],
            ..Selection::default()
        };
        assert_eq!(picked(&s), vec![1, 2, 5]);
        // Exclusions win over inclusions.
        let s = Selection {
            folders: vec![rel("DCIM")],
            files: vec![FileId(3)],
            except_folders: vec![rel("DCIM/100")],
            except_files: vec![FileId(2)],
        };
        assert_eq!(picked(&s), Vec::<i64>::new());
    }

    #[test]
    fn selection_subtree_semantics() {
        // Excluding a parent excludes its children; excluding a child keeps the parent's other files.
        let s = Selection {
            except_folders: vec![rel("DCIM")],
            ..Selection::default()
        };
        assert_eq!(picked(&s), vec![1, 5, 6]);
        let s = Selection {
            folders: vec![rel("DCIM")],
            except_folders: vec![rel("DCIM/100")],
            ..Selection::default()
        };
        assert_eq!(picked(&s), vec![2]);
    }
}
