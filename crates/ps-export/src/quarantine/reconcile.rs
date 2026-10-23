//! Crash reconciliation (ADR-0061, ADR-0064): moves an interruption left mid-way are resolved from the file
//! system, checked against the run's `journal.jsonl`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use ps_model::{CleanupMove, FileId, MoveState};
use serde::Serialize;

use super::CleanupJournal;
use super::journal::{JOURNAL_FILE, Journal, JournalEvent, JournalLine, read_header, read_journal};
use super::run::{RunConfig, identity_of};
use super::run_dir_rel;

/// How each interrupted move was resolved.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReconcileReport {
    /// (file, new state, note).
    pub resolved: Vec<(FileId, MoveState, Option<String>)>,
}

/// The move's file is at `p`: a regular file with the move's size and content identity (re-hashed; the
/// size alone could be another file).
fn holds(p: &Path, m: &CleanupMove) -> bool {
    fs::symlink_metadata(p).is_ok_and(|md| md.file_type().is_file() && md.len() == m.size)
        && identity_of(p, m.content.kind, m.size).is_ok_and(|c| c == m.content)
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The line is about `m`: same file id and same original path (ids alone can be reused).
fn is_about(l: &JournalLine, m: &CleanupMove) -> bool {
    if l.file != Some(m.file.0) {
        return false;
    }
    match (&l.original_bytes, &l.original) {
        (Some(hex), _) => hex_decode(hex).is_some_and(|b| b == m.rel_path.as_bytes()),
        (None, Some(o)) => o.as_bytes() == m.rel_path.as_bytes(),
        (None, None) => false,
    }
}

/// The last move event the JSON journal recorded for each file of the run (none when the journal is absent,
/// unreadable or belongs to another run).
fn journal_events(
    run_dir: &Path,
    cfg: &RunConfig<'_>,
    moves: &[CleanupMove],
) -> HashMap<FileId, JournalEvent> {
    let path = run_dir.join(JOURNAL_FILE);
    let ours = read_header(&path)
        .ok()
        .flatten()
        .is_some_and(|h| h.uid.as_deref() == Some(cfg.uid) && h.run == cfg.run);
    if !ours {
        return HashMap::new();
    }
    let by_id: HashMap<i64, &CleanupMove> = moves.iter().map(|m| (m.file.0, m)).collect();
    let mut out = HashMap::new();
    for l in read_journal(&path).unwrap_or_default() {
        let Some(m) = l.file.and_then(|f| by_id.get(&f)) else {
            continue;
        };
        if l.event != JournalEvent::Run && is_about(&l, m) {
            out.insert(m.file, l.event);
        }
    }
    out
}

/// Resolve every move of the run an interruption may have left with a catalog state that does not match the
/// file system:
///
/// - `moving`: in quarantine only -> `quarantined`; at its path only -> `planned` (the rename never happened;
///   a later run moves it); both or neither -> `skipped` with the reason, nothing touched;
/// - `restoring`: at its path only -> `restored`; in quarantine only -> `quarantined`; both or neither ->
///   `quarantined` with the reason (undo reports it again);
/// - `planned`, `skipped` or `unselected` while `journal.jsonl` records an intent or a move for it (the catalog lost a
///   commit, e.g. on power loss): in quarantine only -> `quarantined`; anything else is left as it is;
/// - `quarantined` while `journal.jsonl` records an undo intent or a restore for it: at its path only ->
///   `restored`; anything else is left as it is.
///
/// "At a path" means a regular file of the move's size whose content identity re-hashes to the move's.
/// Writes the outcome to both journals (the JSON one only if the run folder exists).
pub fn reconcile(
    cfg: &RunConfig<'_>,
    moves: &[CleanupMove],
    catalog: &mut dyn CleanupJournal,
) -> std::io::Result<ReconcileReport> {
    let root = cfg.source_root;
    let run_dir = run_dir_rel(cfg.run).under(root);
    let events = if run_dir.is_dir() {
        journal_events(&run_dir, cfg, moves)
    } else {
        HashMap::new()
    };
    let mut journal: Option<Journal> = None;
    let mut report = ReconcileReport::default();
    for m in moves {
        let journaled = events.get(&m.file).copied();
        let moved_per_journal =
            matches!(journaled, Some(JournalEvent::Intent | JournalEvent::Done));
        let restored_per_journal = matches!(
            journaled,
            Some(JournalEvent::UndoIntent | JournalEvent::Restored)
        );
        let suspect = match m.state {
            MoveState::Moving | MoveState::Restoring => true,
            // Unselected moves are never attempted, but a catalog that lost commits may say unselected for a
            // file the JSON journal records as moved (ADR-0068): treated like planned.
            MoveState::Planned | MoveState::Skipped | MoveState::Unselected => moved_per_journal,
            MoveState::Quarantined => restored_per_journal,
            MoveState::Restored => false,
        };
        if !suspect {
            continue;
        }
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
            (MoveState::Planned | MoveState::Skipped | MoveState::Unselected, false, true) => (
                MoveState::Quarantined,
                Some(
                    "found in quarantine as the journal file records; the catalog had lost the move".to_owned(),
                ),
                JournalEvent::Done,
            ),
            (MoveState::Quarantined, true, false) => (
                MoveState::Restored,
                Some(
                    "found back at its path as the journal file records; the catalog had lost the undo".to_owned(),
                ),
                JournalEvent::Restored,
            ),
            _ => continue,
        };
        if journal.is_none() && run_dir.is_dir() {
            journal = Journal::open(&run_dir, cfg.run, cfg.uid, root).ok();
        }
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
