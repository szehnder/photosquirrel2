use serde::{Deserialize, Serialize};

/// Where a catalogued file stands, derived from the catalog and the export ledger (ADR-0023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// This exact content was copied into the library and verified.
    Exported,
    /// A duplicate or lesser copy whose group keeper was exported.
    Covered,
    /// Not worth keeping (junk, graphic, user-excluded), with a reason.
    Excluded,
    /// A keeper or candidate not yet exported.
    Pending,
    /// Could not be read after retries.
    Unreadable,
    /// Gone from the source since the last scan.
    Missing,
    /// Moved by a cleanup run into its source's quarantine folder (ADR-0034); accounted for. The disposition it
    /// had before the move is kept beside it (`prior`) and comes back with undo.
    Quarantined,
}

impl Disposition {
    pub const ALL: [Disposition; 7] = [
        Disposition::Exported,
        Disposition::Covered,
        Disposition::Excluded,
        Disposition::Pending,
        Disposition::Unreadable,
        Disposition::Missing,
        Disposition::Quarantined,
    ];

    /// Safe to quarantine during cleanup (ADR-0024).
    pub fn is_accounted_safe(self) -> bool {
        matches!(self, Disposition::Exported | Disposition::Covered)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Disposition::Exported => "exported",
            Disposition::Covered => "covered",
            Disposition::Excluded => "excluded",
            Disposition::Pending => "pending",
            Disposition::Unreadable => "unreadable",
            Disposition::Missing => "missing",
            Disposition::Quarantined => "quarantined",
        }
    }
}

/// Why a member joined a duplicate group (ADR-0013).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GroupEdge {
    /// Identical content identity.
    Exact,
    /// Same image at another size or compression.
    Derivative {
        phash_distance: u8,
        dhash_distance: u8,
    },
}

/// Counts and bytes per disposition for a source or folder (ADR-0023).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCounts {
    pub files: [u64; 7],
    pub bytes: [u64; 7],
}

impl CompletionCounts {
    pub fn add(&mut self, d: Disposition, bytes: u64) {
        let i = d as usize;
        self.files[i] += 1;
        self.bytes[i] += bytes;
    }

    pub fn files_of(&self, d: Disposition) -> u64 {
        self.files[d as usize]
    }

    pub fn total_files(&self) -> u64 {
        self.files.iter().sum()
    }

    /// Nothing pending or unreadable: every file is exported, covered, excluded, missing or quarantined.
    pub fn fully_accounted(&self) -> bool {
        self.files_of(Disposition::Pending) == 0 && self.files_of(Disposition::Unreadable) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_counts() {
        let mut c = CompletionCounts::default();
        c.add(Disposition::Exported, 10);
        c.add(Disposition::Covered, 5);
        assert!(c.fully_accounted());
        c.add(Disposition::Pending, 1);
        assert!(!c.fully_accounted());
        assert_eq!(c.total_files(), 3);
        assert_eq!(c.bytes[Disposition::Exported as usize], 10);
    }
}
