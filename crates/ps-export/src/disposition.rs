//! Derived dispositions and completion reports (ADR-0023).
//!
//! Pure over its inputs: the catalogued files, the duplicate groups (members and keeper sets), the export
//! ledger and a check of each ledger row's library copy ([`check_library`]: existence and size, plus a re-hash
//! in verify mode). Nothing is ever set by hand. The SQL form (`file_disposition` view over `file`,
//! `dup_member`, `dup_group`, `user_inclusion` and `export_ledger`) needs the dedup migration and is wired at
//! integration; it must agree with [`derive_dispositions`].
//!
//! Rules, first match wins:
//! - in a group whose whole keeper set has a valid library copy: keeper-set files are `exported`, the other
//!   members `covered` (with the keeper's library path);
//! - not in a group and its content has a valid library copy: `exported`;
//! - gone from the source: `missing`; failed after retries: `unreadable`;
//! - user-excluded, a member of a group whose keeper the user excluded (the photo itself was excluded, every
//!   copy goes with it; the export skips such groups), or (in no group) junk, graphic or a cache thumbnail:
//!   `excluded` with the reason;
//! - everything else (keepers and candidates not exported, members of groups whose keeper is not exported):
//!   `pending`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ps_model::{
    CompletionCounts, ContentId, Disposition, ExclusionReason, FileId, LedgerEntry, PhotoClass,
    RelPath, SourceId,
};
use serde::Serialize;

use crate::library::{FsLibrary, check_copy};

/// One catalogued file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    pub file: FileId,
    pub source: SourceId,
    pub rel_path: RelPath,
    pub size: u64,
    pub content: Option<ContentId>,
    pub class: Option<PhotoClass>,
    /// Failed after retries.
    pub unreadable: bool,
    /// Gone from the source.
    pub missing: bool,
    pub user_excluded: bool,
}

/// One duplicate group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupFacts {
    pub members: Vec<FileId>,
    /// The keeper first, then its RAW+JPEG companion (ranking order).
    pub keeper_set: Vec<FileId>,
}

/// State of a ledger row's library copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CopyCheck {
    Ok,
    Missing,
    SizeMismatch {
        expected: u64,
        found: u64,
    },
    /// Verify mode: the bytes changed.
    HashMismatch,
}

/// What is wrong with a library copy, in a few words.
pub fn copy_check_text(c: CopyCheck) -> String {
    match c {
        CopyCheck::Ok => "ok".into(),
        CopyCheck::Missing => "missing from the library".into(),
        CopyCheck::SizeMismatch { expected, found } => format!(
            "changed in the library, probably by another app ({expected} bytes when exported, {found} now)"
        ),
        CopyCheck::HashMismatch => "contents changed in the library since it was exported".into(),
    }
}

/// Inputs of [`derive_dispositions`].
#[derive(Clone, Copy, Debug)]
pub struct DispositionFacts<'a> {
    pub files: &'a [FileFacts],
    pub groups: &'a [GroupFacts],
    pub ledger: &'a [LedgerEntry],
    /// [`check_library`] result per `done` row path; rows without a check count as invalid.
    pub checks: &'a HashMap<RelPath, CopyCheck>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileDisposition {
    pub file: FileId,
    pub disposition: Disposition,
    pub reason: Option<ExclusionReason>,
    /// The library copy of this file (exported) or of its keeper (covered).
    pub library_path: Option<RelPath>,
}

/// Check the library copy of every `done` row with [`check_copy`]: present with its recorded size (and, when
/// recorded, modification time; a changed time re-hashes that copy); with `verify`, re-hash every copy. A copy
/// that cannot be read counts as missing.
pub fn check_library(
    library: &Path,
    ledger: &[LedgerEntry],
    verify: bool,
) -> HashMap<RelPath, CopyCheck> {
    let probe = FsLibrary::new(library);
    ledger
        .iter()
        .filter(|e| e.is_exported_file())
        .map(|e| {
            let check = check_copy(&probe, e, verify).unwrap_or(CopyCheck::Missing);
            (e.library_rel_path.clone(), check)
        })
        .collect()
}

/// Derive every file's disposition (rules in the module docs). Output order follows `facts.files`.
pub fn derive_dispositions(facts: &DispositionFacts<'_>) -> Vec<FileDisposition> {
    // Contents with a valid library copy -> its path.
    let mut valid: HashMap<ContentId, RelPath> = HashMap::new();
    for e in facts.ledger.iter().filter(|e| e.is_exported_file()) {
        if facts.checks.get(&e.library_rel_path) == Some(&CopyCheck::Ok) {
            valid
                .entry(e.content)
                .or_insert_with(|| e.library_rel_path.clone());
        }
    }
    let by_id: HashMap<FileId, &FileFacts> = facts.files.iter().map(|f| (f.file, f)).collect();
    let exported_path = |id: FileId| {
        by_id
            .get(&id)
            .and_then(|f| f.content)
            .and_then(|c| valid.get(&c).cloned())
    };
    // Group index and whether its keeper set is safely exported (with the keeper's path).
    let mut group_of: HashMap<FileId, usize> = HashMap::new();
    let mut keeper_path: Vec<Option<RelPath>> = Vec::with_capacity(facts.groups.len());
    let excluded_keeper: Vec<bool> = facts
        .groups
        .iter()
        .map(|g| {
            g.keeper_set
                .first()
                .and_then(|k| by_id.get(k))
                .is_some_and(|f| f.user_excluded)
        })
        .collect();
    for (gi, g) in facts.groups.iter().enumerate() {
        for m in &g.members {
            group_of.insert(*m, gi);
        }
        let paths: Option<Vec<RelPath>> = g.keeper_set.iter().map(|k| exported_path(*k)).collect();
        keeper_path.push(paths.filter(|p| !p.is_empty()).map(|mut p| p.remove(0)));
    }

    let d = |f: &FileFacts, disposition, reason, library_path| FileDisposition {
        file: f.file,
        disposition,
        reason,
        library_path,
    };
    facts
        .files
        .iter()
        .map(|f| {
            if let Some(&gi) = group_of.get(&f.file) {
                if let Some(kp) = &keeper_path[gi] {
                    return if facts.groups[gi].keeper_set.contains(&f.file) {
                        let own = exported_path(f.file).unwrap_or_else(|| kp.clone());
                        d(f, Disposition::Exported, None, Some(own))
                    } else {
                        d(f, Disposition::Covered, None, Some(kp.clone()))
                    };
                }
            } else if let Some(p) = f.content.and_then(|c| valid.get(&c)) {
                return d(f, Disposition::Exported, None, Some(p.clone()));
            }
            if f.missing {
                return d(f, Disposition::Missing, None, None);
            }
            if f.unreadable {
                return d(f, Disposition::Unreadable, None, None);
            }
            let in_group = group_of.contains_key(&f.file);
            // A group member is never excluded by class: its group's keeper decides (pending until exported,
            // excluded with it when the user excluded the keeper).
            let reason = if f.user_excluded
                || group_of.get(&f.file).is_some_and(|gi| excluded_keeper[*gi])
            {
                Some(ExclusionReason::UserExcluded)
            } else if in_group {
                None
            } else {
                match f.class {
                    Some(PhotoClass::Junk) => Some(ExclusionReason::Junk),
                    Some(PhotoClass::Graphic) => Some(ExclusionReason::Graphic),
                    Some(PhotoClass::ThumbnailCache) if !in_group => {
                        Some(ExclusionReason::CacheCopy)
                    }
                    _ => None,
                }
            };
            match reason {
                Some(r) => d(f, Disposition::Excluded, Some(r), None),
                None => d(f, Disposition::Pending, None, None),
            }
        })
        .collect()
}

/// "100% accounted for" or what is outstanding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Badge {
    FullyAccounted,
    Outstanding { pending: u64, unreadable: u64 },
}

impl Badge {
    pub fn of(c: &CompletionCounts) -> Badge {
        if c.fully_accounted() {
            Badge::FullyAccounted
        } else {
            Badge::Outstanding {
                pending: c.files_of(Disposition::Pending),
                unreadable: c.files_of(Disposition::Unreadable),
            }
        }
    }

    pub fn label(self) -> String {
        match self {
            Badge::FullyAccounted => "100% accounted for".into(),
            Badge::Outstanding {
                pending,
                unreadable: 0,
            } => format!("{pending} pending"),
            Badge::Outstanding {
                pending,
                unreadable,
            } => format!("{pending} pending, {unreadable} unreadable"),
        }
    }
}

/// Excluded files and bytes per reason.
pub type ExcludedBreakdown = BTreeMap<ExclusionReason, (u64, u64)>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FolderReport {
    /// The folder (`None` = the source root), direct files only.
    pub folder: Option<RelPath>,
    pub counts: CompletionCounts,
    pub excluded: ExcludedBreakdown,
    pub badge: Badge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceReport {
    pub source: SourceId,
    pub counts: CompletionCounts,
    pub excluded: ExcludedBreakdown,
    pub badge: Badge,
    /// Sorted by folder.
    pub folders: Vec<FolderReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompletionReport {
    /// Sorted by source id.
    pub sources: Vec<SourceReport>,
    /// Library copies that are missing or differ (their files are pending again).
    pub mismatches: Vec<(RelPath, CopyCheck)>,
}

/// Aggregate dispositions per source and folder.
pub fn report(
    files: &[FileFacts],
    dispositions: &[FileDisposition],
    checks: &HashMap<RelPath, CopyCheck>,
) -> CompletionReport {
    type Acc = (CompletionCounts, ExcludedBreakdown);
    let mut per: BTreeMap<SourceId, (Acc, BTreeMap<Option<RelPath>, Acc>)> = BTreeMap::new();
    let add = |acc: &mut Acc, d: &FileDisposition, size: u64| {
        acc.0.add(d.disposition, size);
        if let Some(r) = d.reason {
            let e = acc.1.entry(r).or_default();
            e.0 += 1;
            e.1 += size;
        }
    };
    for (f, d) in files.iter().zip(dispositions) {
        let (src, folders) = per.entry(f.source).or_default();
        add(src, d, f.size);
        add(folders.entry(f.rel_path.parent()).or_default(), d, f.size);
    }
    let sources = per
        .into_iter()
        .map(|(source, ((counts, excluded), folders))| SourceReport {
            source,
            badge: Badge::of(&counts),
            counts,
            excluded,
            folders: folders
                .into_iter()
                .map(|(folder, (counts, excluded))| FolderReport {
                    folder,
                    badge: Badge::of(&counts),
                    counts,
                    excluded,
                })
                .collect(),
        })
        .collect();
    let mut mismatches: Vec<_> = checks
        .iter()
        .filter(|(_, c)| **c != CopyCheck::Ok)
        .map(|(p, c)| (p.clone(), *c))
        .collect();
    mismatches.sort_by(|a, b| a.0.cmp(&b.0));
    CompletionReport {
        sources,
        mismatches,
    }
}
