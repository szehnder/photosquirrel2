//! Cleanup runs and their moves (ADR-0024, ADR-0034, ADR-0061): the records `ps-catalog` stores and
//! `ps-export::quarantine` acts on.

use serde::{Deserialize, Serialize};

use crate::{ContentId, Disposition, FileId, RelPath, SourceId};

/// Where one planned move stands (catalog codes 0-6, ADR-0061, ADR-0068; never reuse a value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveState {
    /// In the plan; not moved.
    Planned,
    /// The intent is journaled; the rename may or may not have happened (reconciled after a crash).
    Moving,
    /// In the quarantine folder.
    Quarantined,
    /// Not moved; the note says why (changed source, missing library copy, other volume, ...).
    Skipped,
    /// The undo intent is journaled; the reverse rename may or may not have happened.
    Restoring,
    /// Back at its original path.
    Restored,
    /// In the plan, left out by the user's selection (ADR-0068): never verified, journaled or moved while
    /// it stays unselected; selectable again until the run ends.
    Unselected,
}

impl MoveState {
    pub fn code(self) -> i64 {
        match self {
            MoveState::Planned => 0,
            MoveState::Moving => 1,
            MoveState::Quarantined => 2,
            MoveState::Skipped => 3,
            MoveState::Restoring => 4,
            MoveState::Restored => 5,
            MoveState::Unselected => 6,
        }
    }

    pub fn from_code(c: i64) -> Option<Self> {
        Some(match c {
            0 => MoveState::Planned,
            1 => MoveState::Moving,
            2 => MoveState::Quarantined,
            3 => MoveState::Skipped,
            4 => MoveState::Restoring,
            5 => MoveState::Restored,
            6 => MoveState::Unselected,
            _ => return None,
        })
    }

    /// The file is (or may be, while restoring) in the quarantine folder.
    pub fn is_quarantined(self) -> bool {
        matches!(self, MoveState::Quarantined | MoveState::Restoring)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MoveState::Planned => "planned",
            MoveState::Moving => "moving",
            MoveState::Quarantined => "quarantined",
            MoveState::Skipped => "skipped",
            MoveState::Restoring => "restoring",
            MoveState::Restored => "restored",
            MoveState::Unselected => "unselected",
        }
    }
}

/// Where a cleanup run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupRunState {
    /// Planned (a dry run); nothing moved yet.
    Planned,
    /// Moving files (or interrupted while moving: reconciled on the next start).
    Running,
    /// Every planned move was attempted.
    Completed,
    /// Every quarantined file of the run was restored.
    Undone,
    /// Dropped before it moved any file (ADR-0068): kept in the history, never run, selected or undone.
    Discarded,
}

impl CleanupRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            CleanupRunState::Planned => "planned",
            CleanupRunState::Running => "running",
            CleanupRunState::Completed => "completed",
            CleanupRunState::Undone => "undone",
            CleanupRunState::Discarded => "discarded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planned" => CleanupRunState::Planned,
            "running" => CleanupRunState::Running,
            "completed" => CleanupRunState::Completed,
            "undone" => CleanupRunState::Undone,
            "discarded" => CleanupRunState::Discarded,
            _ => return None,
        })
    }
}

/// One file of a cleanup run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupMove {
    pub file: FileId,
    /// Relative to the run's source root.
    pub rel_path: RelPath,
    /// Relative to the run's source root: `.photosquirrel-quarantine/<run id>/<rel_path>`.
    pub quarantine_rel_path: RelPath,
    /// The catalogued stat fingerprint and identity the file must still have when it is moved.
    pub size: u64,
    pub mtime_ns: i64,
    pub file_index: Option<u64>,
    pub content: ContentId,
    /// `Exported` or `Covered`: what made the file eligible.
    pub disposition: Disposition,
    /// The library copy that makes the file safe (its own, or its group keeper's).
    pub covering_library_path: RelPath,
    pub state: MoveState,
    /// Why a move was skipped or an undo left the file in quarantine.
    pub note: Option<String>,
    /// When the state last changed (ms since the Unix epoch).
    pub at_ms: Option<i64>,
}

/// One cleanup run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRunRecord {
    pub id: i64,
    pub source: SourceId,
    /// When that source was added (ms since the Unix epoch): with `source`, the source's identity. A source
    /// with this id but another `added_at` is a different source (ADR-0063); `None` for runs whose source
    /// could not be identified when migration 15 ran.
    #[serde(default)]
    pub source_added_at_ms: Option<i64>,
    /// Storage bytes of the source root when the run was planned ([`crate::path_to_bytes`]).
    pub root: Vec<u8>,
    /// The export library whose copies make the files safe.
    pub library_id: i64,
    /// Only files under this folder (`None`: the whole source).
    pub folder: Option<RelPath>,
    /// Written in the run's JSON journal header; another uid in an existing run folder refuses the run.
    pub uid: String,
    pub state: CleanupRunState,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MOVES: [MoveState; 7] = [
        MoveState::Planned,
        MoveState::Moving,
        MoveState::Quarantined,
        MoveState::Skipped,
        MoveState::Restoring,
        MoveState::Restored,
        MoveState::Unselected,
    ];

    #[test]
    fn move_states_round_trip_and_keep_their_codes() {
        // Codes are stored in catalogs: 0-5 are those of migration 14, 6 is new (ADR-0068).
        let codes: Vec<i64> = ALL_MOVES.iter().map(|s| s.code()).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5, 6]);
        for s in ALL_MOVES {
            assert_eq!(MoveState::from_code(s.code()), Some(s));
            let json = serde_json::to_string(&s).unwrap_or_default();
            assert_eq!(json, format!("\"{}\"", s.as_str()));
        }
        assert_eq!(MoveState::from_code(7), None);
        assert_eq!(MoveState::from_code(-1), None);
        assert!(!MoveState::Unselected.is_quarantined());
        assert_eq!(MoveState::Unselected.as_str(), "unselected");
    }

    #[test]
    fn run_states_round_trip() {
        for s in [
            CleanupRunState::Planned,
            CleanupRunState::Running,
            CleanupRunState::Completed,
            CleanupRunState::Undone,
            CleanupRunState::Discarded,
        ] {
            assert_eq!(CleanupRunState::parse(s.as_str()), Some(s));
        }
        assert_eq!(CleanupRunState::Discarded.as_str(), "discarded");
        assert_eq!(CleanupRunState::parse("deleted"), None);
    }
}
