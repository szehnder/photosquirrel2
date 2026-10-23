//! Duplicate groups (specs `duplicate-detection`, `best-copy`; ADR-0013, ADR-0014, ADR-0033).
//!
//! [`Groups`] reads the groups the post-scan regroup stored in the catalog and applies the user's corrections:
//! a keeper override, separating a member, merging two groups. Corrections are stored as constraints keyed by
//! content identity, so they survive restarts, rescans, moves and regrouping; each one regroups the affected
//! files right away. A group's id is its smallest member file id.
//!
//! Every call reads or writes the catalog and may regroup: call from a background thread, never the UI thread.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use ps_catalog::groups::reason_texts;
pub use ps_catalog::{GroupOrder, MemberEdge};
use ps_catalog::{GroupRow, Readers, SuggestionRow};
use ps_model::{ContentId, FileId, Format, PhotoClass, SourceId, path_from_bytes};
pub use ps_scan::RegroupReport;

use crate::scan::ScanService;

/// Errors of the groups facade.
#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    #[error(transparent)]
    Scan(#[from] ps_scan::ScanError),
    #[error(transparent)]
    Catalog(#[from] ps_catalog::CatalogError),
    #[error("unknown duplicate group {0}")]
    NoSuchGroup(FileId),
    #[error("file {file} is not a member of group {group}")]
    NotAMember { file: FileId, group: FileId },
    #[error("file {0} has no content identity")]
    NoContent(FileId),
    #[error(
        "file {file} cannot be separated from group {group}: every other member is a byte-identical copy"
    )]
    OnlyExactCopies { file: FileId, group: FileId },
    #[error("cannot merge group {0} with itself")]
    SameGroup(FileId),
}

pub type Result<T, E = GroupError> = std::result::Result<T, E>;

/// Totals over every group.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupTotals {
    pub groups: u64,
    pub grouped_files: u64,
    pub reclaimable_bytes: u64,
    pub suggestions: u64,
}

/// One group in a listing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupListItem {
    pub id: FileId,
    pub keeper: FileId,
    pub keeper_path: PathBuf,
    pub member_count: u32,
    pub reclaimable_bytes: u64,
    pub reviewed: bool,
}

/// One member with why it is in the group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupMember {
    pub file: FileId,
    pub source: SourceId,
    /// Absolute path.
    pub path: PathBuf,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub format: Option<Format>,
    pub class: Option<PhotoClass>,
    /// Best-copy rank (0 = keeper).
    pub rank: u32,
    pub keeper: bool,
    pub in_keeper_set: bool,
    /// `keeper`, `exact`, `derivative` or `user_merge`.
    pub edge: MemberEdge,
    pub via: Option<FileId>,
    pub phash_distance: Option<u8>,
    pub dhash_distance: Option<u8>,
    /// Human-readable join reason ("exact copy of 12", "same image at another size ... of 12").
    pub reason: String,
}

/// A similar-shot suggestion (never collapsed into a group).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SuggestionInfo {
    pub file_a: FileId,
    pub file_b: FileId,
    pub path_a: Option<PathBuf>,
    pub path_b: Option<PathBuf>,
    pub phash_distance: u8,
    pub dhash_distance: u8,
    pub reason: String,
}

/// A separation constraint between two members (ADR-0033).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeparationInfo {
    pub content_a: String,
    pub content_b: String,
}

/// One group with its members (best first) and the keeper reasons.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupDetail {
    pub id: FileId,
    pub keeper: FileId,
    pub display: FileId,
    pub keeper_set: Vec<FileId>,
    pub keeper_reasons: Vec<String>,
    /// The member the keeper reasons compare against (ADR-0074); `None` for groups stored before it (and
    /// single-member groups).
    #[serde(default)]
    pub runner_up: Option<FileId>,
    /// The keeper is a user override ("chosen by you").
    pub chosen_by_you: bool,
    pub reclaimable_bytes: u64,
    pub reviewed: bool,
    pub members: Vec<GroupMember>,
    /// Suggestions involving members of this group.
    pub suggestions: Vec<SuggestionInfo>,
}

impl GroupDetail {
    pub fn member(&self, f: FileId) -> Option<&GroupMember> {
        self.members.iter().find(|m| m.file == f)
    }
}

/// A group by corpus-style paths (`<source folder name>/<relative path>`), for scoring against a ground-truth
/// corpus (ADR-0025) whose sources are its `drive-*` folders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedGroup {
    pub members: Vec<String>,
    pub keeper: String,
}

/// Where a separated member ended up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeparateOutcome {
    /// The group the member is in now (`None`: a singleton).
    pub member_group: Option<FileId>,
    /// The group the rest is in now (`None` when fewer than two remain).
    pub rest_group: Option<FileId>,
    pub separations_added: usize,
    pub regroup: RegroupReport,
}

/// The groups facade of one [`ScanService`].
#[derive(Clone, Copy, Debug)]
pub struct Groups<'a> {
    svc: &'a ScanService,
}

impl ScanService {
    /// Duplicate groups of this catalog.
    pub fn groups(&self) -> Groups<'_> {
        Groups { svc: self }
    }
}

struct Paths {
    roots: HashMap<SourceId, PathBuf>,
}

impl Paths {
    fn new(r: &Readers) -> Result<Paths> {
        Ok(Paths {
            roots: r
                .sources()?
                .into_iter()
                .map(|s| (s.id, path_from_bytes(&s.root)))
                .collect(),
        })
    }
}

fn join_reason(edge: MemberEdge, via: Option<FileId>, p: Option<u8>, d: Option<u8>) -> String {
    let via = via.map_or_else(String::new, |v| format!(" of {v}"));
    match edge {
        MemberEdge::Keeper => "keeper".into(),
        MemberEdge::Exact => format!("exact copy (identical content){via}"),
        MemberEdge::Derivative => format!(
            "same image at another size or compression{via} (pHash distance {}, dHash distance {})",
            p.unwrap_or_default(),
            d.unwrap_or_default()
        ),
        MemberEdge::UserMerge => format!("merged by you{via}"),
    }
}

fn suggestion_text(s: &SuggestionRow) -> String {
    s.reason
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_owned()
}

impl<'a> Groups<'a> {
    fn read(&self) -> &'a Readers {
        self.svc.engine().catalog().read()
    }

    fn path(&self, paths: &Paths, f: FileId) -> Result<Option<(SourceId, PathBuf)>> {
        Ok(self.read().file_by_id(f)?.map(|rec| {
            let root = paths
                .roots
                .get(&rec.key.source)
                .cloned()
                .unwrap_or_default();
            (rec.key.source, rec.key.path.under(&root))
        }))
    }

    /// Regroup now (normally automatic after each completed scan and each correction).
    pub fn regroup(&self) -> Result<RegroupReport> {
        Ok(self.svc.engine().regroup()?)
    }

    pub fn totals(&self) -> Result<GroupTotals> {
        let (groups, grouped_files, reclaimable_bytes) = self.read().group_totals()?;
        Ok(GroupTotals {
            groups,
            grouped_files,
            reclaimable_bytes,
            suggestions: self.read().suggestion_count()?,
        })
    }

    /// A page of groups, most reclaimable first (or by id).
    pub fn list(&self, order: GroupOrder, offset: u64, limit: u64) -> Result<Vec<GroupListItem>> {
        let paths = Paths::new(self.read())?;
        let mut out = Vec::new();
        for s in self.read().groups(order, offset, limit)? {
            out.push(GroupListItem {
                id: s.id,
                keeper: s.keeper,
                keeper_path: self
                    .path(&paths, s.keeper)?
                    .map(|p| p.1)
                    .unwrap_or_default(),
                member_count: s.member_count,
                reclaimable_bytes: s.reclaimable_bytes,
                reviewed: s.reviewed_at_ms.is_some(),
            });
        }
        Ok(out)
    }

    /// The group a file is in.
    pub fn group_of(&self, f: FileId) -> Result<Option<FileId>> {
        Ok(self.read().group_of_file(f)?)
    }

    fn row(&self, id: FileId) -> Result<GroupRow> {
        self.read().group(id)?.ok_or(GroupError::NoSuchGroup(id))
    }

    /// One group with members, join reasons, keeper reasons and related suggestions.
    pub fn show(&self, id: FileId) -> Result<GroupDetail> {
        let g = self.row(id)?;
        let paths = Paths::new(self.read())?;
        let mut members = Vec::with_capacity(g.members.len());
        for m in &g.members {
            let rec = self.read().file_by_id(m.file)?;
            let (source, path) = self
                .path(&paths, m.file)?
                .unwrap_or((SourceId(0), PathBuf::new()));
            members.push(GroupMember {
                file: m.file,
                source,
                path,
                size: rec.as_ref().map_or(0, |r| r.stat.size),
                width: rec.as_ref().and_then(|r| r.width),
                height: rec.as_ref().and_then(|r| r.height),
                format: rec.as_ref().and_then(|r| r.format),
                class: rec.as_ref().and_then(|r| r.class),
                rank: m.rank,
                keeper: m.file == g.keeper,
                in_keeper_set: m.in_keeper_set,
                edge: m.edge,
                via: m.via,
                phash_distance: m.phash_distance,
                dhash_distance: m.dhash_distance,
                reason: join_reason(m.edge, m.via, m.phash_distance, m.dhash_distance),
            });
        }
        let mut suggestions = Vec::new();
        let mut seen = BTreeSet::new();
        for m in &g.members {
            for s in self.read().suggestions(Some(m.file), 100)? {
                if seen.insert((s.file_a, s.file_b)) {
                    suggestions.push(SuggestionInfo {
                        file_a: s.file_a,
                        file_b: s.file_b,
                        path_a: self.path(&paths, s.file_a)?.map(|p| p.1),
                        path_b: self.path(&paths, s.file_b)?.map(|p| p.1),
                        phash_distance: s.phash_distance,
                        dhash_distance: s.dhash_distance,
                        reason: suggestion_text(&s),
                    });
                }
            }
        }
        let keeper_reasons = reason_texts(&g.reasons);
        Ok(GroupDetail {
            id: g.id,
            keeper: g.keeper,
            display: g.display,
            keeper_set: g.keeper_set.clone(),
            chosen_by_you: keeper_reasons.iter().any(|r| r == "chosen by you"),
            keeper_reasons,
            runner_up: g
                .runner_up()
                .filter(|f| g.members.iter().any(|m| m.file == *f)),
            reclaimable_bytes: g.reclaimable_bytes,
            reviewed: g.reviewed_at_ms.is_some(),
            members,
            suggestions,
        })
    }

    /// Similar-shot suggestions (all, or those of one file).
    pub fn suggestions(&self, file: Option<FileId>, limit: u64) -> Result<Vec<SuggestionInfo>> {
        let paths = Paths::new(self.read())?;
        let mut out = Vec::new();
        for s in self.read().suggestions(file, limit)? {
            out.push(SuggestionInfo {
                file_a: s.file_a,
                file_b: s.file_b,
                path_a: self.path(&paths, s.file_a)?.map(|p| p.1),
                path_b: self.path(&paths, s.file_b)?.map(|p| p.1),
                phash_distance: s.phash_distance,
                dhash_distance: s.dhash_distance,
                reason: suggestion_text(&s),
            });
        }
        Ok(out)
    }

    fn content(&self, f: FileId) -> Result<ContentId> {
        self.read()
            .file_by_id(f)?
            .and_then(|r| r.identity)
            .ok_or(GroupError::NoContent(f))
    }

    fn contents(&self, g: &GroupRow) -> Result<Vec<(FileId, ContentId)>> {
        g.members
            .iter()
            .map(|m| Ok((m.file, self.content(m.file)?)))
            .collect()
    }

    /// Make `file` the keeper of `group` (ADR-0014 override, reason "chosen by you"). Persisted by content
    /// identity: it holds across restarts, rescans and regrouping while the file stays in the group.
    pub fn set_keeper(&self, group: FileId, file: FileId) -> Result<GroupDetail> {
        let g = self.row(group)?;
        if !g.members.iter().any(|m| m.file == file) {
            return Err(GroupError::NotAMember { file, group });
        }
        let contents = self.contents(&g)?;
        let chosen = self.content(file)?;
        let others: Vec<ContentId> = contents
            .iter()
            .map(|(_, c)| *c)
            .filter(|c| *c != chosen)
            .collect();
        self.svc
            .engine()
            .catalog()
            .writer()
            .set_keeper_override(chosen, Some(file), others)?;
        self.regroup()?;
        let now = self.group_of(file)?.ok_or(GroupError::NoSuchGroup(group))?;
        self.show(now)
    }

    /// Drop the keeper override of a group (back to the automatic choice).
    pub fn clear_keeper(&self, group: FileId) -> Result<GroupDetail> {
        let g = self.row(group)?;
        let contents: Vec<ContentId> = self.contents(&g)?.into_iter().map(|(_, c)| c).collect();
        self.svc
            .engine()
            .catalog()
            .writer()
            .clear_keeper_overrides(contents)?;
        self.regroup()?;
        let now = self
            .group_of(g.keeper)?
            .ok_or(GroupError::NoSuchGroup(group))?;
        self.show(now)
    }

    /// Separate `file` from its group (ADR-0033): "never pair" constraints between its content and every
    /// other content of the group, then regroup. Byte-identical copies of `file` go with it.
    pub fn separate(&self, group: FileId, file: FileId) -> Result<SeparateOutcome> {
        let g = self.row(group)?;
        if !g.members.iter().any(|m| m.file == file) {
            return Err(GroupError::NotAMember { file, group });
        }
        let contents = self.contents(&g)?;
        let c = self.content(file)?;
        let mut others: Vec<ContentId> = contents
            .iter()
            .map(|(_, x)| *x)
            .filter(|x| *x != c)
            .collect();
        others.sort();
        others.dedup();
        if others.is_empty() {
            return Err(GroupError::OnlyExactCopies { file, group });
        }
        let pairs: Vec<(ContentId, ContentId)> = others.iter().map(|o| (c, *o)).collect();
        let n = pairs.len();
        self.svc
            .engine()
            .catalog()
            .writer()
            .set_separations(pairs, true)?;
        let regroup = self.regroup()?;
        let rest = contents
            .iter()
            .find(|(_, x)| *x != c)
            .map(|(f, _)| *f)
            .and_then(|f| self.group_of(f).ok().flatten());
        Ok(SeparateOutcome {
            member_group: self.group_of(file)?,
            rest_group: rest,
            separations_added: n,
            regroup,
        })
    }

    /// Merge two groups (ADR-0033 "always pair" between their keepers' contents), then regroup. Separations
    /// win over merges.
    pub fn merge(&self, a: FileId, b: FileId) -> Result<GroupDetail> {
        if a == b {
            return Err(GroupError::SameGroup(a));
        }
        let (ga, gb) = (self.row(a)?, self.row(b)?);
        let (ca, cb) = (self.content(ga.keeper)?, self.content(gb.keeper)?);
        self.svc
            .engine()
            .catalog()
            .writer()
            .set_merges(vec![(ca, cb)], true)?;
        self.regroup()?;
        let now = self
            .group_of(ga.keeper)?
            .ok_or(GroupError::NoSuchGroup(a))?;
        self.show(now)
    }

    /// Undo the user merges inside a group: remove every "always pair" constraint between two of its members'
    /// contents, then regroup. The group falls back to what automatic matching (and separations) decide; the
    /// returned detail is the group that now holds the former keeper.
    pub fn undo_merges(&self, group: FileId) -> Result<GroupDetail> {
        let g = self.row(group)?;
        let contents: std::collections::HashSet<ContentId> =
            self.contents(&g)?.into_iter().map(|(_, c)| c).collect();
        let pairs: Vec<(ContentId, ContentId)> = self
            .read()
            .group_constraints()?
            .merges
            .into_iter()
            .filter(|(a, b)| contents.contains(a) && contents.contains(b))
            .collect();
        if !pairs.is_empty() {
            self.svc
                .engine()
                .catalog()
                .writer()
                .set_merges(pairs, false)?;
            self.regroup()?;
        }
        let now = self
            .group_of(g.keeper)?
            .ok_or(GroupError::NoSuchGroup(group))?;
        self.show(now)
    }

    /// The separations among the members' contents of a group or with a file (as hex content ids).
    pub fn separations(&self) -> Result<Vec<SeparationInfo>> {
        Ok(self
            .read()
            .group_constraints()?
            .separations
            .into_iter()
            .map(|(a, b)| SeparationInfo {
                content_a: a.to_hex(),
                content_b: b.to_hex(),
            })
            .collect())
    }

    /// Mark a group reviewed or not.
    pub fn set_reviewed(&self, group: FileId, reviewed: bool) -> Result<()> {
        self.row(group)?;
        self.svc
            .engine()
            .catalog()
            .writer()
            .set_group_reviewed(group, reviewed)?;
        Ok(())
    }

    /// Why two files are or are not the same shot (every rule's verdict, ADR-0013/0035).
    pub fn explain_pair(&self, a: FileId, b: FileId) -> Result<Vec<String>> {
        let mut inputs = Vec::new();
        self.read().analyzed_files(Some(&[a, b]), |rec, meta| {
            if let Some(x) = ps_scan::post::dedup_input(&rec, meta.as_ref()) {
                inputs.push(x);
            }
        })?;
        let find = |f: FileId| inputs.iter().find(|x| x.file == f);
        match (find(a), find(b)) {
            (Some(x), Some(y)) => Ok(ps_dedup::explain_pair(x, y).lines),
            _ => Ok(vec![
                "one of the files is not analyzed (no content identity or hashes)".into(),
            ]),
        }
    }

    /// Every group by `<source folder name>/<relative path>` (corpus scoring).
    pub fn named(&self) -> Result<Vec<NamedGroup>> {
        let r = self.read();
        let names: HashMap<SourceId, String> = r
            .sources()?
            .into_iter()
            .map(|s| {
                let root = path_from_bytes(&s.root);
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (s.id, name)
            })
            .collect();
        let name_of = |f: FileId| -> Result<String> {
            Ok(r.file_by_id(f)?
                .map(|rec| {
                    let root = names.get(&rec.key.source).map_or("", String::as_str);
                    if root.is_empty() {
                        rec.key.path.display().into_owned()
                    } else {
                        format!("{root}/{}", rec.key.path.display())
                    }
                })
                .unwrap_or_default())
        };
        let (n, _, _) = r.group_totals()?;
        let mut out = Vec::with_capacity(n as usize);
        for s in r.groups(GroupOrder::Id, 0, n)? {
            let Some(g) = r.group(s.id)? else { continue };
            out.push(NamedGroup {
                members: g
                    .members
                    .iter()
                    .map(|m| name_of(m.file))
                    .collect::<Result<_>>()?,
                keeper: name_of(g.keeper)?,
            });
        }
        Ok(out)
    }
}
