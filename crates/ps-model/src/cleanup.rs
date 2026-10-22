//! Cleanup runs and their moves (ADR-0024, ADR-0034, ADR-0061): the records `ps-catalog` stores and
//! `ps-export::quarantine` acts on.

use serde::{Deserialize, Serialize};

use crate::{ContentId, Disposition, FileId, RelPath, SourceId};

/// Where one planned move stands (catalog codes 0-5, ADR-0061; never reuse a value).
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
}

impl CleanupRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            CleanupRunState::Planned => "planned",
            CleanupRunState::Running => "running",
            CleanupRunState::Completed => "completed",
            CleanupRunState::Undone => "undone",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planned" => CleanupRunState::Planned,
            "running" => CleanupRunState::Running,
            "completed" => CleanupRunState::Completed,
            "undone" => CleanupRunState::Undone,
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
