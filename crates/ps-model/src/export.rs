//! Export profiles, ledger rows and exclusion reasons (ADR-0022, ADR-0023).

use serde::{Deserialize, Serialize};

use crate::{ContentId, FileId, RelPath};

/// Target application presets (ADR-0022). Defaults can be adjusted per export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportProfile {
    /// Sidecars on, HEIC + MOV Live Photo pairs kept together.
    ApplePhotos,
    /// Sidecars off (Google Photos reads embedded metadata only).
    GooglePhotos,
    /// Sidecars on, RAW+JPEG keeper sets kept.
    Lightroom,
}

impl ExportProfile {
    pub const ALL: [ExportProfile; 3] = [
        ExportProfile::ApplePhotos,
        ExportProfile::GooglePhotos,
        ExportProfile::Lightroom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ExportProfile::ApplePhotos => "apple",
            ExportProfile::GooglePhotos => "google",
            ExportProfile::Lightroom => "lightroom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        ExportProfile::ALL.into_iter().find(|p| p.as_str() == s)
    }
}

/// State of one export ledger row (ADR-0023). Stable storage codes: 0, 1, 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerState {
    /// Recorded before the `.ps-partial` file is created; a crash leaves this row behind and the next run
    /// removes that partial file.
    InProgress,
    /// Copied (or adopted), verified and renamed into place.
    Done,
    /// The copy was aborted (hash mismatch, read error, disk full); nothing is at the target path.
    Failed,
    /// Renamed into place, but the file at the target path no longer matched the verified copy when it was
    /// re-hashed after the rename (ADR-0040): something else wrote that name. The file is kept, never
    /// deleted; the content counts as not exported and is copied again under a free name.
    Suspect,
}

impl LedgerState {
    pub fn code(self) -> i64 {
        match self {
            LedgerState::InProgress => 0,
            LedgerState::Done => 1,
            LedgerState::Failed => 2,
            LedgerState::Suspect => 3,
        }
    }

    pub fn from_code(c: i64) -> Option<Self> {
        Some(match c {
            0 => LedgerState::InProgress,
            1 => LedgerState::Done,
            2 => LedgerState::Failed,
            3 => LedgerState::Suspect,
            _ => return None,
        })
    }
}

/// One export ledger row: a catalog content exported (or being exported) to one library path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// The catalog identity of the exported content (full or sampled, ADR-0011).
    pub content: ContentId,
    /// BLAKE3 of every byte, computed while copying (always full, even for sampled identities).
    pub full_hash: Option<[u8; 32]>,
    /// The catalogued file the bytes were read from.
    pub file: FileId,
    /// Final path inside the library (`2011/2011-08-14/IMG_0042.JPG`).
    pub library_rel_path: RelPath,
    pub size: u64,
    pub state: LedgerState,
    pub profile: ExportProfile,
    /// Milliseconds since the Unix epoch.
    pub at_ms: i64,
}

/// Why a file is `excluded` (ADR-0023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// Icons, tiny images, very short videos (ADR-0015).
    Junk,
    /// Illustrations, UI assets.
    Graphic,
    /// A cache thumbnail that belongs to no duplicate group.
    CacheCopy,
    /// The user excluded it.
    UserExcluded,
}

impl ExclusionReason {
    pub const ALL: [ExclusionReason; 4] = [
        ExclusionReason::Junk,
        ExclusionReason::Graphic,
        ExclusionReason::CacheCopy,
        ExclusionReason::UserExcluded,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ExclusionReason::Junk => "junk",
            ExclusionReason::Graphic => "graphic",
            ExclusionReason::CacheCopy => "cache_copy",
            ExclusionReason::UserExcluded => "user_excluded",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_and_state_round_trip() {
        for p in ExportProfile::ALL {
            assert_eq!(ExportProfile::parse(p.as_str()), Some(p));
        }
        for s in [
            LedgerState::InProgress,
            LedgerState::Done,
            LedgerState::Failed,
            LedgerState::Suspect,
        ] {
            assert_eq!(LedgerState::from_code(s.code()), Some(s));
        }
        assert_eq!(LedgerState::from_code(9), None);
    }
}
