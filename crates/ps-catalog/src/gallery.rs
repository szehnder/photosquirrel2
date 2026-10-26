//! Gallery queries (gallery change; ADR-0015, ADR-0018, ADR-0023): user include/exclude decisions, the
//! filter bar's extra predicates and facets, and exact-duplicate stacks.
//!
//! User decisions live in `user_inclusion` ([`crate::db::GALLERY_MIGRATION`]). Until that migration is
//! registered the table is absent: [`Readers::has_user_decisions`] says so, the class filter falls back to
//! the classifier's verdict and [`crate::Command::SetUserDecision`] reports an error.

use std::collections::HashMap;
use std::rc::Rc;

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use ps_model::{
    AiKind, ExcludedListing, ExclusionCategory, ExclusionCounts, ExclusionSelection, FileId,
    Filter, IdList, ItemId, ItemSet, PhotoClass, Sort, SortField, SortKey,
};

use crate::error::{CatalogError, Result};
use crate::read::Readers;
use crate::writer::Writer;

/// What the user decided about a file. The classifier's result is kept; this wins in queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserDecision {
    /// Show it in the Library although the classifier set it aside.
    Include,
    /// Set it aside although the classifier kept it.
    Exclude,
}

impl UserDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            UserDecision::Include => "include",
            UserDecision::Exclude => "exclude",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "include" => Some(UserDecision::Include),
            "exclude" => Some(UserDecision::Exclude),
            _ => None,
        }
    }

    /// The reason shown beside the classifier's reasons ("included by you", "excluded by you: screenshot").
    pub fn reason_text(self, reason: Option<&str>) -> String {
        let base = match self {
            UserDecision::Include => "included by you",
            UserDecision::Exclude => "excluded by you",
        };
        match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => format!("{base}: {r}"),
            None => base.to_owned(),
        }
    }
}

/// A stored decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserDecisionRecord {
    pub file: FileId,
    pub decision: UserDecision,
    pub reason: Option<String>,
    pub at_ms: i64,
    /// The kind of the AI suggestion whose acceptance recorded this exclusion (`origin = 'ai'`, migration 21);
    /// `None` for the user's own decisions.
    pub ai_kind: Option<AiKind>,
}

/// One decision to store ([`Writer::set_user_decisions`]): `None` clears the file's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionWrite {
    pub file: FileId,
    pub decision: Option<UserDecision>,
    pub reason: Option<String>,
    /// Set for an exclusion recorded by accepting an AI suggestion of this kind.
    pub ai_kind: Option<AiKind>,
}

/// Store (or clear) one decision on the writer connection.
pub(crate) fn write_decision(
    c: &Connection,
    file: FileId,
    decision: Option<UserDecision>,
    reason: Option<&str>,
    ai_kind: Option<AiKind>,
    at_ms: i64,
) -> rusqlite::Result<()> {
    match decision {
        Some(d) => {
            // Only an exclusion can come from an AI suggestion.
            let ai_kind = ai_kind.filter(|_| d == UserDecision::Exclude);
            c.prepare_cached(
                "INSERT INTO user_inclusion (file_id, decision, reason, at, origin, ai_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (file_id) DO UPDATE SET
                    decision = excluded.decision, reason = excluded.reason, at = excluded.at,
                    origin = excluded.origin, ai_kind = excluded.ai_kind",
            )?
            .execute(params![
                file.0,
                d.as_str(),
                reason,
                at_ms,
                if ai_kind.is_some() { "ai" } else { "user" },
                ai_kind.map(AiKind::key)
            ])?;
        }
        None => {
            c.prepare_cached("DELETE FROM user_inclusion WHERE file_id = ?1")?
                .execute([file.0])?;
        }
    }
    Ok(())
}

impl Writer {
    /// Store many decisions in one transaction (the Excluded view's put back, ADR-0102), and commit before
    /// returning. Callers batch large sets (10,000 per call).
    pub fn set_user_decisions(&self, rows: Vec<DecisionWrite>) -> Result<()> {
        let at = crate::model::now_ms();
        self.exec_constraints(move |c| {
            for r in &rows {
                write_decision(c, r.file, r.decision, r.reason.as_deref(), r.ai_kind, at)?;
            }
            Ok(())
        })
    }
}

fn table_exists(c: &Connection, name: &str) -> Result<bool> {
    Ok(
        c.prepare_cached("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
            .query_row([name], |_| Ok(()))
            .optional()?
            .is_some(),
    )
}

impl Readers {
    /// Whether this catalog stores user decisions (migration 7 applied).
    pub fn has_user_decisions(&self) -> Result<bool> {
        self.with(|c| table_exists(c, "user_inclusion"))
    }

    /// The user's decision about a file, if any.
    pub fn user_decision(&self, id: FileId) -> Result<Option<UserDecisionRecord>> {
        self.with(|c| {
            if !table_exists(c, "user_inclusion")? {
                return Ok(None);
            }
            c.prepare_cached(
                "SELECT file_id, decision, reason, at, ai_kind FROM user_inclusion WHERE file_id = ?1",
            )?
            .query_row([id.0], decision_row)
            .optional()?
            .transpose()
        })
    }

    /// The stored decisions of `ids` (one query; files without a decision are left out).
    pub fn user_decisions_of(&self, ids: &[FileId]) -> Result<HashMap<FileId, UserDecisionRecord>> {
        let values: Rc<Vec<Value>> = Rc::new(ids.iter().map(|i| Value::Integer(i.0)).collect());
        self.with(|c| {
            if !table_exists(c, "user_inclusion")? {
                return Ok(HashMap::new());
            }
            let mut stmt = c.prepare_cached(
                "SELECT file_id, decision, reason, at, ai_kind FROM user_inclusion WHERE file_id IN rarray(?1)",
            )?;
            let rows = stmt.query_map([values], decision_row)?;
            let mut out = HashMap::new();
            for r in rows {
                let d = r.map_err(CatalogError::from).and_then(|r| r)?;
                out.insert(d.file, d);
            }
            Ok(out)
        })
    }

    /// Every stored decision, by file id.
    pub fn user_decisions(&self) -> Result<Vec<UserDecisionRecord>> {
        self.with(|c| {
            if !table_exists(c, "user_inclusion")? {
                return Ok(Vec::new());
            }
            let mut stmt = c.prepare_cached(
                "SELECT file_id, decision, reason, at, ai_kind FROM user_inclusion ORDER BY file_id",
            )?;
            let rows = stmt.query_map([], decision_row)?;
            rows.map(|r| r.map_err(CatalogError::from).and_then(|r| r))
                .collect()
        })
    }

    /// Camera models of present files with their counts, most common first (filter bar facet).
    pub fn camera_models(&self) -> Result<Vec<(String, u64)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT camera_model, count(*) AS n FROM file
                 WHERE camera_model IS NOT NULL AND kflags < 16
                 GROUP BY camera_model ORDER BY n DESC, camera_model",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Stored duplicate groups (ADR-0013) as stacks: every grouped file with its group's keeper and the
    /// group's member count. Empty until a regroup has run.
    pub fn group_stacks(&self) -> Result<Vec<(FileId, FileId, u32)>> {
        self.with(|c| {
            if !table_exists(c, "dup_member")? {
                return Ok(Vec::new());
            }
            let mut stmt = c.prepare_cached(
                "SELECT m.file_id, g.keeper_file_id, g.member_count FROM dup_member m
                 JOIN dup_group g ON g.id = m.group_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    FileId(r.get(0)?),
                    FileId(r.get(1)?),
                    r.get::<_, i64>(2)? as u32,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Exact-duplicate sets among present files: files sharing a content identity (ADR-0011), each set
    /// ordered by id. The gallery shows each set as one stack until duplicate groups are stored.
    ///
    /// The inner scan is index-only over `file_by_content` (~0.2 s for 1M files); filtering missing files
    /// there would cost a table lookup per row (~10 s cold), so presence is checked on the few candidates.
    pub fn exact_duplicates(&self) -> Result<Vec<Vec<FileId>>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, content_id FROM file
                 WHERE kflags < 16 AND content_id IN (
                     SELECT content_id FROM file INDEXED BY file_by_content
                     WHERE content_id IS NOT NULL
                     GROUP BY content_id HAVING count(*) > 1)
                 ORDER BY content_id, id",
            )?;
            let mut rows = stmt.query([])?;
            let mut out: Vec<Vec<FileId>> = Vec::new();
            let mut last: Option<Vec<u8>> = None;
            while let Some(r) = rows.next()? {
                let id = FileId(r.get(0)?);
                let digest: Vec<u8> = r.get(1)?;
                if last.as_deref() == Some(&digest[..]) {
                    if let Some(set) = out.last_mut() {
                        set.push(id);
                    }
                } else {
                    out.push(vec![id]);
                    last = Some(digest);
                }
            }
            out.retain(|set| set.len() > 1);
            Ok(out)
        })
    }
}

fn decision_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<UserDecisionRecord>> {
    let decision: String = r.get(1)?;
    Ok(match UserDecision::parse(&decision) {
        Some(d) => Ok(UserDecisionRecord {
            file: FileId(r.get(0)?),
            decision: d,
            reason: r.get(2)?,
            at_ms: r.get(3)?,
            ai_kind: r
                .get::<_, Option<String>>(4)?
                .as_deref()
                .and_then(AiKind::from_key),
        }),
        None => Err(CatalogError::Corrupt(format!("user decision {decision}"))),
    })
}

/// The class filter with user decisions applied, and the gallery's extra predicates, as `AND ...` SQL with
/// bound values. `base` already holds the conditions of every other filter field.
pub(crate) fn extend_filter_sql(
    c: &Connection,
    f: &Filter,
    sql: &mut String,
    args: &mut Vec<Value>,
) -> Result<()> {
    if let Some(classes) = &f.classes {
        let codes = |sql: &mut String, args: &mut Vec<Value>| {
            sql.push_str("(kflags & 7) IN (");
            for (i, cl) in classes.iter().enumerate() {
                sql.push_str(if i == 0 { "?" } else { ",?" });
                args.push(Value::Integer(i64::from(cl.code())));
            }
            sql.push(')');
        };
        let decisions = table_exists(c, "user_inclusion")?;
        if classes.is_empty() {
            sql.push_str(" AND 0");
        } else if !decisions {
            sql.push_str(" AND ");
            codes(sql, args);
        } else {
            // An included file counts as a photo, an excluded one as set aside.
            let mut wanted = Vec::new();
            if classes.iter().any(|c| is_library(*c)) {
                wanted.push(UserDecision::Include);
            }
            if classes.iter().any(|c| !is_library(*c)) {
                wanted.push(UserDecision::Exclude);
            }
            sql.push_str(" AND ((");
            codes(sql, args);
            sql.push_str(" AND id NOT IN (SELECT file_id FROM user_inclusion))");
            if !wanted.is_empty() {
                sql.push_str(" OR id IN (SELECT file_id FROM user_inclusion WHERE decision IN (");
                for (i, d) in wanted.iter().enumerate() {
                    sql.push_str(if i == 0 { "?" } else { ",?" });
                    args.push(Value::Text(d.as_str().to_owned()));
                }
                sql.push_str("))");
            }
            sql.push(')');
        }
    }
    if let Some(model) = &f.camera_model {
        sql.push_str(" AND camera_model = ?");
        args.push(Value::Text(model.clone()));
    }
    if let Some(gps) = f.has_gps {
        sql.push_str(" AND has_gps = ?");
        args.push(Value::Integer(i64::from(gps)));
    }
    if let Some(folder) = f
        .folder
        .as_deref()
        .map(|d| d.trim_matches('/'))
        .filter(|d| !d.is_empty())
    {
        // Paths under `folder/`: byte range [folder + '/', folder + '0') ('0' follows '/').
        let mut lo = folder.as_bytes().to_vec();
        lo.push(b'/');
        let mut hi = folder.as_bytes().to_vec();
        hi.push(b'0');
        sql.push_str(" AND rel_path >= ? AND rel_path < ?");
        args.push(Value::Blob(lo));
        args.push(Value::Blob(hi));
    }
    if let Some((start, end)) = f.captured {
        sql.push_str(" AND capture_ts >= ? AND capture_ts < ?");
        args.push(Value::Integer(start));
        args.push(Value::Integer(end));
    }
    Ok(())
}

/// Classes shown in the Library (the rest are set aside in the Excluded view).
pub fn is_library(c: PhotoClass) -> bool {
    matches!(c, PhotoClass::Photo | PhotoClass::LikelyPhoto)
}

/// `PhotoClass` helper re-exported for callers building Library/Excluded filters.
pub fn excluded_classes() -> Vec<PhotoClass> {
    vec![
        PhotoClass::ThumbnailCache,
        PhotoClass::Graphic,
        PhotoClass::Junk,
    ]
}

/// The Library's classes: photos and likely photos.
pub fn library_classes() -> Vec<PhotoClass> {
    vec![PhotoClass::Photo, PhotoClass::LikelyPhoto]
}

// ---- Excluded view (excluded-view-filters, ADR-0102) ------------------------------------------------------
//
// The effective category of a file is its exclude decision's origin ('you' or 'ai'), none when it has an include
// decision, else the classifier's `file.exclusion`. The lists and the counts come from one streaming pass over the
// excluded set ([`scan_excluded`]): the classifier-excluded rows through the partial covering index
// `file_exclusion` (no table lookups) and the rows with an exclude decision by id. Counting and sorting happen
// here, not in SQL: a UNION ALL with GROUP BY and ORDER BY over 200,000 rows took 120 ms and 200 ms on the
// budget's catalog, the single pass does both in a fraction of that (ADR-0103).

fn parse_category(v: ValueRef<'_>) -> Option<ExclusionCategory> {
    match v {
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(ExclusionCategory::from_key),
        _ => None,
    }
}

fn parse_kind(v: ValueRef<'_>) -> Option<AiKind> {
    match v {
        ValueRef::Text(t) => std::str::from_utf8(t).ok().and_then(AiKind::from_key),
        _ => None,
    }
}

/// The Excluded view's search (ADR-0104): the text, folded, matched against each source's displayed root and
/// the path under it, folded the same way ([`crate::display::search_fold`]).
struct Search {
    needle: String,
    /// Each source's displayed root, folded, with the separator the lists print after it.
    roots: HashMap<i64, String>,
    hay: String,
}

impl Search {
    fn new(c: &Connection, text: &str) -> Result<Search> {
        let mut needle = String::new();
        crate::display::search_fold(text, &mut needle);
        let home = crate::display::display_home();
        let mut roots = HashMap::new();
        let mut stmt = c.prepare_cached("SELECT id, root_path FROM source")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let root = ps_model::path_from_bytes(&r.get::<_, Vec<u8>>(1)?);
            let shown = crate::display::display_path_with(&root, home.as_deref());
            let mut folded = String::new();
            crate::display::search_fold(&shown, &mut folded);
            if !folded.ends_with(std::path::MAIN_SEPARATOR) {
                folded.push(std::path::MAIN_SEPARATOR);
            }
            roots.insert(r.get(0)?, folded);
        }
        Ok(Search {
            needle,
            roots,
            hay: String::new(),
        })
    }

    /// Whether the file of `source` at `rel` (the stored relative path) is matched.
    fn matches(&mut self, source: ValueRef<'_>, rel: ValueRef<'_>) -> bool {
        let (ValueRef::Integer(source), ValueRef::Blob(rel) | ValueRef::Text(rel)) = (source, rel)
        else {
            return false;
        };
        self.hay.clear();
        if let Some(root) = self.roots.get(&source) {
            self.hay.push_str(root);
        }
        let rel = String::from_utf8_lossy(rel);
        if std::path::MAIN_SEPARATOR == '/' {
            crate::display::search_fold(&rel, &mut self.hay);
        } else {
            crate::display::search_fold(
                &rel.replace('/', std::path::MAIN_SEPARATOR_STR),
                &mut self.hay,
            );
        }
        self.hay.contains(self.needle.as_str())
    }
}

/// One listed file and what its order needs.
struct Entry {
    id: ItemId,
    kflags: i64,
    rank: u8,
    kind: Option<AiKind>,
    /// The numeric sort value (size, capture time) or the 8-byte prefix of the sort bytes.
    primary: u64,
    /// The sort bytes (name or path) in `Scan::bytes`, for ties of the prefix.
    bytes: (u32, u32),
    undated: bool,
}

/// The pass's result: every category's count, and the selection's files when a sort was asked for.
struct Scan {
    counts: HashMap<(ExclusionCategory, Option<AiKind>), u64>,
    entries: Vec<Entry>,
    bytes: Vec<u8>,
}

/// The sort column the pass reads for a sort field.
fn sort_column(field: SortField) -> &'static str {
    match field {
        SortField::FileName | SortField::Category => "name",
        SortField::FileSize => "size",
        SortField::Folder => "rel_path",
        SortField::CaptureDate => "capture_ts",
    }
}

/// Stream the excluded set once: count every (category, kind), and collect the files `sel` shows (when `sort` is
/// given) with their sort data. The other filter fields and the search text narrow both.
fn scan_excluded(
    c: &Connection,
    f: &Filter,
    sel: &ExclusionSelection,
    sort: Option<&Sort>,
    within: Option<&ItemSet>,
) -> Result<Scan> {
    let base = Filter {
        classes: None,
        exclusion: None,
        ..f.clone()
    };
    let (cond, args) = crate::read::filter_sql_for(c, &base)?;
    // The search matches what the lists print (ADR-0104): the source's root as displayed (the home folder as
    // `~`) and the path under it, case-insensitively for any script. It is applied here, not in SQL (SQLite's
    // `lower` folds ASCII only).
    let mut search = match sel.search() {
        Some(t) => Some(Search::new(c, t)?),
        None => None,
    };
    let text = if search.is_some() {
        ", source_id, rel_path"
    } else {
        ""
    };
    let col = sort.map_or("NULL", |s| sort_column(s.field));
    let mut scan = Scan {
        counts: HashMap::new(),
        entries: Vec::new(),
        bytes: Vec::new(),
    };
    let add = |scan: &mut Scan,
               id: i64,
               cat: ExclusionCategory,
               kind: Option<AiKind>,
               kflags: i64,
               v: ValueRef<'_>| {
        *scan.counts.entry((cat, kind)).or_insert(0) += 1;
        let id = ItemId(id as u32);
        if sort.is_none() || !sel.shows(cat, kind) || within.is_some_and(|w| !w.contains(id)) {
            return;
        }
        let mut e = Entry {
            id,
            kflags,
            rank: cat.rank(),
            kind,
            primary: 0,
            bytes: (0, 0),
            undated: false,
        };
        match v {
            ValueRef::Integer(n) if sort.is_some_and(|s| s.field == SortField::CaptureDate) => {
                e.primary = SortKey::signed_primary(n);
            }
            ValueRef::Integer(n) => e.primary = n as u64,
            ValueRef::Blob(b) | ValueRef::Text(b) => {
                e.primary = SortKey::bytes_primary(b);
                e.bytes = (scan.bytes.len() as u32, b.len() as u32);
                scan.bytes.extend_from_slice(b);
            }
            _ => e.undated = true,
        }
        scan.entries.push(e);
    };

    // Every decision (few: what the user and accepted AI suggestions decided).
    let mut decided: HashMap<i64, Option<(ExclusionCategory, Option<AiKind>)>> = HashMap::new();
    {
        let mut stmt =
            c.prepare_cached("SELECT file_id, decision, origin, ai_kind FROM user_inclusion")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let excluded = matches!(r.get_ref(1)?, ValueRef::Text(b"exclude"));
            let cat = if !excluded {
                None
            } else if matches!(r.get_ref(2)?, ValueRef::Text(b"ai"))
                && let Some(kind) = parse_kind(r.get_ref(3)?)
            {
                Some((ExclusionCategory::Ai, Some(kind)))
            } else {
                // The user's own, or an AI decision without its kind (refused since migration 22): "you", so
                // it is in a chip.
                Some((ExclusionCategory::You, None))
            };
            decided.insert(r.get(0)?, cat);
        }
    }
    // Classifier-excluded files without a decision.
    {
        let sql = format!(
            "SELECT id, exclusion, kflags, {col}{text} FROM file INDEXED BY file_exclusion
             WHERE exclusion IS NOT NULL{cond}"
        );
        let mut stmt = c.prepare_cached(&sql)?;
        let mut rows = stmt.query(params_from_iter(args.iter()))?;
        while let Some(r) = rows.next()? {
            let ValueRef::Integer(id) = r.get_ref(0)? else {
                return Err(CatalogError::Corrupt("id".into()));
            };
            if decided.contains_key(&id) {
                continue;
            }
            let Some(cat) = parse_category(r.get_ref(1)?) else {
                continue;
            };
            if let Some(s) = &mut search
                && !s.matches(r.get_ref(4)?, r.get_ref(5)?)
            {
                continue;
            }
            let ValueRef::Integer(k) = r.get_ref(2)? else {
                return Err(CatalogError::Corrupt("kflags".into()));
            };
            add(&mut scan, id, cat, None, k, r.get_ref(3)?);
        }
    }
    // Files with an exclude decision.
    let excluded: Rc<Vec<Value>> = Rc::new(
        decided
            .iter()
            .filter(|(_, d)| d.is_some())
            .map(|(id, _)| Value::Integer(*id))
            .collect(),
    );
    if !excluded.is_empty() {
        let sql = format!("SELECT id, kflags, {col}{text} FROM file WHERE id IN rarray(?){cond}");
        let mut stmt = c.prepare_cached(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&excluded];
        for a in &args {
            params.push(a);
        }
        let mut rows = stmt.query(params.as_slice())?;
        while let Some(r) = rows.next()? {
            let ValueRef::Integer(id) = r.get_ref(0)? else {
                return Err(CatalogError::Corrupt("id".into()));
            };
            let Some(Some((cat, kind))) = decided.get(&id).copied() else {
                continue;
            };
            if let Some(s) = &mut search
                && !s.matches(r.get_ref(3)?, r.get_ref(4)?)
            {
                continue;
            }
            let ValueRef::Integer(k) = r.get_ref(1)? else {
                return Err(CatalogError::Corrupt("kflags".into()));
            };
            add(&mut scan, id, cat, kind, k, r.get_ref(2)?);
        }
    }
    Ok(scan)
}

impl Scan {
    fn counts(&self) -> ExclusionCounts {
        let mut rows: Vec<_> = self.counts.iter().map(|((c, k), n)| (*c, *k, *n)).collect();
        rows.sort();
        ExclusionCounts { rows }
    }

    /// The collected files in `sort` order as an ID list (keys non-decreasing, ADR-0018).
    fn into_list(self, sort: &Sort) -> IdList {
        self.into_tagged_list(sort).0
    }

    /// [`Scan::into_list`], with each listed file's category and AI kind by list position.
    fn into_tagged_list(
        mut self,
        sort: &Sort,
    ) -> (IdList, Vec<(ExclusionCategory, Option<AiKind>)>) {
        let bytes = std::mem::take(&mut self.bytes);
        let slice = |e: &Entry| &bytes[e.bytes.0 as usize..(e.bytes.0 + e.bytes.1) as usize];
        let by_bytes = matches!(
            sort.field,
            SortField::FileName | SortField::Folder | SortField::Category
        );
        let cmp = |a: &Entry, b: &Entry| {
            let o = match sort.field {
                SortField::Category => a.rank.cmp(&b.rank),
                // Undated last in either direction.
                SortField::CaptureDate => a.undated.cmp(&b.undated).then(a.primary.cmp(&b.primary)),
                _ => a.primary.cmp(&b.primary),
            };
            let o = if by_bytes {
                o.then_with(|| slice(a).cmp(slice(b)))
            } else {
                o
            };
            o.then(a.id.cmp(&b.id))
        };
        if sort.descending {
            self.entries.sort_unstable_by(|a, b| {
                if sort.field == SortField::CaptureDate && a.undated != b.undated {
                    return a.undated.cmp(&b.undated);
                }
                cmp(b, a)
            });
        } else {
            self.entries.sort_unstable_by(cmp);
        }
        let invert = |p: u64| if sort.descending { !p } else { p };
        let mut out = IdList::default();
        out.ids.reserve(self.entries.len());
        out.keys.reserve(self.entries.len());
        let mut tags = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            tags.push((ExclusionCategory::ALL[usize::from(e.rank)], e.kind));
            let primary = match sort.field {
                SortField::Category => invert(u64::from(e.rank)),
                SortField::CaptureDate if e.undated => SortKey::UNDATED,
                _ => invert(e.primary),
            };
            out.ids.push(e.id);
            out.keys
                .push(crate::read::sort_key(primary, e.id, e.kflags));
        }
        (out, tags)
    }
}

/// The ordered ID list of the excluded files a filter's [`ExclusionSelection`] shows (ADR-0018 keys). Category
/// sorts order by name within a category.
pub(crate) fn excluded_id_list(
    c: &Connection,
    f: &Filter,
    sel: &ExclusionSelection,
    sort: &Sort,
    within: Option<&ItemSet>,
) -> Result<IdList> {
    Ok(scan_excluded(c, f, sel, Some(sort), within)?.into_list(sort))
}

impl Readers {
    /// Excluded files per effective category and AI kind (ADR-0102), for the files the filter's other fields and
    /// its search text allow (its chips are ignored: every category is counted).
    pub fn exclusion_counts(&self, f: &Filter) -> Result<ExclusionCounts> {
        let sel = f.exclusion.clone().unwrap_or_default();
        self.with(|c| Ok(scan_excluded(c, f, &sel, None, None)?.counts()))
    }

    /// The Excluded view's opening read: the counts of [`Readers::exclusion_counts`] and the ID list of
    /// [`Readers::id_list`] for an exclusion filter, from one pass over the excluded set (`excluded-view-open`,
    /// ADR-0103). A filter without an exclusion selection lists every excluded file.
    pub fn excluded_view(&self, f: &Filter, sort: &Sort) -> Result<ExcludedListing> {
        let sel = f.exclusion.clone().unwrap_or_else(ExclusionSelection::all);
        self.with(|c| {
            let scan = scan_excluded(c, f, &sel, Some(sort), None)?;
            let counts = scan.counts();
            let (list, categories) = scan.into_tagged_list(sort);
            Ok(ExcludedListing {
                counts,
                list,
                categories,
            })
        })
    }

    /// The effective exclusion category (and AI kind) of each of `ids` that is excluded (ADR-0102).
    pub fn exclusion_of(
        &self,
        ids: &[FileId],
    ) -> Result<HashMap<FileId, (ExclusionCategory, Option<AiKind>)>> {
        let values: Rc<Vec<Value>> = Rc::new(ids.iter().map(|i| Value::Integer(i.0)).collect());
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT file.id,
                        CASE WHEN u.decision = 'exclude' THEN CASE u.origin WHEN 'ai' THEN 'ai' ELSE 'you' END
                             WHEN u.decision IS NULL THEN file.exclusion END,
                        CASE WHEN u.decision = 'exclude' THEN u.ai_kind END
                 FROM file LEFT JOIN user_inclusion u ON u.file_id = file.id
                 WHERE file.id IN rarray(?1)",
            )?;
            let mut rows = stmt.query([values])?;
            let mut out = HashMap::with_capacity(ids.len());
            while let Some(r) = rows.next()? {
                if let Some(cat) = parse_category(r.get_ref(1)?) {
                    let kind = parse_kind(r.get_ref(2)?);
                    // An AI decision without its kind is filed as the user's own (as the lists do).
                    let entry = match (cat, kind) {
                        (ExclusionCategory::Ai, None) => (ExclusionCategory::You, None),
                        (c, k) => (c, k),
                    };
                    out.insert(FileId(r.get(0)?), entry);
                }
            }
            Ok(out)
        })
    }
}
