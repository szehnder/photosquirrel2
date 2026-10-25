//! Read-only connection pool and queries (ADR-0005, ADR-0018).
//!
//! Queries run on read-only WAL connections, so they never block the writer and always see the last
//! committed batch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use ps_model::{
    ContentId, FileId, Filter, Format, IdList, ItemId, ItemSet, Metadata, PerceptualHash,
    PhotoClass, RelPath, RunStatus, ScanRunId, Sort, SortField, SortKey, SourceId, SourceStatus,
    Stage, StatFingerprint,
};

use crate::db;
use crate::error::{CatalogError, Result};
use crate::model::{FileErrorRecord, FileKey, FileRecord, RunRecord, SourceRecord, codes};

/// Largest detail page (ADR-0018).
pub const PAGE_SIZE: usize = 512;

/// A small pool of read-only connections.
pub struct Readers {
    path: PathBuf,
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl std::fmt::Debug for Readers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readers")
            .field("path", &self.path)
            .field("connections", &self.conns.len())
            .finish()
    }
}

pub(crate) const FILE_COLUMNS: &str = "id, source_id, rel_path, size, mtime_ns, file_index, stage, class, reasons, format,
    width, height, content_id, identity_kind, phash, dhash, hash_version, capture_ts, capture_undated,
    camera_model, has_gps, attempts, missing_since, first_seen, capture_offset_min, capture_provenance,
    capture_precision, date_version, decoder_missing";

/// Number of columns in [`FILE_COLUMNS`]: the index of the first column a query appends after them.
pub(crate) const FILE_COLUMN_COUNT: usize = 29;

impl Readers {
    pub(crate) fn open(path: &Path, size: usize) -> Result<Readers> {
        let conns = (0..size.max(1))
            .map(|_| db::open_reader(path).map(Mutex::new))
            .collect::<Result<Vec<_>>>()?;
        Ok(Readers {
            path: path.to_path_buf(),
            conns,
            next: AtomicUsize::new(0),
        })
    }

    /// Run `f` on a free connection (or wait for one).
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        for c in &self.conns {
            if let Some(g) = c.try_lock() {
                return f(&g);
            }
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        f(&self.conns[i].lock())
    }

    pub fn schema_version(&self) -> Result<u32> {
        self.with(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?))
    }

    /// The complete ordered ID list for a filter and sort, with 16-byte sort keys (ADR-0018).
    ///
    /// Each sort scans one sort-first covering index (no table lookups, no sort step). Sort-key primaries
    /// are non-decreasing along the list: for descending sorts the primary is bitwise inverted. Capture-date
    /// sorts put undated items last (primary [`SortKey::UNDATED`]) in both directions.
    pub fn id_list(&self, filter: &Filter, sort: &Sort) -> Result<IdList> {
        self.with(|c| id_list(c, filter, sort, None))
    }

    /// [`Readers::id_list`] restricted to the items in `within`: the same index scan, skipping rows outside
    /// the set while stepping (no second pass). For filters on data the catalog cannot express in SQL
    /// (effective AI insights, export dispositions; `ps_service::library`).
    pub fn id_list_within(&self, filter: &Filter, sort: &Sort, within: &ItemSet) -> Result<IdList> {
        self.with(|c| id_list(c, filter, sort, Some(within)))
    }

    /// Number of files matching `filter`.
    pub fn count(&self, filter: &Filter) -> Result<u64> {
        self.with(|c| {
            let (cond, args) = filter_sql_for(c, filter)?;
            let sql = format!("SELECT count(*) FROM file WHERE 1 {cond}");
            Ok(c.query_row(&sql, params_from_iter(args), |r| r.get::<_, i64>(0))? as u64)
        })
    }

    /// Details for up to [`PAGE_SIZE`] ids (a window of an ID list), in the given order. Ids that no longer
    /// exist are left out.
    pub fn details(&self, ids: &[ItemId]) -> Result<Vec<FileRecord>> {
        if ids.len() > PAGE_SIZE {
            return Err(CatalogError::Invalid(format!(
                "detail pages hold at most {PAGE_SIZE} items, got {}",
                ids.len()
            )));
        }
        let values: Rc<Vec<Value>> =
            Rc::new(ids.iter().map(|i| Value::Integer(i64::from(i.0))).collect());
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {FILE_COLUMNS} FROM file WHERE id IN rarray(?1)"
            ))?;
            let mut by_id: HashMap<i64, FileRecord> = HashMap::with_capacity(ids.len());
            let mut rows = stmt.query([values])?;
            while let Some(r) = rows.next()? {
                let rec = file_record(r)?;
                by_id.insert(rec.id.0, rec);
            }
            Ok(ids
                .iter()
                .filter_map(|i| by_id.remove(&i64::from(i.0)))
                .collect())
        })
    }

    /// One file by its key.
    pub fn file(&self, key: &FileKey) -> Result<Option<FileRecord>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {FILE_COLUMNS} FROM file WHERE source_id = ?1 AND rel_path = ?2"
            ))?;
            Ok(stmt
                .query_row(params![key.source.0, key.path.as_bytes()], |r| {
                    file_record(r).map_err(to_sql_err)
                })
                .optional()?)
        })
    }

    /// One file by id.
    pub fn file_by_id(&self, id: FileId) -> Result<Option<FileRecord>> {
        self.with(|c| {
            let mut stmt =
                c.prepare_cached(&format!("SELECT {FILE_COLUMNS} FROM file WHERE id = ?1"))?;
            Ok(stmt
                .query_row([id.0], |r| file_record(r).map_err(to_sql_err))
                .optional()?)
        })
    }

    /// A keyset page of a source's files in relative-path byte order, after `after` (exclusive). The
    /// scanner's rescan diff merge-joins the walk against these pages (ADR-0007).
    pub fn source_files(
        &self,
        source: SourceId,
        after: Option<&RelPath>,
        limit: usize,
    ) -> Result<Vec<FileRecord>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {FILE_COLUMNS}, classifier_hash FROM file
                 WHERE source_id = ?1 AND rel_path > ?2 ORDER BY rel_path LIMIT ?3"
            ))?;
            let after: &[u8] = after.map_or(&[], |p| p.as_bytes());
            let mut rows = stmt.query(params![source.0, after, limit as i64])?;
            let mut out = Vec::with_capacity(limit.min(4096));
            while let Some(r) = rows.next()? {
                let mut rec = file_record(r)?;
                rec.classifier_hash = r
                    .get::<_, Option<i64>>(FILE_COLUMN_COUNT)?
                    .map(|h| h as u64);
                out.push(rec);
            }
            Ok(out)
        })
    }

    /// Files still waiting for a decoder (ADR-0091), per source, catalog format key and decoder: one aggregate
    /// over the partial index `file_decoder_missing`; missing files are left out. `sources` empty: every source.
    pub fn undecoded_counts(
        &self,
        sources: &[SourceId],
    ) -> Result<Vec<(SourceId, String, ps_model::DecoderKey, u64)>> {
        self.with(|c| {
            let values: Rc<Vec<Value>> =
                Rc::new(sources.iter().map(|s| Value::Integer(s.0)).collect());
            let mut stmt = c.prepare_cached(
                "SELECT source_id, coalesce(format, ''), decoder_missing, count(*) FROM file
                 WHERE decoder_missing IS NOT NULL AND missing_since IS NULL
                   AND (?1 OR source_id IN rarray(?2))
                 GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
            )?;
            let mut rows = stmt.query(params![sources.is_empty(), values])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let key: String = r.get(2)?;
                let Some(k) = ps_model::DecoderKey::from_storage(&key) else {
                    continue;
                };
                out.push((
                    SourceId(r.get(0)?),
                    r.get(1)?,
                    k,
                    r.get::<_, i64>(3)? as u64,
                ));
            }
            Ok(out)
        })
    }

    /// The stored classifier facts of `ids` (ADR-0051); files without them (probed before schema version 13,
    /// or never probed) are left out.
    pub fn class_facts(&self, ids: &[FileId]) -> Result<HashMap<FileId, ps_model::ClassFacts>> {
        self.with(|c| {
            let values: Rc<Vec<Value>> = Rc::new(ids.iter().map(|i| Value::Integer(i.0)).collect());
            let mut stmt = c.prepare_cached(
                "SELECT id, class_facts FROM file WHERE id IN rarray(?1) AND class_facts IS NOT NULL",
            )?;
            let mut rows = stmt.query([values])?;
            let mut out = HashMap::with_capacity(ids.len());
            while let Some(r) = rows.next()? {
                let json: String = r.get(1)?;
                let facts = serde_json::from_str(&json).map_err(|e| corrupt(&e.to_string()))?;
                out.insert(FileId(r.get(0)?), facts);
            }
            Ok(out)
        })
    }

    /// What re-dating needs of `ids` (ADR-0071): path, modification time and the date facts of the stored
    /// metadata. Rows without metadata (never analysed) are left out. The returned [`Metadata`] holds only
    /// `fields.capture_time`, `capture_date` and the tags named in `tag_names` (the resolver's inputs,
    /// `ps_media::date::STORED_DATE_TAGS`): the JSON is scanned once and the rest of a large tag list is
    /// skipped without being copied (gate review, task 8.7).
    pub fn date_facts(
        &self,
        ids: &[FileId],
        tag_names: &[&str],
    ) -> Result<Vec<(FileId, RelPath, i64, Metadata)>> {
        self.with(|c| {
            let values: Rc<Vec<Value>> = Rc::new(ids.iter().map(|i| Value::Integer(i.0)).collect());
            let mut stmt = c.prepare_cached(
                "SELECT id, rel_path, mtime_ns, metadata FROM file
                 WHERE id IN rarray(?1) AND metadata IS NOT NULL ORDER BY id",
            )?;
            let mut rows = stmt.query([values])?;
            let mut out = Vec::with_capacity(ids.len());
            while let Some(r) = rows.next()? {
                let rel = RelPath::from_bytes(r.get(1)?).ok_or_else(|| corrupt("rel_path"))?;
                let json = r.get_ref(3)?.as_str().map_err(rusqlite::Error::from)?;
                let d: date_json::Stored<'_> =
                    serde_json::from_str(json).map_err(|e| corrupt(&e.to_string()))?;
                let mut meta = Metadata::default();
                meta.fields.capture_time = d.fields.capture_time;
                meta.capture_date = d.capture_date;
                meta.fields.all_tags = d
                    .fields
                    .all_tags
                    .into_iter()
                    .filter(|(_, n, _)| tag_names.contains(&n.0.as_ref()))
                    .map(|(g, n, v)| (g.0.into_owned(), n.0.into_owned(), v.0.into_owned()))
                    .collect();
                out.push((FileId(r.get(0)?), rel, r.get(2)?, meta));
            }
            Ok(out)
        })
    }

    /// Full metadata of an analyzed file.
    pub fn metadata(&self, id: FileId) -> Result<Option<Metadata>> {
        self.with(|c| {
            let json: Option<String> = c
                .prepare_cached("SELECT metadata FROM file WHERE id = ?1")?
                .query_row([id.0], |r| r.get(0))
                .optional()?
                .flatten();
            json.map(|j| serde_json::from_str(&j).map_err(|e| CatalogError::Corrupt(e.to_string())))
                .transpose()
        })
    }

    /// Files sharing a content identity (exact duplicates, ADR-0011).
    pub fn files_with_content(&self, id: &ContentId) -> Result<Vec<FileId>> {
        self.files_with_digest(&id.digest)
    }

    /// Files whose content digest is `digest` (any identity kind), in id order.
    pub fn files_with_digest(&self, digest: &[u8; 32]) -> Result<Vec<FileId>> {
        self.with(|c| {
            let mut stmt =
                c.prepare_cached("SELECT id FROM file WHERE content_id = ?1 ORDER BY id")?;
            let rows = stmt.query_map([&digest[..]], |r| Ok(FileId(r.get(0)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every file with a content identity, as `(file, digest)`: one index-only pass over `file_by_content`
    /// (any state; the ID list query applies presence).
    pub fn for_each_content(&self, mut f: impl FnMut(FileId, &[u8; 32])) -> Result<()> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, content_id FROM file INDEXED BY file_by_content WHERE content_id IS NOT NULL",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let ValueRef::Integer(id) = r.get_ref(0)? else {
                    continue;
                };
                if let ValueRef::Blob(d) = r.get_ref(1)?
                    && let Ok(d) = <&[u8; 32]>::try_from(d)
                {
                    f(FileId(id), d);
                }
            }
            Ok(())
        })
    }

    /// Files per stage in a source.
    pub fn stage_counts(&self, source: SourceId) -> Result<Vec<(Stage, u64)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT stage, count(*) FROM file WHERE source_id = ?1 GROUP BY stage ORDER BY stage",
            )?;
            let mut rows = stmt.query([source.0])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let s = codes::stage_from(r.get(0)?).ok_or_else(|| corrupt("stage"))?;
                out.push((s, r.get::<_, i64>(1)? as u64));
            }
            Ok(out)
        })
    }

    /// Thumbed images whose stored perceptual hashes are of another algorithm version (ADR-0012): the
    /// hash-upgrade job recomputes them from `thumbs.db`. Ordered by id.
    pub fn stale_hashes(&self, version: u8) -> Result<Vec<(FileId, ContentId)>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, content_id, identity_kind FROM file
                 WHERE stage = 3 AND content_id IS NOT NULL AND hash_version IS NOT NULL
                   AND hash_version != ?1
                 ORDER BY id",
            )?;
            let mut rows = stmt.query([i64::from(version)])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let d: Vec<u8> = r.get(1)?;
                let digest: [u8; 32] = d.try_into().map_err(|_| corrupt("content_id"))?;
                let kind = codes::identity_kind_from(r.get::<_, Option<i64>>(2)?.unwrap_or(0));
                out.push((FileId(r.get(0)?), ContentId { digest, kind }));
            }
            Ok(out)
        })
    }

    /// The error rows of a file, oldest first.
    pub fn file_errors(&self, id: FileId) -> Result<Vec<FileErrorRecord>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT stage, message, run_id, at FROM file_error WHERE file_id = ?1 ORDER BY id",
            )?;
            let mut rows = stmt.query([id.0])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(FileErrorRecord {
                    stage: codes::stage_from(r.get(0)?).ok_or_else(|| corrupt("stage"))?,
                    message: r.get(1)?,
                    run: r.get::<_, Option<i64>>(2)?.map(ScanRunId),
                    at_ms: r.get(3)?,
                });
            }
            Ok(out)
        })
    }

    pub fn sources(&self) -> Result<Vec<SourceRecord>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, root_path, volume_uuid, device_hint, total_bytes, fingerprint, status, added_at,
                        folded_into
                 FROM source ORDER BY id",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let status: String = r.get(6)?;
                out.push(SourceRecord {
                    id: SourceId(r.get(0)?),
                    root: r.get(1)?,
                    volume_uuid: r.get(2)?,
                    device_hint: r.get(3)?,
                    total_bytes: r.get::<_, Option<i64>>(4)?.map(|b| b as u64),
                    fingerprint: r.get(5)?,
                    status: SourceStatus::parse(&status).ok_or_else(|| corrupt("source status"))?,
                    added_at_ms: r.get(7)?,
                    folded_into: r.get::<_, Option<i64>>(8)?.map(SourceId),
                });
            }
            Ok(out)
        })
    }

    pub fn run(&self, id: ScanRunId) -> Result<Option<RunRecord>> {
        Ok(self.runs()?.into_iter().find(|r| r.id == id))
    }

    /// All scan runs, oldest first.
    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, status, started_at, ended_at, sources, stats, refresh_analysis FROM scan_run ORDER BY id",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let status: String = r.get(1)?;
                let sources: String = r.get(4)?;
                let stats: String = r.get(5)?;
                let sources: Vec<i64> =
                    serde_json::from_str(&sources).map_err(|e| corrupt(&e.to_string()))?;
                out.push(RunRecord {
                    id: ScanRunId(r.get(0)?),
                    status: RunStatus::parse(&status).ok_or_else(|| corrupt("run status"))?,
                    started_at_ms: r.get(2)?,
                    ended_at_ms: r.get(3)?,
                    sources: sources.into_iter().map(SourceId).collect(),
                    stats: serde_json::from_str(&stats).map_err(|e| corrupt(&e.to_string()))?,
                    refresh_analysis: r.get::<_, i64>(6)? != 0,
                });
            }
            Ok(out)
        })
    }
}

fn corrupt(what: &str) -> CatalogError {
    CatalogError::Corrupt(what.to_owned())
}

fn to_sql_err(e: CatalogError) -> rusqlite::Error {
    match e {
        CatalogError::Sqlite(e) => e,
        other => rusqlite::Error::ModuleError(other.to_string()),
    }
}

/// [`filter_sql`] for every field but the classes, then the class filter with user decisions and the
/// gallery predicates ([`crate::gallery::extend_filter_sql`]).
fn filter_sql_for(c: &Connection, f: &Filter) -> Result<(String, Vec<Value>)> {
    let base = Filter {
        classes: None,
        ..f.clone()
    };
    let (mut sql, mut args) = filter_sql(&base);
    crate::gallery::extend_filter_sql(c, f, &mut sql, &mut args)?;
    Ok((sql, args))
}

/// `AND ...` conditions for a filter plus their bound values. Bound parameters only, never interpolated
/// values.
fn filter_sql(f: &Filter) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut args = Vec::new();
    if let Some(classes) = &f.classes {
        if classes.is_empty() {
            sql.push_str(" AND 0");
        } else {
            sql.push_str(" AND (kflags & 7) IN (");
            for (i, c) in classes.iter().enumerate() {
                sql.push_str(if i == 0 { "?" } else { ",?" });
                args.push(Value::Integer(i64::from(c.code())));
            }
            sql.push(')');
        }
    }
    if let Some(k) = f.kind {
        sql.push_str(" AND ((kflags >> 3) & 1) = ?");
        args.push(Value::Integer(codes::kind(k)));
    }
    if let Some(sources) = &f.sources {
        if sources.is_empty() {
            sql.push_str(" AND 0");
        } else {
            sql.push_str(" AND source_id IN (");
            for (i, s) in sources.iter().enumerate() {
                sql.push_str(if i == 0 { "?" } else { ",?" });
                args.push(Value::Integer(s.0));
            }
            sql.push(')');
        }
    }
    if !f.include_missing {
        sql.push_str(" AND kflags < 16");
    }
    (sql, args)
}

fn id_list(
    c: &Connection,
    filter: &Filter,
    sort: &Sort,
    within: Option<&ItemSet>,
) -> Result<IdList> {
    let (cond, args) = filter_sql_for(c, filter)?;
    let dir = if sort.descending { "DESC" } else { "ASC" };
    let tail = "kflags";
    let mut out = IdList::default();
    let n = c.query_row("SELECT max(id) FROM file", [], |r| {
        r.get::<_, Option<i64>>(0)
    })?;
    if n.is_some_and(|n| n > i64::from(u32::MAX)) {
        return Err(CatalogError::Corrupt(
            "file ids exceed the 32-bit item id range".into(),
        ));
    }
    // max(id) is a rowid lookup and an upper bound of the row count: one allocation for the common case.
    let estimate = match within {
        Some(w) => w.len(),
        None => n.unwrap_or(0).clamp(0, 1 << 24) as usize,
    };
    out.ids.reserve(estimate);
    out.keys.reserve(estimate);
    let invert = |p: u64| if sort.descending { !p } else { p };
    match sort.field {
        SortField::CaptureDate => {
            let dated = format!(
                "SELECT id, capture_ts, {tail} FROM file INDEXED BY file_by_capture
                 WHERE capture_ts IS NOT NULL {cond} ORDER BY capture_ts {dir}, id {dir}"
            );
            collect(c, &dated, &args, within, &mut out, |v| {
                let ValueRef::Integer(ts) = v else {
                    return Err(corrupt("capture_ts"));
                };
                Ok(invert(SortKey::signed_primary(ts)))
            })?;
            let undated = format!(
                "SELECT id, NULL, {tail} FROM file INDEXED BY file_by_capture
                 WHERE capture_ts IS NULL {cond} ORDER BY id {dir}"
            );
            collect(c, &undated, &args, within, &mut out, |_| {
                Ok(SortKey::UNDATED)
            })?;
        }
        SortField::FileSize => {
            let sql = format!(
                "SELECT id, size, {tail} FROM file INDEXED BY file_by_size
                 WHERE 1 {cond} ORDER BY size {dir}, id {dir}"
            );
            collect(c, &sql, &args, within, &mut out, |v| match v {
                ValueRef::Integer(s) => Ok(invert(s as u64)),
                _ => Err(corrupt("size")),
            })?;
        }
        SortField::FileName => {
            let sql = format!(
                "SELECT id, name, {tail} FROM file INDEXED BY file_by_name
                 WHERE 1 {cond} ORDER BY name {dir}, id {dir}"
            );
            collect(c, &sql, &args, within, &mut out, |v| match v {
                ValueRef::Blob(b) | ValueRef::Text(b) => Ok(invert(SortKey::bytes_primary(b))),
                _ => Err(corrupt("name")),
            })?;
        }
    }
    Ok(out)
}

/// Step a `SELECT id, <primary>, kflags` query into the list.
fn collect(
    c: &Connection,
    sql: &str,
    args: &[Value],
    within: Option<&ItemSet>,
    out: &mut IdList,
    primary: impl Fn(ValueRef<'_>) -> Result<u64>,
) -> Result<()> {
    let mut stmt = c.prepare_cached(sql)?;
    let mut rows = stmt.query(params_from_iter(args.iter()))?;
    while let Some(r) = rows.next()? {
        let ValueRef::Integer(id) = r.get_ref(0)? else {
            return Err(corrupt("id"));
        };
        let id = ItemId(id as u32);
        if within.is_some_and(|w| !w.contains(id)) {
            continue;
        }
        let p = primary(r.get_ref(1)?)?;
        let ValueRef::Integer(k) = r.get_ref(2)? else {
            return Err(corrupt("kflags"));
        };
        let class = match (k & 7) as u8 {
            7 => SortKey::CLASS_NONE,
            c => c,
        };
        let mut flags = 0;
        if k & 8 != 0 {
            flags |= SortKey::FLAG_VIDEO;
        }
        if k & 16 != 0 {
            flags |= SortKey::FLAG_MISSING;
        }
        out.ids.push(id);
        out.keys.push(SortKey::new(p, id, class, flags));
    }
    Ok(())
}

pub(crate) fn file_record(r: &Row<'_>) -> Result<FileRecord> {
    let rel: Vec<u8> = r.get(2)?;
    let path = RelPath::from_bytes(rel).ok_or_else(|| corrupt("rel_path"))?;
    let stage = codes::stage_from(r.get(6)?).ok_or_else(|| corrupt("stage"))?;
    let class = match r.get::<_, Option<i64>>(7)? {
        Some(c) => Some(PhotoClass::from_code(c as u8).ok_or_else(|| corrupt("class"))?),
        None => None,
    };
    let reasons = match r.get::<_, Option<String>>(8)? {
        Some(j) => serde_json::from_str(&j).map_err(|e| corrupt(&e.to_string()))?,
        None => Vec::new(),
    };
    let format = match r.get::<_, Option<String>>(9)? {
        Some(f) => Some(Format::from_storage(&f).ok_or_else(|| corrupt("format"))?),
        None => None,
    };
    let identity = match r.get::<_, Option<Vec<u8>>>(12)? {
        Some(d) => {
            let digest: [u8; 32] = d.try_into().map_err(|_| corrupt("content_id"))?;
            let kind = codes::identity_kind_from(r.get::<_, Option<i64>>(13)?.unwrap_or(0));
            Some(ContentId { digest, kind })
        }
        None => None,
    };
    let hashes = match (
        r.get::<_, Option<i64>>(14)?,
        r.get::<_, Option<i64>>(15)?,
        r.get::<_, Option<i64>>(16)?,
    ) {
        (Some(p), Some(d), Some(v)) => Some(PerceptualHash {
            phash: p as u64,
            dhash: d as u64,
            version: v as u8,
        }),
        _ => None,
    };
    Ok(FileRecord {
        id: FileId(r.get(0)?),
        key: FileKey::new(SourceId(r.get(1)?), path),
        stat: StatFingerprint {
            size: r.get::<_, i64>(3)? as u64,
            mtime_ns: r.get(4)?,
            file_index: r.get::<_, Option<i64>>(5)?.map(|i| i as u64),
        },
        stage,
        class,
        reasons,
        format,
        width: r.get(10)?,
        height: r.get(11)?,
        identity,
        hashes,
        capture_ms: r.get(17)?,
        capture_undated: r.get(18)?,
        camera_model: r.get(19)?,
        has_gps: r.get(20)?,
        attempts: r.get(21)?,
        missing_since_ms: r.get(22)?,
        first_seen_ms: r.get(23)?,
        capture_offset_min: r.get(24)?,
        capture_provenance: match r.get_ref(25)? {
            ValueRef::Text(t) => Some(
                std::str::from_utf8(t)
                    .ok()
                    .and_then(ps_model::DateProvenance::parse)
                    .ok_or_else(|| corrupt("capture_provenance"))?,
            ),
            _ => None,
        },
        capture_precision: ps_model::DatePrecision::from_code(r.get::<_, i64>(26)? as u8)
            .ok_or_else(|| corrupt("capture_precision"))?,
        date_version: r.get::<_, i64>(27)? as u8,
        classifier_hash: None,
        decoder_missing: match r.get_ref(28)? {
            ValueRef::Text(t) => Some(
                std::str::from_utf8(t)
                    .ok()
                    .and_then(ps_model::DecoderKey::from_storage)
                    .ok_or_else(|| corrupt("decoder_missing"))?,
            ),
            _ => None,
        },
    })
}

/// The date facts of a stored metadata JSON, borrowed from the text (see [`Readers::date_facts`]).
mod date_json {
    use std::borrow::Cow;

    use ps_model::{CaptureDate, CaptureTime};
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct Stored<'a> {
        #[serde(default, borrow)]
        pub fields: Fields<'a>,
        #[serde(default)]
        pub capture_date: Option<CaptureDate>,
    }

    #[derive(Default, Deserialize)]
    pub(super) struct Fields<'a> {
        #[serde(default)]
        pub capture_time: Option<CaptureTime>,
        #[serde(default, borrow)]
        pub all_tags: Vec<(Txt<'a>, Txt<'a>, Txt<'a>)>,
    }

    /// A string borrowed from the JSON text unless it has escapes.
    #[derive(Deserialize)]
    pub(super) struct Txt<'a>(#[serde(borrow)] pub Cow<'a, str>);
}
