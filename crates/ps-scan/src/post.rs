//! The post-scan stage: incremental duplicate regrouping (ADR-0013, ADR-0014, ADR-0033; duplicate-groups
//! design "Incremental").
//!
//! [`GroupEngine`] is long-lived (one per [`crate::Scanner`]). Its first refresh loads every grouping input from
//! the catalog, groups them and stores the result, writing only groups that differ from what the catalog
//! already holds. Later refreshes read a cheap change stamp per file ([`ps_catalog::Readers::dedup_stamps`]),
//! reload only new and changed files, update the `ps-dedup` indexes in place and recompute only the affected
//! groups ([`ps_dedup::Grouper::regroup_live`]); the delta is stored atomically through the catalog writer.
//! Constraint changes (keeper overrides, separations, merges) are picked up the same way.
//!
//! Nothing here touches source files: it reads and writes the catalog only.

use std::collections::HashMap;
use std::time::Instant;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use ps_catalog::{
    Catalog, ConstraintRows, FileRecord, GroupChanges, GroupRow, MemberEdge, MemberRow,
    SuggestionRow,
};
use ps_dedup::{
    Constraints, DedupInput, DedupMeta, EdgeKind, Group, Grouper, GroupingDelta, LiveGrouping,
    MemberReason, Suggestion,
};
use ps_model::{FileId, Metadata};

use crate::error::Result;

/// What one refresh did (stored in the run summary as `grouping`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegroupReport {
    /// The engine grouped everything (first refresh of this process).
    pub full: bool,
    /// Grouping inputs (analyzed, present files).
    pub inputs: u64,
    /// New, changed or removed inputs plus files of changed constraints.
    pub changed: u64,
    pub groups: u64,
    pub grouped_files: u64,
    pub suggestions: u64,
    /// Groups written (inserted or replaced) and deleted in the catalog.
    pub groups_written: u64,
    pub groups_removed: u64,
    /// Time spent grouping (in memory), and in total.
    pub group_ms: u64,
    pub elapsed_ms: u64,
}

struct State {
    grouper: Grouper<'static>,
    live: LiveGrouping,
    stamps: HashMap<FileId, u64>,
    grouped_files: u64,
}

/// The long-lived regroup engine of one catalog.
#[derive(Default)]
pub struct GroupEngine {
    state: Mutex<Option<State>>,
}

impl std::fmt::Debug for GroupEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.state.lock();
        f.debug_struct("GroupEngine")
            .field("loaded", &s.is_some())
            .field("inputs", &s.as_ref().map_or(0, |s| s.grouper.len()))
            .finish()
    }
}

/// The `ps-dedup` constraints stored in the catalog.
pub fn constraints_from(rows: &ConstraintRows) -> Constraints {
    let mut c = Constraints::default();
    for (content, _) in &rows.keeper_overrides {
        c.override_keeper(*content);
    }
    for &(a, b) in &rows.separations {
        c.separate(a, b);
    }
    for &(a, b) in &rows.merges {
        c.merge(a, b);
    }
    c
}

/// The grouping input of an analyzed file. The path is `<source id>/<relative path>`, so path depth and the
/// RAW+JPEG same-folder rule do not depend on where a source is mounted.
pub fn dedup_input(rec: &FileRecord, meta: Option<&Metadata>) -> Option<DedupInput> {
    let format = rec.format?;
    let class = rec.class?;
    let default = Metadata::default();
    let m = meta.unwrap_or(&default);
    let mut dm = DedupMeta::from_media(&m.fields, m.likely_camera_original);
    if dm.width.is_none() || dm.height.is_none() {
        dm.width = rec.width;
        dm.height = rec.height;
    }
    Some(DedupInput {
        file: rec.id,
        content: rec.identity,
        hash: rec.hashes,
        format,
        class,
        path: format!("{}/{}", rec.key.source.0, rec.key.path.display()),
        size: rec.stat.size,
        mtime_ns: rec.stat.mtime_ns,
        meta: dm,
    })
}

fn load_inputs(catalog: &Catalog, ids: Option<&[FileId]>) -> Result<Vec<DedupInput>> {
    let mut out = Vec::new();
    catalog.read().analyzed_files(ids, |rec, meta| {
        if let Some(x) = dedup_input(&rec, meta.as_ref()) {
            out.push(x);
        }
    })?;
    Ok(out)
}

fn reasons_json(g: &Group) -> serde_json::Value {
    serde_json::Value::Array(
        g.keeper_reasons
            .iter()
            .map(|r| {
                serde_json::json!({
                    "text": r.to_string(),
                    "reason": serde_json::to_value(r).unwrap_or(serde_json::Value::Null),
                })
            })
            .collect(),
    )
}

/// The catalog row of a group.
pub fn group_row(g: &Group) -> GroupRow {
    let digest = *blake3::hash(&serde_json::to_vec(g).unwrap_or_default()).as_bytes();
    let rank: HashMap<FileId, u32> = g
        .ranking
        .iter()
        .enumerate()
        .map(|(i, f)| (*f, i as u32))
        .collect();
    let mut members: Vec<MemberRow> = g
        .members
        .iter()
        .map(|m| {
            let (edge, via, p, d) = match m.reason {
                MemberReason::Keeper => (MemberEdge::Keeper, None, None, None),
                MemberReason::Joined { edge, via } => match edge {
                    EdgeKind::Exact => (MemberEdge::Exact, Some(via), None, None),
                    EdgeKind::Derivative {
                        phash_distance,
                        dhash_distance,
                    } => (
                        MemberEdge::Derivative,
                        Some(via),
                        Some(phash_distance),
                        Some(dhash_distance),
                    ),
                    EdgeKind::UserMerge => (MemberEdge::UserMerge, Some(via), None, None),
                },
            };
            MemberRow {
                file: m.file,
                rank: rank.get(&m.file).copied().unwrap_or(u32::MAX),
                edge,
                via,
                phash_distance: p,
                dhash_distance: d,
                in_keeper_set: g.keeper_set.contains(&m.file),
            }
        })
        .collect();
    members.sort_by_key(|m| (m.rank, m.file));
    GroupRow {
        id: g.members[0].file,
        keeper: g.keeper,
        display: g.display,
        keeper_set: g.keeper_set.clone(),
        reasons: reasons_json(g),
        reclaimable_bytes: g.reclaimable_bytes,
        digest,
        members,
        reviewed_at_ms: None,
    }
}

/// The catalog row of a suggestion.
pub fn suggestion_row(s: &Suggestion) -> SuggestionRow {
    SuggestionRow {
        file_a: s.file_a,
        file_b: s.file_b,
        phash_distance: s.phash_distance,
        dhash_distance: s.dhash_distance,
        reason: serde_json::json!({
            "text": s.reason.to_string(),
            "reason": serde_json::to_value(&s.reason).unwrap_or(serde_json::Value::Null),
        }),
    }
}

fn changes_from(delta: &GroupingDelta) -> GroupChanges {
    GroupChanges {
        clear: false,
        removed_groups: delta.removed_groups.clone(),
        groups: delta.groups.iter().map(group_row).collect(),
        removed_suggestions: delta.removed_suggestions.clone(),
        suggestions: delta.suggestions.iter().map(suggestion_row).collect(),
    }
}

impl GroupEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the in-memory state; the next refresh regroups everything (and still writes only differences).
    pub fn reset(&self) {
        *self.state.lock() = None;
    }

    /// Bring the stored groups up to date with the catalog's files and constraints.
    pub fn refresh(&self, catalog: &Catalog) -> Result<RegroupReport> {
        let started = Instant::now();
        let mut slot = self.state.lock();
        let constraints = constraints_from(&catalog.read().group_constraints()?);
        let stamps: HashMap<FileId, u64> = catalog.read().dedup_stamps()?.into_iter().collect();
        let mut report = RegroupReport::default();
        let changes = match slot.as_mut() {
            None => {
                report.full = true;
                let inputs = load_inputs(catalog, None)?;
                report.changed = inputs.len() as u64;
                let t = Instant::now();
                let grouper = Grouper::owned(inputs, constraints);
                let grouping = grouper.group_all();
                report.group_ms = t.elapsed().as_millis() as u64;
                let live = LiveGrouping::from_grouping(&grouping);
                let changes = full_changes(catalog, &grouping)?;
                *slot = Some(State {
                    grouper,
                    live,
                    stamps: HashMap::new(),
                    grouped_files: 0,
                });
                changes
            }
            Some(st) => {
                let mut changed: Vec<FileId> = stamps
                    .iter()
                    .filter(|(f, s)| st.stamps.get(f) != Some(s))
                    .map(|(f, _)| *f)
                    .collect();
                changed.sort();
                let removed: Vec<FileId> = st
                    .stamps
                    .keys()
                    .filter(|f| !stamps.contains_key(f))
                    .copied()
                    .collect();
                let reload = load_inputs(catalog, Some(&changed))?;
                let t = Instant::now();
                let loaded: std::collections::HashSet<FileId> =
                    reload.iter().map(|x| x.file).collect();
                for x in reload {
                    st.grouper.upsert(x);
                }
                for f in changed.iter().filter(|f| !loaded.contains(f)) {
                    st.grouper.remove(*f);
                }
                for f in &removed {
                    st.grouper.remove(*f);
                }
                let mut all = changed;
                all.extend(removed);
                if *st.grouper.constraints() != constraints {
                    all.extend(st.grouper.set_constraints(constraints));
                }
                all.sort();
                all.dedup();
                report.changed = all.len() as u64;
                let delta = st.grouper.regroup_live(&mut st.live, &all);
                report.group_ms = t.elapsed().as_millis() as u64;
                changes_from(&delta)
            }
        };
        report.groups_written = changes.groups.len() as u64;
        report.groups_removed = changes.removed_groups.len() as u64;
        if !changes.is_empty() {
            catalog.writer().apply_groups(changes)?;
        }
        if let Some(st) = slot.as_mut() {
            st.stamps = stamps;
            st.grouped_files = st.live.groups().map(|g| g.members.len() as u64).sum();
            report.inputs = st.grouper.len() as u64;
            report.groups = st.live.len() as u64;
            report.grouped_files = st.grouped_files;
            report.suggestions = st.live.suggestions().count() as u64;
        }
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        log::info!(
            "regrouped {} of {} inputs into {} groups ({} written, {} removed) in {} ms",
            report.changed,
            report.inputs,
            report.groups,
            report.groups_written,
            report.groups_removed,
            report.elapsed_ms
        );
        Ok(report)
    }
}

/// The changes that make the stored groups equal `grouping`, writing only what differs.
fn full_changes(catalog: &Catalog, grouping: &ps_dedup::Grouping) -> Result<GroupChanges> {
    let stored: HashMap<FileId, [u8; 32]> = catalog.read().group_digests()?.into_iter().collect();
    let mut changes = GroupChanges::default();
    let mut keep = std::collections::HashSet::new();
    for g in &grouping.groups {
        let row = group_row(g);
        keep.insert(row.id);
        if stored.get(&row.id) != Some(&row.digest) {
            changes.groups.push(row);
        }
    }
    changes.removed_groups = stored
        .keys()
        .filter(|k| !keep.contains(k))
        .copied()
        .collect();
    changes.removed_groups.sort();
    let stored_pairs: std::collections::HashSet<(FileId, FileId)> =
        catalog.read().suggestion_pairs()?.into_iter().collect();
    let mut want = std::collections::HashSet::new();
    for s in &grouping.suggestions {
        want.insert((s.file_a, s.file_b));
        if !stored_pairs.contains(&(s.file_a, s.file_b)) {
            changes.suggestions.push(suggestion_row(s));
        }
    }
    changes.removed_suggestions = stored_pairs.difference(&want).copied().collect();
    changes.removed_suggestions.sort();
    Ok(changes)
}
