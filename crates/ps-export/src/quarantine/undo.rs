//! Undo: rename quarantined files back, never onto an occupied path (module docs of [`super`]).

use std::fs;
use std::io;
use std::path::Path;

use ps_model::{CleanupMove, FileId, MoveState, RelPath};
use serde::Serialize;

use super::fsops::{self, MoveError};
use super::journal::{Journal, JournalEvent};
use super::run::RunConfig;
use super::{CleanupJournal, run_dir_rel};

/// A file undo left in quarantine, or restored with a remark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UndoNote {
    pub file: FileId,
    pub rel_path: RelPath,
    pub text: String,
}

/// What an undo did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct UndoReport {
    pub restored: u64,
    pub bytes_restored: u64,
    /// Left in quarantine: the original path is occupied (never overwritten) or the rename failed.
    pub conflicts: Vec<UndoNote>,
    /// Not in the quarantine folder any more (moved or deleted by someone else); nothing to restore.
    pub missing: Vec<UndoNote>,
    /// Restored, but its bytes differ from the scan (edited while in quarantine).
    pub changed: Vec<UndoNote>,
    /// No file of the run is in quarantine any more: the empty folders it created were removed.
    pub run_fully_undone: bool,
    pub empty_folders_removed: usize,
}

fn note(m: &CleanupMove, text: impl Into<String>) -> UndoNote {
    UndoNote {
        file: m.file,
        rel_path: m.rel_path.clone(),
        text: text.into(),
    }
}

/// The quarantined file still has its catalogued size and identity.
fn unchanged(path: &Path, m: &CleanupMove) -> bool {
    let Ok(md) = fs::symlink_metadata(path) else {
        return false;
    };
    if md.len() != m.size {
        return false;
    }
    let Ok(f) = fs::File::open(path) else {
        return false;
    };
    let found = match m.content.kind {
        ps_model::ContentIdKind::Full => {
            crate::library::full_hash_of(f).map(|(h, _)| ps_model::ContentId::full(h))
        }
        ps_model::ContentIdKind::Sampled => ps_hash::identity_from_reader(f, m.size),
    };
    found.is_ok_and(|c| c == m.content)
}

/// Undo the quarantined moves of `moves` that `select` picks (every move of the run when it picks all).
/// Uses `cfg.run`, `cfg.uid` and `cfg.source_root` (the library is not needed). Errors: a journal write
/// failed (the undo stops).
pub fn undo(
    cfg: &RunConfig<'_>,
    moves: &[CleanupMove],
    select: &dyn Fn(&CleanupMove) -> bool,
    catalog: &mut dyn CleanupJournal,
) -> io::Result<UndoReport> {
    let root = cfg.source_root;
    let run_dir = run_dir_rel(cfg.run).under(root);
    let mut report = UndoReport::default();
    let mut journal: Option<Journal> = None;
    let mut still_quarantined = 0u64;
    for m in moves {
        if !m.state.is_quarantined() {
            continue;
        }
        if !select(m) {
            still_quarantined += 1;
            continue;
        }
        let q = m.quarantine_rel_path.under(root);
        let original = m.rel_path.under(root);
        if fs::symlink_metadata(&q).is_err() {
            report
                .missing
                .push(note(m, "not in the quarantine folder any more"));
            still_quarantined += 1;
            continue;
        }
        if journal.is_none() {
            journal = Some(Journal::open(&run_dir, cfg.run, cfg.uid, root)?);
        }
        let Some(j) = journal.as_mut() else {
            unreachable!("opened above")
        };
        let conflict = |j: &mut Journal,
                        catalog: &mut dyn CleanupJournal,
                        report: &mut UndoReport,
                        text: String|
         -> io::Result<()> {
            j.record(JournalEvent::Conflict, m, Some(text.clone()))?;
            catalog.record(m.file, MoveState::Quarantined, Some(text.clone()))?;
            report.conflicts.push(note(m, text));
            Ok(())
        };
        if fs::symlink_metadata(&original).is_ok() {
            conflict(
                j,
                catalog,
                &mut report,
                "its original path is occupied by another file; left in quarantine".into(),
            )?;
            still_quarantined += 1;
            continue;
        }
        let changed = !unchanged(&q, m);
        let parent_rel = m
            .rel_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let dir = match fsops::prepare_dir(root, &parent_rel) {
            Ok(d) => d,
            Err(e) => {
                conflict(
                    j,
                    catalog,
                    &mut report,
                    format!("its original folder cannot be used: {e}; left in quarantine"),
                )?;
                still_quarantined += 1;
                continue;
            }
        };
        catalog.record(m.file, MoveState::Restoring, None)?;
        j.record(JournalEvent::UndoIntent, m, None)?;
        match fsops::move_no_replace(&q, &original) {
            Ok(_) => {
                fsops::sync_dir_logged(&dir);
                let text = changed.then(|| {
                    "restored, but its contents changed while it was in quarantine".to_owned()
                });
                j.record(JournalEvent::Restored, m, text.clone())?;
                catalog.record(m.file, MoveState::Restored, text.clone())?;
                if let Some(t) = text {
                    report.changed.push(note(m, t));
                }
                report.restored += 1;
                report.bytes_restored += m.size;
            }
            Err(e) => {
                let text = match e {
                    MoveError::Occupied(_) => {
                        "its original path is occupied by another file; left in quarantine"
                            .to_owned()
                    }
                    other => format!("could not be moved back: {other}; left in quarantine"),
                };
                conflict(j, catalog, &mut report, text)?;
                still_quarantined += 1;
            }
        }
    }
    if still_quarantined == 0 {
        report.run_fully_undone = true;
        if run_dir.is_dir() {
            report.empty_folders_removed = fsops::remove_empty_dirs(&run_dir);
        }
    }
    Ok(report)
}
