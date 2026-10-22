//! Catalog storage for cleanup runs (migration 14, ADR-0061): the catalog half of the write-ahead journal.
//!
//! Every write is committed when it returns, so a move's intent is durable before the rename it describes
//! (`ps_export::quarantine`). This crate stores rows only; it never touches a source tree.

use rusqlite::{Connection, Row, params};

use ps_model::{
    CleanupMove, CleanupRunRecord, CleanupRunState, ContentId, Disposition, FileId, MoveState,
    RelPath, SourceId,
};

use crate::error::{CatalogError, Result};
use crate::model::{codes, now_ms};
use crate::read::Readers;
use crate::writer::Writer;

fn corrupt(what: &str) -> CatalogError {
    CatalogError::Corrupt(what.to_owned())
}

fn rel(v: Vec<u8>, what: &str) -> Result<RelPath> {
    RelPath::from_bytes(v).ok_or_else(|| corrupt(what))
}

fn disposition_code(d: Disposition) -> i64 {
    match d {
        Disposition::Covered => 1,
        _ => 0,
    }
}

const MOVE_COLUMNS: &str = "file_id, rel_path, quarantine_rel_path, size, mtime_ns, file_index, content_id, \
                            identity_kind, disposition, covering_library_path, state, note, at";

fn move_row(r: &Row<'_>) -> Result<CleanupMove> {
    let digest: Vec<u8> = r.get(6)?;
    Ok(CleanupMove {
        file: FileId(r.get(0)?),
        rel_path: rel(r.get(1)?, "cleanup path")?,
        quarantine_rel_path: rel(r.get(2)?, "quarantine path")?,
        size: r.get::<_, i64>(3)? as u64,
        mtime_ns: r.get(4)?,
        file_index: r.get::<_, Option<i64>>(5)?.map(|i| i as u64),
        content: ContentId {
            digest: digest.try_into().map_err(|_| corrupt("content id"))?,
            kind: codes::identity_kind_from(r.get(7)?),
        },
        disposition: if r.get::<_, i64>(8)? == 1 {
            Disposition::Covered
        } else {
            Disposition::Exported
        },
        covering_library_path: rel(r.get(9)?, "library path")?,
        state: MoveState::from_code(r.get(10)?).ok_or_else(|| corrupt("move state"))?,
        note: r.get(11)?,
        at_ms: r.get(12)?,
    })
}

const RUN_COLUMNS: &str =
    "id, source_id, root_path, library_id, folder, uid, state, created_at, started_at, ended_at";

fn run_row(r: &Row<'_>) -> Result<CleanupRunRecord> {
    let state: String = r.get(6)?;
    Ok(CleanupRunRecord {
        id: r.get(0)?,
        source: SourceId(r.get(1)?),
        root: r.get(2)?,
        library_id: r.get(3)?,
        folder: r
            .get::<_, Option<Vec<u8>>>(4)?
            .map(|f| rel(f, "cleanup folder"))
            .transpose()?,
        uid: r.get(5)?,
        state: CleanupRunState::parse(&state).ok_or_else(|| corrupt("cleanup run state"))?,
        created_at_ms: r.get(7)?,
        started_at_ms: r.get(8)?,
        ended_at_ms: r.get(9)?,
    })
}

fn insert_move(c: &Connection, run: i64, m: &CleanupMove) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO cleanup_move (run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns, file_index,
                                   content_id, identity_kind, disposition, covering_library_path, state, note, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        run,
        m.file.0,
        m.rel_path.as_bytes(),
        m.quarantine_rel_path.as_bytes(),
        m.size as i64,
        m.mtime_ns,
        m.file_index.map(|i| i as i64),
        &m.content.digest[..],
        codes::identity_kind(m.content.kind),
        disposition_code(m.disposition),
        m.covering_library_path.as_bytes(),
        m.state.code(),
        m.note,
        m.at_ms
    ])?;
    Ok(())
}

/// What a new run is made of; [`Writer::cleanup_create_run`] passes the new run's id to `moves`, which builds
/// the quarantine paths from it.
pub struct NewCleanupRun {
    pub source: SourceId,
    pub root: Vec<u8>,
    pub library_id: i64,
    pub folder: Option<RelPath>,
    pub uid: String,
    pub moves: Box<dyn FnOnce(i64) -> Vec<CleanupMove> + Send>,
}

impl Writer {
    /// Store a planned run and its moves in one transaction; returns the run id.
    pub fn cleanup_create_run(&self, run: NewCleanupRun) -> Result<i64> {
        self.exec(move |c| {
            c.prepare_cached(
                "INSERT INTO cleanup_run (source_id, root_path, library_id, folder, uid, state, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'planned', ?6)",
            )?
            .execute(params![
                run.source.0,
                run.root,
                run.library_id,
                run.folder.as_ref().map(RelPath::as_bytes),
                run.uid,
                now_ms()
            ])?;
            let id = c.last_insert_rowid();
            for m in (run.moves)(id) {
                insert_move(c, id, &m)?;
            }
            Ok(id)
        })
    }

    /// Set one move's state (and note); committed when this returns.
    pub fn cleanup_set_move(
        &self,
        run: i64,
        file: FileId,
        state: MoveState,
        note: Option<String>,
    ) -> Result<()> {
        self.exec(move |c| {
            let n = c
                .prepare_cached(
                    "UPDATE cleanup_move SET state = ?3, note = ?4, at = ?5 WHERE run_id = ?1 AND file_id = ?2",
                )?
                .execute(params![run, file.0, state.code(), note, now_ms()])?;
            if n == 1 {
                Ok(())
            } else {
                Err(rusqlite::Error::QueryReturnedNoRows)
            }
        })
    }

    /// Set a run's state; `running` sets the start time, `completed` and `undone` the end time.
    pub fn cleanup_set_run_state(&self, run: i64, state: CleanupRunState) -> Result<()> {
        self.exec(move |c| {
            let now = now_ms();
            c.prepare_cached(
                "UPDATE cleanup_run SET state = ?2,
                    started_at = CASE WHEN ?2 = 'running' THEN coalesce(started_at, ?3) ELSE started_at END,
                    ended_at = CASE WHEN ?2 IN ('completed', 'undone') THEN ?3 ELSE ended_at END
                 WHERE id = ?1",
            )?
            .execute(params![run, state.as_str(), now])?;
            Ok(())
        })
    }
}

impl Readers {
    /// Every cleanup run, newest first. Empty on catalogs before migration 14 opened read-only.
    pub fn cleanup_runs(&self) -> Result<Vec<CleanupRunRecord>> {
        if self.schema_version()? < crate::db::CLEANUP_MIGRATION.version {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {RUN_COLUMNS} FROM cleanup_run ORDER BY id DESC"
            ))?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(run_row(r)?);
            }
            Ok(out)
        })
    }

    pub fn cleanup_run(&self, id: i64) -> Result<Option<CleanupRunRecord>> {
        if self.schema_version()? < crate::db::CLEANUP_MIGRATION.version {
            return Ok(None);
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {RUN_COLUMNS} FROM cleanup_run WHERE id = ?1"
            ))?;
            let mut rows = stmt.query([id])?;
            rows.next()?.map(run_row).transpose()
        })
    }

    /// The moves of one run, by original path.
    pub fn cleanup_moves(&self, run: i64) -> Result<Vec<CleanupMove>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {MOVE_COLUMNS} FROM cleanup_move WHERE run_id = ?1 ORDER BY rel_path"
            ))?;
            let mut rows = stmt.query([run])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(move_row(r)?);
            }
            Ok(out)
        })
    }

    /// Files in a quarantine folder now (a move in state quarantined or restoring): file, run, quarantine
    /// path. Empty before migration 14.
    pub fn quarantined_files(&self) -> Result<Vec<(FileId, i64, RelPath)>> {
        if self.schema_version()? < crate::db::CLEANUP_MIGRATION.version {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT file_id, run_id, quarantine_rel_path FROM cleanup_move WHERE state IN (2, 4)
                 ORDER BY file_id",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push((
                    FileId(r.get(0)?),
                    r.get(1)?,
                    rel(r.get(2)?, "quarantine path")?,
                ));
            }
            Ok(out)
        })
    }

    /// Runs with a move left mid-way (moving or restoring) by a crash.
    pub fn cleanup_runs_to_reconcile(&self) -> Result<Vec<i64>> {
        if self.schema_version()? < crate::db::CLEANUP_MIGRATION.version {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT DISTINCT run_id FROM cleanup_move WHERE state IN (1, 4)
                 UNION SELECT id FROM cleanup_run WHERE state = 'running' ORDER BY 1",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
        })
    }
}
