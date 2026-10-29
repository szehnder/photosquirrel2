//! Reversible cleanup by quarantine (ADR-0004, ADR-0023, ADR-0024, ADR-0034, ADR-0061).
//!
//! The only code that changes a source tree, and only by renaming already-safe files into
//! `<source root>/.photosquirrel-quarantine/<run id>/<relative path>` on the same volume and back. Nothing is
//! ever deleted, copied or overwritten.
//!
//! - [`eligible`] and [`plan_moves`]: the dry run. Only files whose disposition is `exported` or `covered`, that
//!   are present, readable and not user-excluded, with the library copy that makes them safe. Planning only
//!   reads; the plan is stored in the catalog (`cleanup_run`, `cleanup_move`) by the service.
//! - [`run`]: for each planned move, immediately before it: the library copy exists and re-hashes to its
//!   ledger hash ([`crate::library::check_copy`] in verify mode, cached per library file within a run), the
//!   source still has its catalogued stat fingerprint and re-hashes to its catalogued identity, the quarantine
//!   folder is a real folder on the same device; then the write-ahead journal (catalog intent, then
//!   `journal.jsonl` intent, both durable), the no-replace rename ([`fsops::move_no_replace`]) and `done` in
//!   both journals. Any failed check skips the file with the reason.
//! - [`undo`]: the reverse renames of a whole run or a selection, never onto an occupied path (reported and
//!   left in quarantine), with the same journaling. After a full undo the empty folders the run created in its
//!   quarantine folder are removed (`remove_dir`, empty folders only); the journal stays.
//! - [`reconcile`]: after a crash, every move left `moving` or `restoring` is resolved from the file system.
//!
//! [`CleanupJournal`] is the catalog half of the journal (implemented over the catalog in `ps-service`);
//! [`QuarantineHooks`] lets tests inject faults (another volume, a kill mid-run).

pub mod fsops;
pub mod journal;
mod reconcile;
mod run;
mod undo;

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use ps_model::{CleanupMove, Disposition, FileId, MoveState, RelPath};
use serde::Serialize;

use crate::disposition::{FileDisposition, FileFacts};

pub use fsops::MoveError;
pub use journal::{JOURNAL_FILE, JournalEvent, JournalLine, read_journal};
pub use reconcile::{ReconcileReport, reconcile};
pub use run::{RunConfig, RunProgress, RunReport, run};
pub use undo::{UndoNote, UndoReport, undo};

/// The folder ADR-0024 reserves at each source root (equal to `ps_scan::walk::QUARANTINE_DIR`, which the
/// scanner never walks; a ps-service test keeps them equal).
pub const QUARANTINE_DIR: &str = ".photosquirrel-quarantine";

/// `.photosquirrel-quarantine/<run>/<rel_path>`, relative to the source root.
pub fn quarantine_rel_path(run: i64, rel_path: &RelPath) -> RelPath {
    run_dir_rel(run).join(rel_path)
}

/// `.photosquirrel-quarantine/<run>`, relative to the source root.
pub fn run_dir_rel(run: i64) -> RelPath {
    RelPath::from_bytes(format!("{QUARANTINE_DIR}/{run}").into_bytes())
        .unwrap_or_else(|| unreachable!("a quarantine folder name is a valid relative path"))
}

/// The catalog half of the write-ahead journal. `record` is durable when it returns.
pub trait CleanupJournal {
    fn record(&mut self, file: FileId, state: MoveState, note: Option<String>) -> io::Result<()>;
}

/// What decides whether a planned file is still safe to move can change while a run is going (a group edit, an
/// exclusion, a regroup): [`run`] compares [`Recheck::generation`] before every move and, when it changed, asks
/// [`Recheck::holds`] again for the files still to move (ADR-0099). Implemented over the catalog in `ps-service`.
pub trait Recheck {
    /// A counter that changes after every committed write that can change a file's safety.
    fn generation(&self) -> u64;
    /// The files of `planned` that are not safe to move by the dispositions of now, with why
    /// ([`unsafe_now`]). An error leaves the file being checked in place (skipped, never moved).
    fn holds(
        &self,
        planned: &[&CleanupMove],
    ) -> io::Result<std::collections::HashMap<FileId, SkipReason>>;
}

/// Fault injection and platform seams. Production uses [`NoQuarantineHooks`].
pub trait QuarantineHooks: Sync {
    /// The volume `path` lives on ([`fsops::device_of`]).
    fn device(&self, path: &Path) -> io::Result<Option<u64>> {
        fsops::device_of(path)
    }
    /// Whether the filesystem holding `path` keeps file ids ([`fsops::file_ids`], ADR-0112): the pre-move check
    /// compares the catalog's file index only where it does.
    fn file_ids(&self, path: &Path) -> ps_model::IdStability {
        fsops::file_ids(path)
    }
    /// After both intents are journaled, just before the rename.
    fn before_rename(&self, _m: &CleanupMove) {}
    /// Right after the rename, before `done` is journaled.
    fn after_rename(&self, _m: &CleanupMove) {}
}

/// No faults.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoQuarantineHooks;

impl QuarantineHooks for NoQuarantineHooks {}

/// Why a planned file was not moved (or an undo left it in quarantine).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum SkipReason {
    /// Size, modification time, identity or bytes differ from the catalog.
    SourceChanged(String),
    /// Not at its path any more.
    SourceMissing,
    /// Not a regular file (a symlink or folder now has its name).
    NotAFile,
    /// The ledger has no completed row for the covering library copy.
    NotInLedger,
    /// The covering library copy is missing or differs from its ledger hash.
    LibraryCopy(crate::disposition::CopyCheck),
    /// The quarantine folder is on another volume.
    CrossVolume,
    /// Something already exists at the quarantine path.
    QuarantineOccupied,
    /// The quarantine folder is not a real folder inside the source.
    UnsafeFolder(String),
    /// The file system offers no atomic no-replace move (ADR-0062): refused rather than risk an overwrite.
    NoSafeMove(String),
    /// The system refused the move for lack of permission (a read-only folder, another user's file).
    PermissionDenied(String),
    /// A read or rename failed.
    Io(String),
    /// Re-checked when the run started (ADR-0096): by the dispositions of now the file is no longer safe to
    /// move (a better copy of it is not in the library, its library copy is gone, it was excluded, ...).
    NoLongerSafe(String),
    /// It ranks above the keeper the user chose (ADR-0096): accounted for by that choice, but kept.
    BetterThanKeeper,
    /// It is damaged (could not be read: cut short or corrupt, ADR-0115): its exact copy is in the library, but the
    /// original medium may still hold the rest of it for a recovery tool, so it is kept.
    Damaged,
}

impl SkipReason {
    /// A short sentence for reports and the journal.
    pub fn text(&self) -> String {
        match self {
            SkipReason::SourceChanged(s) => format!("changed since scanning: {s}"),
            SkipReason::SourceMissing => "no longer at its path".into(),
            SkipReason::NotAFile => "not a regular file any more".into(),
            SkipReason::NotInLedger => "its library copy is not in the export ledger".into(),
            SkipReason::LibraryCopy(c) => {
                format!("its library copy is {}", crate::copy_check_text(*c))
            }
            SkipReason::CrossVolume => {
                "the quarantine folder is on another volume (never copied)".into()
            }
            SkipReason::QuarantineOccupied => {
                "something already exists at its quarantine path".into()
            }
            SkipReason::UnsafeFolder(s) => format!("unsafe quarantine folder: {s}"),
            SkipReason::NoSafeMove(s) => format!(
                "not moved: the drive cannot rename without risking an overwrite ({s}); nothing was changed"
            ),
            SkipReason::PermissionDenied(s) => {
                format!("not moved: permission denied ({s}); nothing was changed")
            }
            SkipReason::Io(s) => s.clone(),
            SkipReason::NoLongerSafe(s) => format!("no longer safe: {s}; it was not moved"),
            SkipReason::BetterThanKeeper => {
                "better than the keeper you chose: this copy is kept".into()
            }
            SkipReason::Damaged => "damaged: keep the original for recovery".into(),
        }
    }
}

/// A planned file that was not moved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub file: FileId,
    pub rel_path: RelPath,
    pub reason: SkipReason,
    pub text: String,
}

impl Skipped {
    fn new(m: &CleanupMove, reason: SkipReason) -> Self {
        Self::of(m.file, m.rel_path.clone(), reason)
    }

    /// A file left out with `reason` (a plan's files kept back, ADR-0096).
    pub fn of(file: FileId, rel_path: RelPath, reason: SkipReason) -> Self {
        Skipped {
            file,
            rel_path,
            text: reason.text(),
            reason,
        }
    }
}

/// A file eligible for cleanup (ADR-0023, ADR-0024): exported or covered with a library copy, present,
/// readable, analysed, not damaged (ADR-0115) and not excluded by the user. Quarantined files are not eligible
/// again.
pub fn eligible(f: &FileFacts, d: &FileDisposition) -> bool {
    !f.damaged && eligible_but_damaged(f, d)
}

/// [`eligible`] without the damaged rule: a damaged file this holds for is kept back ([`SkipReason::Damaged`]).
pub fn eligible_but_damaged(f: &FileFacts, d: &FileDisposition) -> bool {
    d.disposition.is_accounted_safe()
        && !d.better_than_keeper
        && d.library_path.is_some()
        && f.content.is_some()
        && !f.missing
        && !f.unreadable
        && !f.user_excluded
        && f.quarantined.is_none()
}

/// Why a file must not be moved now, from its facts and disposition derived now (ADR-0096); `None` when it is
/// [`eligible`] and, for a planned move, still made safe by the library copy it was planned with
/// (`covering`). A run re-derives every disposition when it starts and skips each planned file this names, so
/// a plan stored before a keeper change or an exclusion never moves a file that is no longer safe.
pub fn unsafe_now(
    f: &FileFacts,
    d: &FileDisposition,
    covering: Option<&RelPath>,
) -> Option<SkipReason> {
    let no = |s: &str| Some(SkipReason::NoLongerSafe(s.to_owned()));
    if f.damaged {
        // Found damaged since planning, or before: the original stays (ADR-0115).
        return Some(SkipReason::Damaged);
    }
    if d.better_than_keeper {
        return Some(SkipReason::BetterThanKeeper);
    }
    match d.disposition {
        Disposition::Exported | Disposition::Covered => {}
        Disposition::Superseded => {
            return no("it is the keeper or a better copy, and it is not in the library yet");
        }
        Disposition::Pending => return no("no equal or better copy of it is in the library now"),
        Disposition::Excluded => return no("it is excluded now"),
        Disposition::Missing => return Some(SkipReason::SourceMissing),
        Disposition::Unreadable => return no("it could not be read at the last scan"),
        Disposition::Quarantined => return no("it is in a quarantine folder already"),
    }
    if f.user_excluded {
        return no("you excluded it");
    }
    if !eligible(f, d) {
        return no("it is not exported or covered any more");
    }
    if covering.is_some_and(|c| d.library_path.as_ref() != Some(c)) {
        return no("the library copy that makes it safe changed since planning; plan again");
    }
    None
}

/// What [`plan_moves`] needs of one eligible file beyond its facts: the catalogued stat fingerprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    pub mtime_ns: i64,
    pub file_index: Option<u64>,
}

/// The planned moves of run `run`: every eligible file under `folder` (all when `None`), ordered by path.
/// Pure: reads nothing.
pub fn plan_moves<'a>(
    run: i64,
    folder: Option<&RelPath>,
    files: impl IntoIterator<Item = (&'a FileFacts, &'a FileDisposition, Stat)>,
) -> Vec<CleanupMove> {
    let mut out: Vec<CleanupMove> = files
        .into_iter()
        .filter(|(f, d, _)| eligible(f, d))
        .filter(|(f, _, _)| folder.is_none_or(|dir| f.rel_path.is_within(dir)))
        .filter_map(|(f, d, s)| {
            Some(CleanupMove {
                file: f.file,
                rel_path: f.rel_path.clone(),
                quarantine_rel_path: quarantine_rel_path(run, &f.rel_path),
                size: s.size,
                mtime_ns: s.mtime_ns,
                file_index: s.file_index,
                content: f.content?,
                disposition: d.disposition,
                covering_library_path: d.library_path.clone()?,
                state: MoveState::Planned,
                note: None,
                at_ms: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

/// Files and bytes of one folder of a plan or run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FolderTotals {
    /// `None`: the source root.
    pub folder: Option<RelPath>,
    pub files: u64,
    pub bytes: u64,
    pub exported: u64,
    pub covered: u64,
}

/// Totals of a set of moves per folder (sorted) and overall.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MoveTotals {
    pub files: u64,
    pub bytes: u64,
    pub folders: Vec<FolderTotals>,
}

/// Totals of `moves` (only those `wanted`) per folder.
pub fn totals<'a>(moves: impl IntoIterator<Item = &'a CleanupMove>) -> MoveTotals {
    let mut per: BTreeMap<Option<RelPath>, FolderTotals> = BTreeMap::new();
    let mut t = MoveTotals::default();
    for m in moves {
        let folder = m.rel_path.parent();
        let e = per.entry(folder.clone()).or_insert_with(|| FolderTotals {
            folder,
            ..FolderTotals::default()
        });
        e.files += 1;
        e.bytes += m.size;
        match m.disposition {
            Disposition::Covered => e.covered += 1,
            _ => e.exported += 1,
        }
        t.files += 1;
        t.bytes += m.size;
    }
    t.folders = per.into_values().collect();
    t
}
