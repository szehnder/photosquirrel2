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
//! - in a group none of whose keeper-set files has a valid library copy but another member does (the keeper
//!   changed after that member was exported, ADR-0092): the members with a valid copy are `exported`; the
//!   keeper set and every member ranked above the best-ranked exported member are `superseded` (better than
//!   anything of the group in the library: not accounted for, never cleanup-eligible, ADR-0093), with that
//!   copy's path; the members ranked below it are `covered` by it (a keeper set with only some files in the
//!   library is not covered: pending, the plan completes it);
//! - not in a group and its content has a valid library copy: `exported`;
//! - moved by a cleanup run (ADR-0034): `quarantined`, with the disposition the rules below give it as if it
//!   were still at its path (`prior`) - checked first, since the scanner sees it as gone. Only while that
//!   prior disposition is exported or covered, that is while a valid library copy covers it: a quarantined
//!   file whose library copy is gone or differs is `pending` again, with its `quarantine_path`, even when it
//!   is excluded, so it is not accounted for and export copies it from the quarantine folder (ADR-0066,
//!   ADR-0067);
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
    /// Moved into its source's quarantine folder by a cleanup run (ADR-0034): the quarantine path, relative
    /// to the source root.
    pub quarantined: Option<RelPath>,
}

/// One duplicate group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupFacts {
    pub members: Vec<FileId>,
    /// The keeper first, then its RAW+JPEG companion (ranking order).
    pub keeper_set: Vec<FileId>,
    // `members` are in ranking order (best first): a member before the best-ranked exported member is better
    // than every copy of the group in the library (ADR-0093).
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
    /// Quarantined files: the disposition the file would have at its original path (ADR-0034).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior: Option<Disposition>,
    /// Quarantined files: where it is, relative to its source root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quarantine_path: Option<RelPath>,
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
    // Whether a group's path is its keeper set's (`false`: an earlier member's copy covers it).
    let mut by_keeper: Vec<bool> = Vec::with_capacity(facts.groups.len());
    // Groups covered by an earlier copy: the rank (index in `members`) of the best-ranked exported member.
    let mut covering_rank: Vec<usize> = Vec::with_capacity(facts.groups.len());
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
        let kp = paths.filter(|p| !p.is_empty()).map(|mut p| p.remove(0));
        by_keeper.push(kp.is_some());
        let covering = if kp.is_none() && g.keeper_set.iter().all(|k| exported_path(*k).is_none()) {
            g.members
                .iter()
                .enumerate()
                .find_map(|(i, m)| exported_path(*m).map(|p| (i, p)))
        } else {
            None
        };
        covering_rank.push(covering.as_ref().map_or(0, |(i, _)| *i));
        keeper_path.push(kp.or(covering.map(|(_, p)| p)));
    }

    let d = |f: &FileFacts, disposition, reason, library_path| FileDisposition {
        file: f.file,
        disposition,
        reason,
        library_path,
        prior: None,
        quarantine_path: None,
    };
    let derive = |f: &FileFacts| -> FileDisposition {
        if let Some(&gi) = group_of.get(&f.file) {
            if let Some(kp) = &keeper_path[gi] {
                if !by_keeper[gi] {
                    // In the library through an earlier member's copy (ADR-0092): what ranks below that copy is
                    // covered by it; the keeper set and what ranks above it are better than anything of the
                    // group in the library, so superseded, never covered (ADR-0093).
                    let g = &facts.groups[gi];
                    let better = g.keeper_set.contains(&f.file)
                        || g.members
                            .iter()
                            .position(|m| *m == f.file)
                            .is_some_and(|r| r < covering_rank[gi]);
                    return match exported_path(f.file) {
                        Some(own) => d(f, Disposition::Exported, None, Some(own)),
                        None if better => d(f, Disposition::Superseded, None, Some(kp.clone())),
                        None => d(f, Disposition::Covered, None, Some(kp.clone())),
                    };
                }
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
                Some(PhotoClass::ThumbnailCache) if !in_group => Some(ExclusionReason::CacheCopy),
                _ => None,
            }
        };
        match reason {
            Some(r) => d(f, Disposition::Excluded, Some(r), None),
            None => d(f, Disposition::Pending, None, None),
        }
    };
    facts
        .files
        .iter()
        .map(|f| match &f.quarantined {
            Some(q) => {
                let at_home = FileFacts {
                    missing: false,
                    ..f.clone()
                };
                let prior = derive(&at_home);
                // Accounted for only while a valid library copy covers it, whatever its exclusion state
                // (ADR-0067): an excluded file in quarantine whose library copy is gone has its only copy
                // there, so it is pending like any other.
                if prior.disposition == Disposition::Superseded {
                    // Better than the group's library copy and only in the quarantine folder (moved before
                    // ADR-0093): superseded, with where it is; `export run --upgrade` copies it from there.
                    FileDisposition {
                        quarantine_path: Some(q.clone()),
                        ..prior
                    }
                } else if prior.disposition.is_accounted_safe() {
                    FileDisposition {
                        disposition: Disposition::Quarantined,
                        prior: Some(prior.disposition),
                        quarantine_path: Some(q.clone()),
                        ..prior
                    }
                } else {
                    FileDisposition {
                        disposition: Disposition::Pending,
                        reason: None,
                        library_path: None,
                        prior: None,
                        quarantine_path: Some(q.clone()),
                        ..prior
                    }
                }
            }
            None => derive(f),
        })
        .collect()
}

/// "100% accounted for" or what is outstanding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Badge {
    FullyAccounted,
    Outstanding {
        pending: u64,
        unreadable: u64,
        /// Of `pending`, the files whose only copy is in a quarantine folder.
        #[serde(default)]
        in_quarantine: u64,
        /// Better copies not in the library yet (ADR-0093).
        #[serde(default)]
        superseded: u64,
    },
}

impl Badge {
    pub fn of(c: &CompletionCounts) -> Badge {
        if c.fully_accounted() {
            Badge::FullyAccounted
        } else {
            Badge::Outstanding {
                pending: c.files_of(Disposition::Pending),
                unreadable: c.files_of(Disposition::Unreadable),
                in_quarantine: c.pending_in_quarantine,
                superseded: c.files_of(Disposition::Superseded),
            }
        }
    }

    pub fn label(self) -> String {
        match self {
            Badge::FullyAccounted => "100% accounted for".into(),
            Badge::Outstanding {
                pending,
                unreadable,
                in_quarantine,
                superseded,
            } => {
                let mut parts = Vec::new();
                if pending > 0 || superseded == 0 {
                    let mut s = format!("{pending} pending");
                    if in_quarantine > 0 {
                        s.push_str(&format!(
                            " ({in_quarantine} only in the quarantine folder, library copy gone)"
                        ));
                    }
                    parts.push(s);
                }
                if superseded > 0 {
                    parts.push(superseded_label(superseded));
                }
                let mut s = parts.join(", ");
                if unreadable > 0 {
                    s.push_str(&format!(", {unreadable} unreadable"));
                }
                s
            }
        }
    }
}

/// "N better copies not in the library yet", the superseded files of a report (ADR-0093).
pub fn superseded_label(n: u64) -> String {
    if n == 1 {
        "1 better copy not in the library yet".into()
    } else {
        format!("{n} better copies not in the library yet")
    }
}

/// What to do about superseded files, for reports.
pub const SUPERSEDED_HINT: &str = "the library holds only a lesser copy of these photos; `export run --upgrade` \
     (or the export panel's \"Also copy the better copies\") copies them beside it. Cleanup never moves them.";

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

/// Count one file's disposition (and whether it is pending only in a quarantine folder).
pub fn count_disposition(c: &mut CompletionCounts, d: &FileDisposition, size: u64) {
    c.add(d.disposition, size);
    if d.disposition == Disposition::Pending && d.quarantine_path.is_some() {
        c.add_pending_in_quarantine();
    }
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
        count_disposition(&mut acc.0, d, size);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ps_model::{ExportProfile, LedgerKind, LedgerState};

    fn rel(s: &str) -> RelPath {
        RelPath::from_bytes(s.as_bytes().to_vec()).expect("rel")
    }

    fn file(i: i64) -> FileFacts {
        FileFacts {
            file: FileId(i),
            source: SourceId(1),
            rel_path: rel(&format!("a/{i}.jpg")),
            size: 10,
            content: Some(ContentId::full([i as u8; 32])),
            class: Some(PhotoClass::Photo),
            unreadable: false,
            missing: true,
            user_excluded: false,
            quarantined: Some(rel(&format!(".photosquirrel-quarantine/1/a/{i}.jpg"))),
        }
    }

    fn row(i: i64) -> LedgerEntry {
        LedgerEntry {
            content: ContentId::full([i as u8; 32]),
            full_hash: None,
            file: FileId(i),
            library_rel_path: rel(&format!("2020/{i}.jpg")),
            size: 10,
            state: LedgerState::Done,
            profile: ExportProfile::Lightroom,
            at_ms: 0,
            library_mtime_ns: None,
            kind: LedgerKind::File,
        }
    }

    /// Quarantined files that are (or became) excluded: by the user, by a reclassification, or as a member of
    /// a group whose keeper the user excluded. With their library copy valid they are quarantined; with it
    /// gone they are pending in quarantine, never accounted for (ADR-0067).
    #[test]
    fn excluded_quarantined_files_need_a_valid_library_copy() {
        let files = vec![
            FileFacts {
                user_excluded: true,
                ..file(1)
            },
            FileFacts {
                class: Some(PhotoClass::Junk),
                ..file(2)
            },
            FileFacts {
                user_excluded: true,
                ..file(3)
            },
            file(4),
        ];
        // 3 is the keeper of a group, 4 its copy.
        let groups = vec![GroupFacts {
            members: vec![FileId(3), FileId(4)],
            keeper_set: vec![FileId(3)],
        }];
        let ledger: Vec<LedgerEntry> = [1, 2, 3].map(row).into();
        let derive = |checks: &HashMap<RelPath, CopyCheck>| {
            derive_dispositions(&DispositionFacts {
                files: &files,
                groups: &groups,
                ledger: &ledger,
                checks,
            })
        };
        let ok: HashMap<RelPath, CopyCheck> = ledger
            .iter()
            .map(|e| (e.library_rel_path.clone(), CopyCheck::Ok))
            .collect();
        for d in derive(&ok) {
            assert_eq!(d.disposition, Disposition::Quarantined, "{d:?}");
        }
        let gone: HashMap<RelPath, CopyCheck> = ledger
            .iter()
            .map(|e| (e.library_rel_path.clone(), CopyCheck::Missing))
            .collect();
        let d = derive(&gone);
        let mut counts = CompletionCounts::default();
        for (f, x) in files.iter().zip(&d) {
            assert_eq!(x.disposition, Disposition::Pending, "{x:?}");
            assert_eq!(x.reason, None, "{x:?}");
            assert_eq!(x.quarantine_path, f.quarantined, "{x:?}");
            count_disposition(&mut counts, x, f.size);
        }
        assert!(!counts.fully_accounted());
        assert_eq!(counts.pending_in_quarantine, 4);
    }
}
