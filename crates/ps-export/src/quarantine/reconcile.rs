//! Crash reconciliation (ADR-0061): moves left `moving` or `restoring` are resolved from the file system.

use std::fs;
use std::io;
use std::path::Path;

use ps_model::{CleanupMove, FileId, MoveState};
use serde::Serialize;

use super::CleanupJournal;
use super::journal::{Journal, JournalEvent};
use super::run::RunConfig;
use super::run_dir_rel;

/// How each interrupted move was resolved.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReconcileReport {
    /// (file, new state, note).
    pub resolved: Vec<(FileId, MoveState, Option<String>)>,
}

/// A regular file of the move's size is at `p`.
fn holds(p: &Path, m: &CleanupMove) -> bool {
    fs::symlink_metadata(p).is_ok_and(|md| md.file_type().is_file() && md.len() == m.size)
}

/// Resolve every move of the run in state `moving` or `restoring`:
///
/// - `moving`: in quarantine only -> `quarantined`; at its path only -> `planned` (the rename never happened;
///   a later run moves it); both or neither -> `skipped` with the reason, nothing touched;
/// - `restoring`: at its path only -> `restored`; in quarantine only -> `quarantined`; both or neither ->
///   `quarantined` with the reason (undo reports it again).
///
/// Writes the outcome to both journals (the JSON one only if the run folder exists).
pub fn reconcile(
    cfg: &RunConfig<'_>,
    moves: &[CleanupMove],
    catalog: &mut dyn CleanupJournal,
) -> io::Result<ReconcileReport> {
    let root = cfg.source_root;
    let run_dir = run_dir_rel(cfg.run).under(root);
    let mut journal = if run_dir.is_dir() {
        Journal::open(&run_dir, cfg.run, cfg.uid, root).ok()
    } else {
        None
    };
    let mut report = ReconcileReport::default();
    for m in moves {
        let (at_home, in_q) = (
            holds(&m.rel_path.under(root), m),
            holds(&m.quarantine_rel_path.under(root), m),
        );
        let (state, text, event) = match (m.state, at_home, in_q) {
            (MoveState::Moving, false, true) => (MoveState::Quarantined, None, JournalEvent::Done),
            (MoveState::Moving, true, false) => (
                MoveState::Planned,
                Some("not moved: the run was interrupted before the rename".to_owned()),
                JournalEvent::Skipped,
            ),
            (MoveState::Moving, true, true) => (
                MoveState::Skipped,
                Some("found at both its path and its quarantine path after an interruption; both left alone".to_owned()),
                JournalEvent::Skipped,
            ),
            (MoveState::Moving, false, false) => (
                MoveState::Skipped,
                Some("found at neither its path nor its quarantine path after an interruption".to_owned()),
                JournalEvent::Skipped,
            ),
            (MoveState::Restoring, true, false) => {
                (MoveState::Restored, None, JournalEvent::Restored)
            }
            (MoveState::Restoring, false, true) => (
                MoveState::Quarantined,
                Some("undo was interrupted before the rename; still in quarantine".to_owned()),
                JournalEvent::Conflict,
            ),
            (MoveState::Restoring, true, true) => (
                MoveState::Quarantined,
                Some("its original path is occupied by another file; left in quarantine".to_owned()),
                JournalEvent::Conflict,
            ),
            (MoveState::Restoring, false, false) => (
                MoveState::Quarantined,
                Some("found at neither its path nor its quarantine path after an interruption".to_owned()),
                JournalEvent::Conflict,
            ),
            _ => continue,
        };
        if let Some(j) = journal.as_mut() {
            j.record(
                event,
                m,
                text.clone()
                    .or_else(|| Some("reconciled after an interruption".into())),
            )?;
        }
        catalog.record(m.file, state, text.clone())?;
        log::warn!(
            "cleanup run {}: {} reconciled as {}",
            cfg.run,
            m.rel_path,
            state.as_str()
        );
        report.resolved.push((m.file, state, text));
    }
    Ok(report)
}
