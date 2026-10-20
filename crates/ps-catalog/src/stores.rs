//! Catalog storage for the AI queue (migration 5, ADR-0020/0021) and the export ledger (migration 6,
//! ADR-0022/0023). Typed reads on [`Readers`], atomic writes on [`Writer`] (each write is committed when it
//! returns, so the AI queue survives a crash and a ledger row is durable before the copy it describes).
//!
//! The engines' storage traits (`ps_llm::queue::QueueStore`, `ps_export::Ledger`) are implemented over these in
//! `ps-service`; this crate does not depend on either engine.

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use ps_model::{ContentId, ExportProfile, FileId, LedgerEntry, LedgerState, RelPath};

use crate::error::{CatalogError, Result};
use crate::model::{codes, now_ms};
use crate::read::Readers;
use crate::writer::Writer;

fn corrupt(what: &str) -> CatalogError {
    CatalogError::Corrupt(what.to_owned())
}

fn digest(v: Vec<u8>) -> Result<[u8; 32]> {
    v.try_into().map_err(|_| corrupt("content id"))
}

// ---- AI queue ---------------------------------------------------------------------------------------------

/// `ai_queue.state` codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiItemState {
    Pending,
    InFlight,
    Done,
    Failed,
    /// Waiting for an offline source.
    Deferred,
}

impl AiItemState {
    pub fn code(self) -> i64 {
        match self {
            AiItemState::Pending => 0,
            AiItemState::InFlight => 1,
            AiItemState::Done => 2,
            AiItemState::Failed => 3,
            AiItemState::Deferred => 4,
        }
    }

    pub fn from_code(c: i64) -> Option<Self> {
        Some(match c {
            0 => AiItemState::Pending,
            1 => AiItemState::InFlight,
            2 => AiItemState::Done,
            3 => AiItemState::Failed,
            4 => AiItemState::Deferred,
            _ => return None,
        })
    }
}

/// One queued content identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiQueueRow {
    pub content: ContentId,
    pub priority: i64,
    pub state: AiItemState,
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// Items per state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiCounts {
    pub pending: u64,
    pub in_flight: u64,
    pub done: u64,
    pub failed: u64,
    pub deferred: u64,
}

/// One stored insight (`ai_result`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiResultRow {
    pub content: ContentId,
    pub provider: String,
    pub model: String,
    pub schema_version: u32,
    pub prompt_version: u32,
    /// JSON `ps_model::ImageInsight`.
    pub insight: String,
    pub latency_ms: u64,
    pub requests: u32,
    pub at_ms: i64,
}

/// `ai_run` (a single row): the queue's run state across restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRunState {
    Paused,
    Running,
    Finished,
}

impl AiRunState {
    fn code(self) -> i64 {
        match self {
            AiRunState::Paused => 0,
            AiRunState::Running => 1,
            AiRunState::Finished => 2,
        }
    }

    fn from_code(c: i64) -> Option<Self> {
        Some(match c {
            0 => AiRunState::Paused,
            1 => AiRunState::Running,
            2 => AiRunState::Finished,
            _ => return None,
        })
    }
}

/// The identity kind of a digest from the file table (`ai_queue` keys by digest only).
fn kind_of(c: &rusqlite::Connection, d: &[u8]) -> rusqlite::Result<i64> {
    Ok(
        c.prepare_cached("SELECT identity_kind FROM file WHERE content_id = ?1 LIMIT 1")?
            .query_row([d], |r| r.get::<_, Option<i64>>(0))
            .optional()?
            .flatten()
            .unwrap_or(0),
    )
}

fn queue_row(c: &rusqlite::Connection, r: &Row<'_>) -> rusqlite::Result<AiQueueRow> {
    let d: Vec<u8> = r.get(0)?;
    let kind = kind_of(c, &d)?;
    let digest: [u8; 32] = d
        .try_into()
        .map_err(|_| rusqlite::Error::ModuleError("content id".into()))?;
    Ok(AiQueueRow {
        content: ContentId {
            digest,
            kind: codes::identity_kind_from(kind),
        },
        priority: r.get(1)?,
        state: AiItemState::from_code(r.get(2)?)
            .ok_or_else(|| rusqlite::Error::ModuleError("ai_queue state".into()))?,
        attempts: r.get::<_, i64>(3)? as u32,
        last_error: r.get(4)?,
    })
}

fn set_state(
    c: &rusqlite::Connection,
    content: &ContentId,
    state: AiItemState,
    add_attempts: u32,
    error: Option<&str>,
    keep_error: bool,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE ai_queue SET state = ?2, attempts = attempts + ?3,
                last_error = CASE WHEN ?5 THEN last_error ELSE ?4 END, updated_at = ?6
         WHERE content_id = ?1",
    )?
    .execute(params![
        &content.digest[..],
        state.code(),
        i64::from(add_attempts),
        error,
        keep_error,
        now_ms()
    ])?;
    Ok(())
}

impl Writer {
    /// Queue contents (`(content, priority)`; lower runs first). A content already queued keeps its state and
    /// gets the new priority while pending. Returns how many were new.
    pub fn ai_enqueue(&self, items: Vec<(ContentId, i64)>) -> Result<u64> {
        self.exec(move |c| {
            let now = now_ms();
            let mut added = 0;
            for (content, priority) in &items {
                let n = c
                    .prepare_cached(
                        "INSERT INTO ai_queue (content_id, priority, enqueued_at, updated_at) VALUES (?1, ?2, ?3, ?3)
                         ON CONFLICT (content_id) DO NOTHING",
                    )?
                    .execute(params![&content.digest[..], priority, now])?;
                if n == 0 {
                    c.prepare_cached(
                        "UPDATE ai_queue SET priority = ?2 WHERE content_id = ?1 AND state = 0",
                    )?
                    .execute(params![&content.digest[..], priority])?;
                }
                added += n as u64;
            }
            Ok(added)
        })
    }

    /// Start-up recovery: in-flight and deferred items become pending again; a `running` queue becomes
    /// `paused` (spec "Survives restarts": the queue is paused after a restart).
    pub fn ai_recover(&self) -> Result<()> {
        self.exec(|c| {
            c.execute(
                "UPDATE ai_queue SET state = 0, updated_at = ?1 WHERE state IN (1, 4)",
                [now_ms()],
            )?;
            c.execute(
                "UPDATE ai_run SET state = 0, updated_at = ?1 WHERE state = 1",
                [now_ms()],
            )?;
            Ok(())
        })
    }

    /// Claim the first pending item by priority (then content id), marking it in flight.
    pub fn ai_claim_next(&self) -> Result<Option<AiQueueRow>> {
        self.exec(|c| {
            let row = c
                .prepare_cached(
                    "SELECT content_id, priority, state, attempts, last_error FROM ai_queue
                     WHERE state = 0 ORDER BY priority, content_id LIMIT 1",
                )?
                .query_row([], |r| queue_row(c, r))
                .optional()?;
            if let Some(r) = &row {
                set_state(c, &r.content, AiItemState::InFlight, 0, None, true)?;
            }
            Ok(row.map(|mut r| {
                r.state = AiItemState::InFlight;
                r
            }))
        })
    }

    /// Set an item's state (adding `requests` attempts; `error` replaces the last error unless `None` with
    /// `keep_error`).
    pub fn ai_set_state(
        &self,
        content: ContentId,
        state: AiItemState,
        requests: u32,
        error: Option<String>,
    ) -> Result<()> {
        self.exec(move |c| {
            set_state(
                c,
                &content,
                state,
                requests,
                error.as_deref(),
                error.is_none() && state == AiItemState::Pending,
            )
        })
    }

    /// Store a result and mark the item done, atomically.
    pub fn ai_complete(&self, result: AiResultRow) -> Result<()> {
        self.exec(move |c| {
            c.prepare_cached(
                "INSERT INTO ai_result (content_id, provider, model, schema_version, prompt_version, insight,
                                        latency_ms, requests, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (content_id, provider, model, schema_version) DO UPDATE SET
                    prompt_version = excluded.prompt_version, insight = excluded.insight,
                    latency_ms = excluded.latency_ms, requests = excluded.requests, at = excluded.at",
            )?
            .execute(params![
                &result.content.digest[..],
                result.provider,
                result.model,
                i64::from(result.schema_version),
                i64::from(result.prompt_version),
                result.insight,
                result.latency_ms as i64,
                i64::from(result.requests),
                result.at_ms
            ])?;
            set_state(
                c,
                &result.content,
                AiItemState::Done,
                result.requests,
                None,
                false,
            )
        })
    }

    /// Record the queue's run state and provider.
    pub fn ai_set_run(
        &self,
        state: AiRunState,
        provider: Option<String>,
        model: Option<String>,
    ) -> Result<()> {
        self.exec(move |c| {
            c.prepare_cached(
                "INSERT INTO ai_run (id, state, provider, model, updated_at) VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET state = excluded.state,
                    provider = coalesce(excluded.provider, provider), model = coalesce(excluded.model, model),
                    updated_at = excluded.updated_at",
            )?
            .execute(params![state.code(), provider, model, now_ms()])?;
            Ok(())
        })
    }
}

fn result_row(r: &Row<'_>, kind: i64) -> rusqlite::Result<AiResultRow> {
    let d: Vec<u8> = r.get(0)?;
    let digest: [u8; 32] = d
        .try_into()
        .map_err(|_| rusqlite::Error::ModuleError("content id".into()))?;
    Ok(AiResultRow {
        content: ContentId {
            digest,
            kind: codes::identity_kind_from(kind),
        },
        provider: r.get(1)?,
        model: r.get(2)?,
        schema_version: r.get::<_, i64>(3)? as u32,
        prompt_version: r.get::<_, i64>(4)? as u32,
        insight: r.get(5)?,
        latency_ms: r.get::<_, i64>(6)? as u64,
        requests: r.get::<_, i64>(7)? as u32,
        at_ms: r.get(8)?,
    })
}

const RESULT_COLUMNS: &str = "content_id, provider, model, schema_version, prompt_version, insight, latency_ms, requests, at";

impl Readers {
    pub fn ai_counts(&self) -> Result<AiCounts> {
        self.with(|c| {
            let mut out = AiCounts::default();
            let mut stmt =
                c.prepare_cached("SELECT state, count(*) FROM ai_queue GROUP BY state")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let n = r.get::<_, i64>(1)? as u64;
                match AiItemState::from_code(r.get(0)?).ok_or_else(|| corrupt("ai state"))? {
                    AiItemState::Pending => out.pending = n,
                    AiItemState::InFlight => out.in_flight = n,
                    AiItemState::Done => out.done = n,
                    AiItemState::Failed => out.failed = n,
                    AiItemState::Deferred => out.deferred = n,
                }
            }
            Ok(out)
        })
    }

    /// Queue items in run order (priority, then content id).
    pub fn ai_queue(&self, limit: u64) -> Result<Vec<AiQueueRow>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT content_id, priority, state, attempts, last_error FROM ai_queue
                 ORDER BY priority, content_id LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit as i64], |r| queue_row(c, r))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// One queue item.
    pub fn ai_item(&self, content: &ContentId) -> Result<Option<AiQueueRow>> {
        self.with(|c| {
            Ok(c.prepare_cached(
                "SELECT content_id, priority, state, attempts, last_error FROM ai_queue WHERE content_id = ?1",
            )?
            .query_row([&content.digest[..]], |r| queue_row(c, r))
            .optional()?)
        })
    }

    /// A result exists for this content, provider, model (empty: any model) and schema version.
    pub fn ai_has_result(
        &self,
        content: &ContentId,
        provider: &str,
        model: &str,
        schema_version: u32,
    ) -> Result<bool> {
        self.with(|c| {
            Ok(c.prepare_cached(
                "SELECT 1 FROM ai_result WHERE content_id = ?1 AND provider = ?2
                    AND (?3 = '' OR model = ?3) AND schema_version = ?4 LIMIT 1",
            )?
            .query_row(
                params![
                    &content.digest[..],
                    provider,
                    model,
                    i64::from(schema_version)
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })
    }

    /// Results, newest first.
    pub fn ai_results(&self, limit: u64) -> Result<Vec<AiResultRow>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {RESULT_COLUMNS} FROM ai_result ORDER BY at DESC, content_id LIMIT ?1"
            ))?;
            let mut rows = stmt.query([limit as i64])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let d: Vec<u8> = r.get(0)?;
                let kind = kind_of(c, &d)?;
                out.push(result_row(r, kind)?);
            }
            Ok(out)
        })
    }

    /// The content of every result row stored at or after `at_ms`, one entry per row (`ai_result_by_time`).
    /// Incremental updates of the gallery's insight index (`ps_service::library`).
    pub fn ai_result_contents_since(&self, at_ms: i64) -> Result<Vec<[u8; 32]>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT content_id FROM ai_result INDEXED BY ai_result_by_time WHERE at >= ?1",
            )?;
            let mut rows = stmt.query([at_ms])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                if let rusqlite::types::ValueRef::Blob(d) = r.get_ref(0)?
                    && let Ok(d) = <[u8; 32]>::try_from(d)
                {
                    out.push(d);
                }
            }
            Ok(out)
        })
    }

    /// The newest insight JSON of each of `digests` that has a result, as `(digest, insight JSON)`.
    pub fn ai_latest_insights_of(
        &self,
        digests: &[[u8; 32]],
        mut f: impl FnMut([u8; 32], &str),
    ) -> Result<()> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT insight FROM ai_result WHERE content_id = ?1 ORDER BY at DESC LIMIT 1",
            )?;
            for d in digests {
                let mut rows = stmt.query([&d[..]])?;
                if let Some(r) = rows.next()?
                    && let rusqlite::types::ValueRef::Text(t) = r.get_ref(0)?
                {
                    f(*d, std::str::from_utf8(t).unwrap_or_default());
                }
            }
            Ok(())
        })
    }

    /// The newest result of every content, as `(digest, insight JSON)`, in content order: one pass over the
    /// table's primary key (gallery insight filters, `ps_service::library`).
    pub fn ai_latest_insights(&self, mut f: impl FnMut([u8; 32], &str)) -> Result<()> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT content_id, insight, at FROM ai_result ORDER BY content_id, at DESC",
            )?;
            let mut rows = stmt.query([])?;
            let mut last: Option<[u8; 32]> = None;
            while let Some(r) = rows.next()? {
                let rusqlite::types::ValueRef::Blob(d) = r.get_ref(0)? else {
                    continue;
                };
                let Ok(digest) = <[u8; 32]>::try_from(d) else {
                    continue;
                };
                if last == Some(digest) {
                    continue;
                }
                last = Some(digest);
                if let rusqlite::types::ValueRef::Text(t) = r.get_ref(1)? {
                    f(digest, std::str::from_utf8(t).unwrap_or_default());
                }
            }
            Ok(())
        })
    }

    /// The newest result of a content.
    pub fn ai_result_for(&self, content: &ContentId) -> Result<Option<AiResultRow>> {
        self.with(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {RESULT_COLUMNS} FROM ai_result WHERE content_id = ?1 ORDER BY at DESC LIMIT 1"
            ))?
            .query_row([&content.digest[..]], |r| {
                result_row(r, codes::identity_kind(content.kind))
            })
            .optional()?)
        })
    }

    /// The stored queue run state, provider and model.
    pub fn ai_run(&self) -> Result<Option<(AiRunState, Option<String>, Option<String>)>> {
        self.with(|c| {
            let row = c
                .prepare_cached("SELECT state, provider, model FROM ai_run WHERE id = 1")?
                .query_row([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .optional()?;
            row.map(|(s, p, m)| {
                Ok((
                    AiRunState::from_code(s).ok_or_else(|| corrupt("ai_run state"))?,
                    p,
                    m,
                ))
            })
            .transpose()
        })
    }
}

// ---- Export ledger ----------------------------------------------------------------------------------------

/// A registered export library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRecord {
    pub id: i64,
    /// Storage bytes of the absolute root ([`ps_model::path_to_bytes`]).
    pub root: Vec<u8>,
    pub volume_uuid: Option<String>,
    pub created_at_ms: i64,
}

fn profile_from(s: &str) -> Option<ExportProfile> {
    ExportProfile::parse(s)
}

fn ledger_row(r: &Row<'_>) -> Result<LedgerEntry> {
    let content = ContentId {
        digest: digest(r.get(0)?)?,
        kind: codes::identity_kind_from(r.get(1)?),
    };
    let full_hash = match r.get::<_, Option<Vec<u8>>>(2)? {
        Some(h) => Some(digest(h)?),
        None => None,
    };
    let rel: Vec<u8> = r.get(4)?;
    let profile: String = r.get(7)?;
    Ok(LedgerEntry {
        content,
        full_hash,
        file: FileId(r.get(3)?),
        library_rel_path: RelPath::from_bytes(rel).ok_or_else(|| corrupt("library path"))?,
        size: r.get::<_, i64>(5)? as u64,
        state: LedgerState::from_code(r.get(6)?).ok_or_else(|| corrupt("ledger state"))?,
        profile: profile_from(&profile).ok_or_else(|| corrupt("export profile"))?,
        at_ms: r.get(8)?,
    })
}

impl Writer {
    /// Register a library root (or return the existing one).
    pub fn export_library(&self, root: Vec<u8>, volume_uuid: Option<String>) -> Result<i64> {
        self.exec(move |c| {
            c.prepare_cached(
                "INSERT INTO export_library (root_path, volume_uuid, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (root_path) DO UPDATE SET volume_uuid = coalesce(excluded.volume_uuid, volume_uuid)",
            )?
            .execute(params![root, volume_uuid, now_ms()])?;
            c.prepare_cached("SELECT id FROM export_library WHERE root_path = ?1")?
                .query_row([&root], |r| r.get(0))
        })
    }

    /// Insert or replace the ledger row at `entry.library_rel_path`; committed when this returns.
    pub fn ledger_record(&self, library: i64, entry: LedgerEntry) -> Result<()> {
        self.exec(move |c| {
            c.prepare_cached(
                "INSERT INTO export_ledger (library_id, content_id, identity_kind, full_hash, file_id,
                                            library_rel_path, size, state, profile, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (library_id, library_rel_path) DO UPDATE SET
                    content_id = excluded.content_id, identity_kind = excluded.identity_kind,
                    full_hash = excluded.full_hash, file_id = excluded.file_id, size = excluded.size,
                    state = excluded.state, profile = excluded.profile, at = excluded.at",
            )?
            .execute(params![
                library,
                &entry.content.digest[..],
                codes::identity_kind(entry.content.kind),
                entry.full_hash.as_ref().map(|h| &h[..]),
                entry.file.0,
                entry.library_rel_path.as_bytes(),
                entry.size as i64,
                entry.state.code(),
                entry.profile.as_str(),
                entry.at_ms
            ])?;
            Ok(())
        })
    }
}

impl Readers {
    /// Every registered library.
    pub fn export_libraries(&self) -> Result<Vec<LibraryRecord>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, root_path, volume_uuid, created_at FROM export_library ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(LibraryRecord {
                    id: r.get(0)?,
                    root: r.get(1)?,
                    volume_uuid: r.get(2)?,
                    created_at_ms: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The ledger rows of one library, by library path.
    pub fn ledger(&self, library: i64) -> Result<Vec<LedgerEntry>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT content_id, identity_kind, full_hash, file_id, library_rel_path, size, state, profile, at
                 FROM export_ledger WHERE library_id = ?1 ORDER BY library_rel_path",
            )?;
            let mut rows = stmt.query([library])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(ledger_row(r)?);
            }
            Ok(out)
        })
    }

    /// The user's exclusions (`user_inclusion` decisions `exclude`), by file id.
    pub fn user_excluded_files(&self) -> Result<Vec<FileId>> {
        if !self.has_user_decisions()? {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT file_id FROM user_inclusion WHERE decision = 'exclude' ORDER BY file_id",
            )?;
            let rows = stmt.query_map([], |r| Ok(FileId(r.get(0)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

impl Readers {
    /// Every catalogued file (present and missing), by id, without collecting.
    pub fn for_each_file(&self, mut f: impl FnMut(crate::FileRecord)) -> Result<()> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {} FROM file ORDER BY id",
                crate::read::FILE_COLUMNS
            ))?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                f(crate::read::file_record(r)?);
            }
            Ok(())
        })
    }
}
