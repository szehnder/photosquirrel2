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
    /// Moved by a cleanup run into its source's quarantine folder (ADR-0034) and still safe in the library
    /// (exported or covered at its original path, or excluded); accounted for. The disposition it had before
    /// the move is kept beside it (`prior`) and comes back with undo. A quarantined file whose library copy is
    /// gone is `Pending` (with its quarantine path), not accounted for (ADR-0066).
    Quarantined,
    /// A group member that is better than every copy of its group in the library (the keeper above all), in a
    /// group the library holds only through a lesser member's earlier copy (ADR-0092): its quality is not in the
    /// library yet. Not accounted for and never cleanup-eligible; `export run --upgrade` copies it (ADR-0093).
    Superseded,
}

impl Disposition {
    pub const ALL: [Disposition; 8] = [
        Disposition::Exported,
        Disposition::Covered,
        Disposition::Excluded,
        Disposition::Pending,
        Disposition::Unreadable,
        Disposition::Missing,
        Disposition::Quarantined,
        Disposition::Superseded,
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
            Disposition::Superseded => "superseded",
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
    pub files: [u64; Disposition::ALL.len()],
    pub bytes: [u64; Disposition::ALL.len()],
    /// Of the pending files, those whose only copy is in a quarantine folder: a cleanup moved them while
    /// their library copy existed, and that copy is gone or differs now (ADR-0066). Export copies them from
    /// the quarantine folder again; undo puts them back.
    #[serde(default)]
    pub pending_in_quarantine: u64,
    /// Of `pending_in_quarantine`, those whose library copy still exists but a regroup moved them out of the group
    /// it covers: that copy is of another photo (burst frames told apart, ADR-0105).
    #[serde(default)]
    pub pending_regrouped: u64,
    /// Files catalogued as could not be read (damaged: cut short or corrupt, ADR-0115), whatever their
    /// disposition: copied as is, but the photo may be incomplete, so the report says to keep the original.
    /// [`CompletionCounts::fully_accounted`] does not read it.
    #[serde(default)]
    pub damaged: u64,
}

impl CompletionCounts {
    pub fn add(&mut self, d: Disposition, bytes: u64) {
        let i = d as usize;
        self.files[i] += 1;
        self.bytes[i] += bytes;
    }

    /// Count one more pending file that is only in a quarantine folder (also counted as pending).
    pub fn add_pending_in_quarantine(&mut self) {
        self.pending_in_quarantine += 1;
    }

    /// Add every count of `o`.
    pub fn merge(&mut self, o: &CompletionCounts) {
        for i in 0..self.files.len() {
            self.files[i] += o.files[i];
            self.bytes[i] += o.bytes[i];
        }
        self.pending_in_quarantine += o.pending_in_quarantine;
        self.pending_regrouped += o.pending_regrouped;
        self.damaged += o.damaged;
    }

    pub fn files_of(&self, d: Disposition) -> u64 {
        self.files[d as usize]
    }

    pub fn total_files(&self) -> u64 {
        self.files.iter().sum()
    }

    /// Nothing pending, superseded or unreadable: every file is exported, covered, excluded, missing or
    /// quarantined (with a valid library copy; a quarantined file without one is pending). A superseded file's
    /// quality is not in the library yet (ADR-0093).
    pub fn fully_accounted(&self) -> bool {
        self.files_of(Disposition::Pending) == 0
            && self.files_of(Disposition::Unreadable) == 0
            && self.files_of(Disposition::Superseded) == 0
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
        let mut s = CompletionCounts::default();
        s.add(Disposition::Superseded, 1);
        assert!(!s.fully_accounted(), "a better copy not in the library");
        assert!(!Disposition::Superseded.is_accounted_safe());
    }
}
