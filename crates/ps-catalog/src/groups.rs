//! Duplicate groups, suggestions and user constraints (catalog migration 4, ADR-0013/0014/0033).
//!
//! Groups are derived: the regroup after a scan computes them (`ps-dedup`) and stores only what changed with
//! [`Writer::apply_groups`], in one transaction. User constraints (keeper overrides, separations, merges) are
//! keyed by content identity and survive every rescan and regroup.
//!
//! This module also serves the grouping inputs: [`Readers::dedup_stamps`] (a cheap per-file change stamp, so a
//! long-lived grouping engine only reloads what changed) and [`Readers::analyzed_files`] (records with full
//! metadata).

use std::rc::Rc;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use ps_model::{ContentId, FileId, Metadata};

use crate::error::{CatalogError, Result};
use crate::model::{FileRecord, codes, now_ms};
use crate::read::{FILE_COLUMN_COUNT, FILE_COLUMNS, Readers, file_record};
use crate::writer::Writer;

/// How a member joined its group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberEdge {
    /// The keeper: root of the explanation tree.
    Keeper,
    Exact,
    Derivative,
    UserMerge,
}

impl MemberEdge {
    fn code(self) -> i64 {
        match self {
            MemberEdge::Keeper => 0,
            MemberEdge::Exact => 1,
            MemberEdge::Derivative => 2,
            MemberEdge::UserMerge => 3,
        }
    }

    fn from_code(c: i64) -> Option<Self> {
        Some(match c {
            0 => MemberEdge::Keeper,
            1 => MemberEdge::Exact,
            2 => MemberEdge::Derivative,
            3 => MemberEdge::UserMerge,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MemberEdge::Keeper => "keeper",
            MemberEdge::Exact => "exact",
            MemberEdge::Derivative => "derivative",
            MemberEdge::UserMerge => "user_merge",
        }
    }
}

/// One stored member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRow {
    pub file: FileId,
    /// Position in the best-copy ranking (0 = keeper).
    pub rank: u32,
    pub edge: MemberEdge,
    /// The member this one joined through.
    pub via: Option<FileId>,
    pub phash_distance: Option<u8>,
    pub dhash_distance: Option<u8>,
    pub in_keeper_set: bool,
}

/// One stored group. `id` is the smallest member file id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupRow {
    pub id: FileId,
    pub keeper: FileId,
    pub display: FileId,
    pub keeper_set: Vec<FileId>,
    /// JSON array of `{"text": ..., "reason": ...}` (opaque to the catalog).
    pub reasons: serde_json::Value,
    pub reclaimable_bytes: u64,
    /// Digest of the stored form; the regroup skips unchanged groups.
    pub digest: [u8; 32],
    /// Ordered by rank.
    pub members: Vec<MemberRow>,
    pub reviewed_at_ms: Option<i64>,
}

impl GroupRow {
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// The stored keeper reasons as text.
    pub fn reason_texts(&self) -> Vec<String> {
        reason_texts(&self.reasons)
    }

    /// The member the keeper reasons compare against ([`reason_runner_up`]).
    pub fn runner_up(&self) -> Option<FileId> {
        reason_runner_up(&self.reasons)
    }
}

/// The member a stored reasons array was explained against: the first reason's `runner_up` (ADR-0074), absent
/// in rows stored before it.
pub fn reason_runner_up(v: &serde_json::Value) -> Option<FileId> {
    v.as_array()?
        .first()?
        .get("runner_up")?
        .as_i64()
        .map(FileId)
}

/// The `text` fields of a stored reasons array.
pub fn reason_texts(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r.get("text").and_then(|t| t.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A group in a listing (no members).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupSummary {
    pub id: FileId,
    pub keeper: FileId,
    pub display: FileId,
    pub member_count: u32,
    pub reclaimable_bytes: u64,
    pub reviewed_at_ms: Option<i64>,
}

/// A stored similar-shot suggestion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SuggestionRow {
    /// `file_a < file_b`.
    pub file_a: FileId,
    pub file_b: FileId,
    pub phash_distance: u8,
    pub dhash_distance: u8,
    /// `{"text": ..., "reason": ...}` (opaque to the catalog).
    pub reason: serde_json::Value,
}

/// What a regroup changed. Applied in one transaction: deletions first, then inserts and replacements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupChanges {
    /// Delete every stored group and suggestion first (a full rewrite).
    pub clear: bool,
    pub removed_groups: Vec<FileId>,
    /// Inserted, or replacing the stored group with the same id.
    pub groups: Vec<GroupRow>,
    pub removed_suggestions: Vec<(FileId, FileId)>,
    pub suggestions: Vec<SuggestionRow>,
}

impl GroupChanges {
    pub fn is_empty(&self) -> bool {
        !self.clear
            && self.removed_groups.is_empty()
            && self.groups.is_empty()
            && self.removed_suggestions.is_empty()
            && self.suggestions.is_empty()
    }
}

/// The user's grouping constraints (ADR-0014, ADR-0033), keyed by content identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintRows {
    /// Content chosen as keeper, and the copy that was chosen.
    pub keeper_overrides: Vec<(ContentId, Option<FileId>)>,
    /// Ordered pairs (smaller first).
    pub separations: Vec<(ContentId, ContentId)>,
    pub merges: Vec<(ContentId, ContentId)>,
}

fn ordered(a: ContentId, b: ContentId) -> (ContentId, ContentId) {
    if a <= b { (a, b) } else { (b, a) }
}

fn kind_code(c: &ContentId) -> i64 {
    codes::identity_kind(c.kind)
}

fn content_from(digest: Vec<u8>, kind: i64) -> Result<ContentId> {
    let digest: [u8; 32] = digest
        .try_into()
        .map_err(|_| CatalogError::Corrupt("content id".into()))?;
    Ok(ContentId {
        digest,
        kind: codes::identity_kind_from(kind),
    })
}

fn json_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

fn corrupt(what: &str) -> CatalogError {
    CatalogError::Corrupt(what.to_owned())
}

fn delete_group(c: &Connection, id: FileId) -> rusqlite::Result<()> {
    // Members go with ON DELETE CASCADE; delete them explicitly too in case foreign keys are off.
    c.prepare_cached("DELETE FROM dup_member WHERE group_id = ?1")?
        .execute([id.0])?;
    c.prepare_cached("DELETE FROM dup_group WHERE id = ?1")?
        .execute([id.0])?;
    Ok(())
}

fn insert_group(c: &Connection, g: &GroupRow) -> rusqlite::Result<()> {
    let set: Vec<i64> = g.keeper_set.iter().map(|f| f.0).collect();
    c.prepare_cached(
        "INSERT INTO dup_group (id, keeper_file_id, display_file_id, keeper_set, reasons, member_count,
                                reclaimable_bytes, digest, reviewed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?
    .execute(params![
        g.id.0,
        g.keeper.0,
        g.display.0,
        serde_json::to_string(&set).map_err(json_err)?,
        g.reasons.to_string(),
        g.members.len() as i64,
        g.reclaimable_bytes as i64,
        &g.digest[..],
        g.reviewed_at_ms,
    ])?;
    let mut stmt = c.prepare_cached(
        "INSERT INTO dup_member (file_id, group_id, rank, edge_kind, via_file_id, phash_d, dhash_d,
                                 in_keeper_set)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for m in &g.members {
        stmt.execute(params![
            m.file.0,
            g.id.0,
            i64::from(m.rank),
            m.edge.code(),
            m.via.map(|v| v.0),
            m.phash_distance,
            m.dhash_distance,
            m.in_keeper_set,
        ])?;
    }
    Ok(())
}

fn apply(c: &Connection, ch: &GroupChanges) -> rusqlite::Result<()> {
    if ch.clear {
        c.execute_batch(
            "DELETE FROM dup_member; DELETE FROM dup_group; DELETE FROM dup_suggestion;",
        )?;
    }
    for id in &ch.removed_groups {
        delete_group(c, *id)?;
    }
    // Keep the review time of a group that is replaced.
    let mut reviewed: Vec<Option<i64>> = Vec::with_capacity(ch.groups.len());
    for g in &ch.groups {
        let r: Option<i64> = c
            .prepare_cached("SELECT reviewed_at FROM dup_group WHERE id = ?1")?
            .query_row([g.id.0], |r| r.get(0))
            .optional()?
            .flatten();
        reviewed.push(r.or(g.reviewed_at_ms));
        delete_group(c, g.id)?;
    }
    for (g, r) in ch.groups.iter().zip(reviewed) {
        if r == g.reviewed_at_ms {
            insert_group(c, g)?;
        } else {
            let mut g = g.clone();
            g.reviewed_at_ms = r;
            insert_group(c, &g)?;
        }
    }
    for (a, b) in &ch.removed_suggestions {
        c.prepare_cached("DELETE FROM dup_suggestion WHERE file_a = ?1 AND file_b = ?2")?
            .execute([a.0, b.0])?;
    }
    for s in &ch.suggestions {
        c.prepare_cached(
            "INSERT INTO dup_suggestion (file_a, file_b, phash_d, dhash_d, reason) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (file_a, file_b) DO UPDATE SET
                phash_d = excluded.phash_d, dhash_d = excluded.dhash_d, reason = excluded.reason",
        )?
        .execute(params![
            s.file_a.0,
            s.file_b.0,
            s.phash_distance,
            s.dhash_distance,
            s.reason.to_string()
        ])?;
    }
    Ok(())
}

impl Writer {
    /// Store a regroup's changes atomically and return once committed.
    pub fn apply_groups(&self, changes: GroupChanges) -> Result<()> {
        self.exec_constraints(move |c| apply(c, &changes))
    }

    /// Make `content` the keeper of its group (`chosen` is the copy the user picked). Other overrides among
    /// `replace` (the contents of the group's other members) are dropped, so a group has one override.
    pub fn set_keeper_override(
        &self,
        content: ContentId,
        chosen: Option<FileId>,
        replace: Vec<ContentId>,
    ) -> Result<()> {
        self.exec_constraints(move |c| {
            for r in &replace {
                c.prepare_cached(
                    "DELETE FROM keeper_override WHERE content_id = ?1 AND content_kind = ?2",
                )?
                .execute(params![&r.digest[..], kind_code(r)])?;
            }
            c.prepare_cached(
                "INSERT INTO keeper_override (content_id, content_kind, chosen_file_id, at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (content_id, content_kind) DO UPDATE SET
                    chosen_file_id = excluded.chosen_file_id, at = excluded.at",
            )?
            .execute(params![
                &content.digest[..],
                kind_code(&content),
                chosen.map(|f| f.0),
                now_ms()
            ])?;
            Ok(())
        })
    }

    /// Drop the keeper overrides of `contents` (back to the automatic choice).
    pub fn clear_keeper_overrides(&self, contents: Vec<ContentId>) -> Result<()> {
        self.exec_constraints(move |c| {
            for r in &contents {
                c.prepare_cached(
                    "DELETE FROM keeper_override WHERE content_id = ?1 AND content_kind = ?2",
                )?
                .execute(params![&r.digest[..], kind_code(r)])?;
            }
            Ok(())
        })
    }

    /// Add (`add == true`) or remove "never pair" constraints between content pairs (ADR-0033).
    pub fn set_separations(&self, pairs: Vec<(ContentId, ContentId)>, add: bool) -> Result<()> {
        self.exec_constraints(move |c| pair_rows(c, "dup_separation", &pairs, add))
    }

    /// Add or remove "always pair" constraints (user merges).
    pub fn set_merges(&self, pairs: Vec<(ContentId, ContentId)>, add: bool) -> Result<()> {
        self.exec_constraints(move |c| pair_rows(c, "dup_merge", &pairs, add))
    }

    /// Mark a group reviewed (or not).
    pub fn set_group_reviewed(&self, group: FileId, reviewed: bool) -> Result<()> {
        self.exec(move |c| {
            c.prepare_cached("UPDATE dup_group SET reviewed_at = ?2 WHERE id = ?1")?
                .execute(params![group.0, reviewed.then(now_ms)])?;
            Ok(())
        })
    }
}

fn pair_rows(
    c: &Connection,
    table: &'static str,
    pairs: &[(ContentId, ContentId)],
    add: bool,
) -> rusqlite::Result<()> {
    for &(a, b) in pairs {
        if a == b {
            continue;
        }
        let (a, b) = ordered(a, b);
        let p = params![&a.digest[..], kind_code(&a), &b.digest[..], kind_code(&b)];
        if add {
            c.prepare_cached(&format!(
                "INSERT INTO {table} (content_a, kind_a, content_b, kind_b, at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT DO NOTHING"
            ))?
            .execute(params![
                &a.digest[..],
                kind_code(&a),
                &b.digest[..],
                kind_code(&b),
                now_ms()
            ])?;
        } else {
            c.prepare_cached(&format!(
                "DELETE FROM {table} WHERE content_a = ?1 AND kind_a = ?2 AND content_b = ?3 AND kind_b = ?4"
            ))?
            .execute(p)?;
        }
    }
    Ok(())
}

/// FNV-1a, 64 bit: a cheap, stable change stamp.
struct Stamp(u64);

impl Stamp {
    fn new() -> Self {
        Stamp(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }

    fn int(&mut self, v: Option<i64>) {
        match v {
            Some(v) => self.bytes(&v.to_le_bytes()),
            None => self.bytes(&[]),
        }
    }
}

/// Grouping inputs: analyzed files with a content identity that are present in their source, or that a
/// cleanup run moved into its quarantine folder (migration 14, ADR-0034: a quarantined file is accounted for,
/// so it keeps its place in its group - a moved keeper stays the keeper and its copies stay covered - instead
/// of dropping out like a file that is gone).
/// Quarantined files are matched by source and path
/// ([`file_is_quarantined_sql!`](crate::cleanup::file_is_quarantined_sql), ADR-0063).
const INPUT_WHERE: &str = concat!(
    "stage IN (2, 3) AND content_id IS NOT NULL AND (missing_since IS NULL OR ",
    crate::cleanup::file_is_quarantined_sql!(),
    ")"
);

fn summary(r: &Row<'_>) -> rusqlite::Result<GroupSummary> {
    Ok(GroupSummary {
        id: FileId(r.get(0)?),
        keeper: FileId(r.get(1)?),
        display: FileId(r.get(2)?),
        member_count: r.get::<_, i64>(3)? as u32,
        reclaimable_bytes: r.get::<_, i64>(4)? as u64,
        reviewed_at_ms: r.get(5)?,
    })
}

/// Sort order of [`Readers::groups`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupOrder {
    /// Most reclaimable bytes first.
    #[default]
    Reclaimable,
    /// By id (smallest member file id).
    Id,
}

impl Readers {
    /// A change stamp per grouping input (analyzed, present files with a content identity), ordered by id.
    /// The stamp covers every column grouping and ranking read except the metadata JSON, which only changes
    /// together with them (re-analysis follows a fingerprint change). Not the capture date columns: grouping
    /// and ranking read the embedded capture time from the metadata, so re-dating a row (ADR-0071) is no
    /// grouping change.
    pub fn dedup_stamps(&self) -> Result<Vec<(FileId, u64)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT id, source_id, rel_path, size, mtime_ns, class, format, width, height, content_id,
                        identity_kind, phash, dhash, hash_version
                 FROM file WHERE {INPUT_WHERE} ORDER BY id"
            ))?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let mut s = Stamp::new();
                s.int(r.get(1)?);
                s.bytes(r.get_ref(2)?.as_blob().map_err(rusqlite::Error::from)?);
                for i in [3, 4, 5] {
                    s.int(r.get(i)?);
                }
                s.bytes(r.get::<_, Option<String>>(6)?.unwrap_or_default().as_bytes());
                s.int(r.get(7)?);
                s.int(r.get(8)?);
                s.bytes(r.get_ref(9)?.as_blob().map_err(rusqlite::Error::from)?);
                for i in 10..=13 {
                    s.int(r.get(i)?);
                }
                out.push((FileId(r.get(0)?), s.0));
            }
            Ok(out)
        })
    }

    /// Grouping inputs with their metadata: all of them (`ids == None`, ordered by id) or the given ids
    /// (those that are still inputs). Calls `f` per file without collecting.
    pub fn analyzed_files(
        &self,
        ids: Option<&[FileId]>,
        mut f: impl FnMut(FileRecord, Option<Metadata>),
    ) -> Result<()> {
        self.with(|c| {
            let mut run = |stmt: &mut rusqlite::CachedStatement<'_>,
                           p: &[&dyn rusqlite::ToSql]|
             -> Result<()> {
                let mut rows = stmt.query(p)?;
                while let Some(r) = rows.next()? {
                    let rec = file_record(r)?;
                    let meta = match r
                        .get_ref(FILE_COLUMN_COUNT)?
                        .as_str_or_null()
                        .map_err(rusqlite::Error::from)?
                    {
                        Some(j) => Some(
                            serde_json::from_str::<Metadata>(j)
                                .map_err(|e| CatalogError::Corrupt(e.to_string()))?,
                        ),
                        None => None,
                    };
                    f(rec, meta);
                }
                Ok(())
            };
            match ids {
                None => {
                    let mut stmt = c.prepare_cached(&format!(
                        "SELECT {FILE_COLUMNS}, metadata FROM file WHERE {INPUT_WHERE} ORDER BY id"
                    ))?;
                    run(&mut stmt, &[])
                }
                Some(ids) => {
                    let mut stmt = c.prepare_cached(&format!(
                        "SELECT {FILE_COLUMNS}, metadata FROM file
                         WHERE id IN rarray(?1) AND {INPUT_WHERE} ORDER BY id"
                    ))?;
                    for chunk in ids.chunks(4096) {
                        let v: Rc<Vec<Value>> =
                            Rc::new(chunk.iter().map(|i| Value::Integer(i.0)).collect());
                        run(&mut stmt, &[&v])?;
                    }
                    Ok(())
                }
            }
        })
    }

    /// The stored user constraints.
    pub fn group_constraints(&self) -> Result<ConstraintRows> {
        self.with(|c| {
            let mut out = ConstraintRows::default();
            let mut stmt = c.prepare_cached(
                "SELECT content_id, content_kind, chosen_file_id FROM keeper_override ORDER BY at, content_id",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                out.keeper_overrides.push((
                    content_from(r.get(0)?, r.get(1)?)?,
                    r.get::<_, Option<i64>>(2)?.map(FileId),
                ));
            }
            for (table, dst) in [
                ("dup_separation", &mut out.separations),
                ("dup_merge", &mut out.merges),
            ] {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT content_a, kind_a, content_b, kind_b FROM {table}
                     ORDER BY content_a, kind_a, content_b, kind_b"
                ))?;
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    dst.push((
                        content_from(r.get(0)?, r.get(1)?)?,
                        content_from(r.get(2)?, r.get(3)?)?,
                    ));
                }
            }
            Ok(out)
        })
    }

    /// Stored group ids and digests, by id.
    pub fn group_digests(&self) -> Result<Vec<(FileId, [u8; 32])>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached("SELECT id, digest FROM dup_group ORDER BY id")?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let d: Vec<u8> = r.get(1)?;
                out.push((
                    FileId(r.get(0)?),
                    d.try_into().map_err(|_| corrupt("group digest"))?,
                ));
            }
            Ok(out)
        })
    }

    /// Stored suggestion pairs.
    pub fn suggestion_pairs(&self) -> Result<Vec<(FileId, FileId)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT file_a, file_b FROM dup_suggestion ORDER BY file_a, file_b",
            )?;
            let rows = stmt.query_map([], |r| Ok((FileId(r.get(0)?), FileId(r.get(1)?))))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Number of groups, files in groups and reclaimable bytes.
    pub fn group_totals(&self) -> Result<(u64, u64, u64)> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT count(*), coalesce(sum(member_count), 0), coalesce(sum(reclaimable_bytes), 0)
                 FROM dup_group",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)? as u64,
                        r.get::<_, i64>(1)? as u64,
                        r.get::<_, i64>(2)? as u64,
                    ))
                },
            )?)
        })
    }

    /// A page of groups.
    pub fn groups(&self, order: GroupOrder, offset: u64, limit: u64) -> Result<Vec<GroupSummary>> {
        self.with(|c| {
            let ord = match order {
                GroupOrder::Reclaimable => "reclaimable_bytes DESC, id",
                GroupOrder::Id => "id",
            };
            let mut stmt = c.prepare_cached(&format!(
                "SELECT id, keeper_file_id, display_file_id, member_count, reclaimable_bytes, reviewed_at
                 FROM dup_group ORDER BY {ord} LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit as i64, offset as i64], summary)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// One group with its members (by rank).
    pub fn group(&self, id: FileId) -> Result<Option<GroupRow>> {
        self.with(|c| {
            let head = c
                .prepare_cached(
                    "SELECT id, keeper_file_id, display_file_id, keeper_set, reasons, reclaimable_bytes,
                            digest, reviewed_at
                     FROM dup_group WHERE id = ?1",
                )?
                .query_row([id.0], |r| {
                    Ok((
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, Vec<u8>>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                    ))
                })
                .optional()?;
            let Some((keeper, display, set, reasons, reclaimable, digest, reviewed)) = head else {
                return Ok(None);
            };
            let set: Vec<i64> =
                serde_json::from_str(&set).map_err(|e| CatalogError::Corrupt(e.to_string()))?;
            let reasons: serde_json::Value =
                serde_json::from_str(&reasons).map_err(|e| CatalogError::Corrupt(e.to_string()))?;
            let mut stmt = c.prepare_cached(
                "SELECT file_id, rank, edge_kind, via_file_id, phash_d, dhash_d, in_keeper_set
                 FROM dup_member WHERE group_id = ?1 ORDER BY rank",
            )?;
            let mut rows = stmt.query([id.0])?;
            let mut members = Vec::new();
            while let Some(r) = rows.next()? {
                members.push(MemberRow {
                    file: FileId(r.get(0)?),
                    rank: r.get::<_, i64>(1)? as u32,
                    edge: MemberEdge::from_code(r.get(2)?).ok_or_else(|| corrupt("edge kind"))?,
                    via: r.get::<_, Option<i64>>(3)?.map(FileId),
                    phash_distance: r.get(4)?,
                    dhash_distance: r.get(5)?,
                    in_keeper_set: r.get(6)?,
                });
            }
            Ok(Some(GroupRow {
                id,
                keeper: FileId(keeper),
                display: FileId(display),
                keeper_set: set.into_iter().map(FileId).collect(),
                reasons,
                reclaimable_bytes: reclaimable as u64,
                digest: digest.try_into().map_err(|_| corrupt("group digest"))?,
                members,
                reviewed_at_ms: reviewed,
            }))
        })
    }

    /// The group a file belongs to.
    pub fn group_of_file(&self, f: FileId) -> Result<Option<FileId>> {
        self.with(|c| {
            Ok(
                c.prepare_cached("SELECT group_id FROM dup_member WHERE file_id = ?1")?
                    .query_row([f.0], |r| Ok(FileId(r.get(0)?)))
                    .optional()?,
            )
        })
    }

    /// Every group's members and keeper set (dispositions, export planning), by group id.
    pub fn group_memberships(&self) -> Result<Vec<(FileId, Vec<FileId>, Vec<FileId>)>> {
        self.with(|c| {
            let mut out: Vec<(FileId, Vec<FileId>, Vec<FileId>)> = Vec::new();
            let mut stmt = c.prepare_cached(
                "SELECT group_id, file_id, in_keeper_set FROM dup_member ORDER BY group_id, rank",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let g = FileId(r.get(0)?);
                let f = FileId(r.get(1)?);
                let k: bool = r.get(2)?;
                if out.last().is_none_or(|l| l.0 != g) {
                    out.push((g, Vec::new(), Vec::new()));
                }
                if let Some(l) = out.last_mut() {
                    l.1.push(f);
                    if k {
                        l.2.push(f);
                    }
                }
            }
            Ok(out)
        })
    }

    /// Suggestions involving `f`, or all of them (`None`), up to `limit`.
    pub fn suggestions(&self, f: Option<FileId>, limit: u64) -> Result<Vec<SuggestionRow>> {
        self.with(|c| {
            let sql = match f {
                Some(_) => {
                    "SELECT file_a, file_b, phash_d, dhash_d, reason FROM dup_suggestion
                     WHERE file_a = ?1 OR file_b = ?1 ORDER BY file_a, file_b LIMIT ?2"
                }
                None => {
                    "SELECT file_a, file_b, phash_d, dhash_d, reason FROM dup_suggestion
                     WHERE ?1 IS NULL ORDER BY file_a, file_b LIMIT ?2"
                }
            };
            let mut stmt = c.prepare_cached(sql)?;
            let mut rows = stmt.query(params![f.map(|f| f.0), limit as i64])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let reason: String = r.get(4)?;
                out.push(SuggestionRow {
                    file_a: FileId(r.get(0)?),
                    file_b: FileId(r.get(1)?),
                    phash_distance: r.get(2)?,
                    dhash_distance: r.get(3)?,
                    reason: serde_json::from_str(&reason)
                        .map_err(|e| CatalogError::Corrupt(e.to_string()))?,
                });
            }
            Ok(out)
        })
    }

    /// Number of stored suggestions.
    pub fn suggestion_count(&self) -> Result<u64> {
        self.with(|c| {
            Ok(c.query_row("SELECT count(*) FROM dup_suggestion", [], |r| {
                r.get::<_, i64>(0)
            })? as u64)
        })
    }
}
