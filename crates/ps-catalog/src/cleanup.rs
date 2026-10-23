//! Catalog storage for cleanup runs (migration 14, ADR-0061): the catalog half of the write-ahead journal.
//!
//! Every write is committed with `synchronous = FULL` when it returns ([`Writer::exec_durable`], ADR-0064), so
//! a move's intent is on disk before the rename it describes (`ps_export::quarantine`), even across a power
//! loss. This crate stores rows only; it never touches a source tree.

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

const RUN_COLUMNS: &str = "id, source_id, root_path, library_id, folder, uid, state, created_at, started_at, \
                           ended_at, source_added_at";
/// [`RUN_COLUMNS`] on a catalog before migration 15 (opened read-only by an observer).
const RUN_COLUMNS_V14: &str = "id, source_id, root_path, library_id, folder, uid, state, created_at, \
                               started_at, ended_at, NULL";

/// SQL condition: the cleanup run `r` belongs to the source that now has its id (same `added_at`).
const RUN_OF_CURRENT_SOURCE: &str =
    "EXISTS (SELECT 1 FROM source s WHERE s.id = r.source_id AND s.added_at = r.source_added_at)";

/// SQL condition on a row of `file` (unaliased): a cleanup run moved this very file into its quarantine folder
/// and it is still there (move state 2 or 4) - same source (by identity), same relative path and the stat
/// fingerprint the move recorded. File ids alone are not enough: SQLite may reuse them (ADR-0063). A macro so
/// other modules can `concat!` it into their SQL.
macro_rules! file_is_quarantined_sql {
    () => {
        "EXISTS (SELECT 1 FROM cleanup_move m \
         JOIN cleanup_run r ON r.id = m.run_id \
         JOIN source s ON s.id = r.source_id AND s.added_at = r.source_added_at \
         WHERE m.rel_path = file.rel_path AND m.state IN (2, 4) AND r.source_id = file.source_id \
         AND m.size = file.size AND m.mtime_ns = file.mtime_ns)"
    };
}
pub(crate) use file_is_quarantined_sql;

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
        source_added_at_ms: r.get(10)?,
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
    /// The source's `added_at` (its identity with the id).
    pub source_added_at_ms: i64,
    pub root: Vec<u8>,
    pub library_id: i64,
    pub folder: Option<RelPath>,
    pub uid: String,
    pub moves: Box<dyn FnOnce(i64) -> Vec<CleanupMove> + Send>,
}

/// A file a cleanup run moved into its source's quarantine folder that is still there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarantinedFile {
    pub file: FileId,
    pub run: i64,
    pub source: SourceId,
    /// Its original path, relative to the source root.
    pub rel_path: RelPath,
    pub quarantine_rel_path: RelPath,
    /// The stat fingerprint it had when it was moved (a catalog row with another fingerprint at `rel_path`
    /// describes a different file).
    pub size: u64,
    pub mtime_ns: i64,
}

impl Writer {
    /// Store a planned run and its moves in one transaction; returns the run id. Like every write of this
    /// module it is committed with `synchronous = FULL` ([`Writer::exec_durable`], ADR-0064).
    pub fn cleanup_create_run(&self, run: NewCleanupRun) -> Result<i64> {
        self.exec_durable(move |c| {
            c.prepare_cached(
                "INSERT INTO cleanup_run (source_id, source_added_at, root_path, library_id, folder, uid, state,
                                          created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'planned', ?7)",
            )?
            .execute(params![
                run.source.0,
                run.source_added_at_ms,
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

    /// Set one move's state (and note); durably committed when this returns (the write-ahead intent of a
    /// rename must survive a power loss, ADR-0064).
    pub fn cleanup_set_move(
        &self,
        run: i64,
        file: FileId,
        state: MoveState,
        note: Option<String>,
    ) -> Result<()> {
        self.exec_durable(move |c| {
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

    /// Set a run's selection (ADR-0068): every move of `run` in state planned (0) or unselected (6) becomes
    /// planned when `picked` names it and unselected otherwise; moves in any other state are untouched. One
    /// durable transaction; the ids go through a temporary table, so a million of them is fine. Returns how
    /// many moves are planned afterwards.
    pub fn cleanup_select_moves(&self, run: i64, picked: Vec<FileId>) -> Result<u64> {
        self.exec_durable(move |c| {
            c.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS cleanup_pick (file_id INTEGER PRIMARY KEY) WITHOUT ROWID;
                 DELETE FROM temp.cleanup_pick;",
            )?;
            {
                let mut ins =
                    c.prepare_cached("INSERT OR IGNORE INTO temp.cleanup_pick (file_id) VALUES (?1)")?;
                for f in &picked {
                    ins.execute([f.0])?;
                }
            }
            c.prepare_cached(
                "UPDATE cleanup_move
                 SET state = CASE WHEN file_id IN (SELECT file_id FROM temp.cleanup_pick) THEN 0 ELSE 6 END,
                     at = ?2
                 WHERE run_id = ?1 AND state IN (0, 6)
                   AND state <> CASE WHEN file_id IN (SELECT file_id FROM temp.cleanup_pick) THEN 0 ELSE 6 END",
            )?
            .execute(params![run, now_ms()])?;
            c.execute_batch("DELETE FROM temp.cleanup_pick;")?;
            Ok(c.prepare_cached("SELECT count(*) FROM cleanup_move WHERE run_id = ?1 AND state = 0")?
                .query_row([run], |r| r.get::<_, i64>(0))? as u64)
        })
    }

    /// Discard a run (ADR-0068): its planned moves become unselected and the run `discarded` with its end time,
    /// in one durable transaction. Only a `planned` run with no move in state 1, 2, 4 or 5 changes; returns
    /// whether it did. Touches catalog rows only.
    pub fn cleanup_discard_run(&self, run: i64) -> Result<bool> {
        self.exec_durable(move |c| {
            let n = c
                .prepare_cached(
                    "UPDATE cleanup_run SET state = 'discarded', ended_at = ?2
                     WHERE id = ?1 AND state = 'planned'
                       AND NOT EXISTS (SELECT 1 FROM cleanup_move WHERE run_id = ?1 AND state IN (1, 2, 4, 5))",
                )?
                .execute(params![run, now_ms()])?;
            if n == 1 {
                c.prepare_cached(
                    "UPDATE cleanup_move SET state = 6, at = ?2 WHERE run_id = ?1 AND state = 0",
                )?
                .execute(params![run, now_ms()])?;
            }
            Ok(n == 1)
        })
    }

    /// Set a run's state; `running` sets the start time, `completed` and `undone` the end time.
    pub fn cleanup_set_run_state(&self, run: i64, state: CleanupRunState) -> Result<()> {
        self.exec_durable(move |c| {
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
    fn run_columns(&self) -> Result<&'static str> {
        Ok(
            if self.schema_version()? < crate::db::CLEANUP_SOURCE_MIGRATION.version {
                RUN_COLUMNS_V14
            } else {
                RUN_COLUMNS
            },
        )
    }

    /// Every cleanup run, newest first. Empty on catalogs before migration 14 opened read-only.
    pub fn cleanup_runs(&self) -> Result<Vec<CleanupRunRecord>> {
        if self.schema_version()? < crate::db::CLEANUP_MIGRATION.version {
            return Ok(Vec::new());
        }
        let cols = self.run_columns()?;
        self.with(|c| {
            let mut stmt =
                c.prepare_cached(&format!("SELECT {cols} FROM cleanup_run ORDER BY id DESC"))?;
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
        let cols = self.run_columns()?;
        self.with(|c| {
            let mut stmt =
                c.prepare_cached(&format!("SELECT {cols} FROM cleanup_run WHERE id = ?1"))?;
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

    /// The moves of one run whose file lies directly in `folder` (`None`: the source root), by path. Served by
    /// the path index (a range of paths), so expanding one folder of a huge plan reads only that folder.
    pub fn cleanup_moves_in_folder(
        &self,
        run: i64,
        folder: Option<&RelPath>,
    ) -> Result<Vec<CleanupMove>> {
        let folder = folder.cloned();
        self.with(move |c| {
            let mut out = Vec::new();
            match &folder {
                Some(f) => {
                    let mut lo = f.as_bytes().to_vec();
                    lo.push(b'/');
                    let mut hi = f.as_bytes().to_vec();
                    hi.push(b'/' + 1);
                    let mut stmt = c.prepare_cached(&format!(
                        "SELECT {MOVE_COLUMNS} FROM cleanup_move
                         WHERE rel_path > ?2 AND rel_path < ?3 AND run_id = ?1 ORDER BY rel_path"
                    ))?;
                    let mut rows = stmt.query(params![run, lo, hi])?;
                    while let Some(r) = rows.next()? {
                        let m = move_row(r)?;
                        if m.rel_path.parent().as_ref() == Some(f) {
                            out.push(m);
                        }
                    }
                }
                None => {
                    let mut stmt = c.prepare_cached(&format!(
                        "SELECT {MOVE_COLUMNS} FROM cleanup_move
                         WHERE run_id = ?1 AND instr(rel_path, x'2f') = 0 ORDER BY rel_path"
                    ))?;
                    let mut rows = stmt.query([run])?;
                    while let Some(r) = rows.next()? {
                        out.push(move_row(r)?);
                    }
                }
            }
            Ok(out)
        })
    }

    /// Files in a quarantine folder now (a move in state quarantined or restoring) of runs whose source is
    /// still the one they were planned on, by file id. Empty before migration 15 (catalogs opened read-only
    /// at an older version).
    pub fn quarantined_files(&self) -> Result<Vec<QuarantinedFile>> {
        if self.schema_version()? < crate::db::CLEANUP_SOURCE_MIGRATION.version {
            return Ok(Vec::new());
        }
        self.with(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT m.file_id, m.run_id, r.source_id, m.rel_path, m.quarantine_rel_path, m.size, m.mtime_ns
                 FROM cleanup_move m JOIN cleanup_run r ON r.id = m.run_id
                 WHERE m.state IN (2, 4) AND {RUN_OF_CURRENT_SOURCE}
                 ORDER BY m.file_id"
            ))?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(QuarantinedFile {
                    file: FileId(r.get(0)?),
                    run: r.get(1)?,
                    source: SourceId(r.get(2)?),
                    rel_path: rel(r.get(3)?, "cleanup path")?,
                    quarantine_rel_path: rel(r.get(4)?, "quarantine path")?,
                    size: r.get::<_, i64>(5)? as u64,
                    mtime_ns: r.get(6)?,
                });
            }
            Ok(out)
        })
    }

    /// How many files of `source` are in its quarantine folder now (runs of that very source, by identity).
    /// A source with quarantined files is never removed or folded (ADR-0063): its runs must stay undoable.
    pub fn quarantined_count(&self, source: SourceId) -> Result<u64> {
        if self.schema_version()? < crate::db::CLEANUP_MIGRATION.version {
            return Ok(0);
        }
        // Before migration 15 there is no identity: count by id.
        let identity = if self.schema_version()? < crate::db::CLEANUP_SOURCE_MIGRATION.version {
            "1"
        } else {
            RUN_OF_CURRENT_SOURCE
        };
        self.with(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT count(*) FROM cleanup_move m JOIN cleanup_run r ON r.id = m.run_id
                 WHERE r.source_id = ?1 AND m.state IN (1, 2, 4) AND {identity}"
            ))?
            .query_row([source.0], |r| r.get::<_, i64>(0))? as u64)
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn rel(s: &str) -> RelPath {
        RelPath::from_bytes(s.as_bytes().to_vec()).unwrap()
    }

    fn a_move(file: i64, state: MoveState) -> CleanupMove {
        let path = format!("DCIM/{file}.jpg");
        CleanupMove {
            file: FileId(file),
            quarantine_rel_path: rel(&format!(".photosquirrel-quarantine/1/{path}")),
            rel_path: rel(&path),
            size: 10,
            mtime_ns: 5,
            file_index: None,
            content: ContentId::full([1; 32]),
            disposition: Disposition::Exported,
            covering_library_path: rel("2011/x.jpg"),
            state,
            note: None,
            at_ms: None,
        }
    }

    fn run_with(cat: &crate::Catalog, moves: Vec<CleanupMove>) -> i64 {
        let lib = cat.writer().export_library(b"/lib".to_vec(), None).unwrap();
        cat.writer()
            .cleanup_create_run(NewCleanupRun {
                source: SourceId(1),
                source_added_at_ms: 1,
                root: b"/a".to_vec(),
                library_id: lib,
                folder: None,
                uid: "u".into(),
                moves: Box::new(move |_| moves),
            })
            .unwrap()
    }

    fn states(cat: &crate::Catalog, run: i64) -> Vec<(i64, MoveState)> {
        cat.read()
            .cleanup_moves(run)
            .unwrap()
            .into_iter()
            .map(|m| (m.file.0, m.state))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .collect()
    }

    /// Selecting only flips planned and unselected moves; every other state is left alone.
    #[test]
    fn select_changes_only_planned_and_unselected_moves() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let all = [
            MoveState::Planned,
            MoveState::Moving,
            MoveState::Quarantined,
            MoveState::Skipped,
            MoveState::Restoring,
            MoveState::Restored,
            MoveState::Unselected,
            MoveState::Planned,
        ];
        let run = run_with(
            &cat,
            all.iter()
                .enumerate()
                .map(|(i, s)| a_move(i as i64 + 1, *s))
                .collect(),
        );
        // Pick everything except file 1: 1 becomes unselected, 7 planned, 8 stays planned.
        let picked: Vec<FileId> = (2..=8).map(FileId).collect();
        assert_eq!(cat.writer().cleanup_select_moves(run, picked).unwrap(), 2);
        let want = [
            MoveState::Unselected,
            MoveState::Moving,
            MoveState::Quarantined,
            MoveState::Skipped,
            MoveState::Restoring,
            MoveState::Restored,
            MoveState::Planned,
            MoveState::Planned,
        ];
        assert_eq!(
            states(&cat, run),
            want.iter()
                .enumerate()
                .map(|(i, s)| (i as i64 + 1, *s))
                .collect::<Vec<_>>()
        );
        // Picking nothing unselects the planned ones only.
        assert_eq!(
            cat.writer().cleanup_select_moves(run, Vec::new()).unwrap(),
            0
        );
        let s = states(&cat, run);
        assert_eq!(s[6].1, MoveState::Unselected);
        assert_eq!(s[2].1, MoveState::Quarantined);
    }

    #[test]
    fn select_takes_100k_ids_in_one_call() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let run = run_with(
            &cat,
            (1..=100_000)
                .map(|i| a_move(i, MoveState::Planned))
                .collect(),
        );
        let picked: Vec<FileId> = (1..=100_000).filter(|i| i % 2 == 0).map(FileId).collect();
        assert_eq!(
            cat.writer().cleanup_select_moves(run, picked).unwrap(),
            50_000
        );
        let moves = cat.read().cleanup_moves(run).unwrap();
        assert!(
            moves
                .iter()
                .all(|m| (m.file.0 % 2 == 0) == (m.state == MoveState::Planned))
        );
        assert!(
            moves
                .iter()
                .all(|m| matches!(m.state, MoveState::Planned | MoveState::Unselected))
        );
    }

    #[test]
    fn discard_marks_the_run_and_its_planned_moves() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let run = run_with(
            &cat,
            vec![
                a_move(1, MoveState::Planned),
                a_move(2, MoveState::Skipped),
                a_move(3, MoveState::Unselected),
            ],
        );
        assert!(cat.writer().cleanup_discard_run(run).unwrap());
        let r = cat.read().cleanup_run(run).unwrap().unwrap();
        assert_eq!(r.state, CleanupRunState::Discarded);
        assert!(r.ended_at_ms.is_some());
        assert_eq!(
            states(&cat, run),
            vec![
                (1, MoveState::Unselected),
                (2, MoveState::Skipped),
                (3, MoveState::Unselected)
            ]
        );
        // Not twice, and never a run that moved a file.
        assert!(!cat.writer().cleanup_discard_run(run).unwrap());
        for s in [
            MoveState::Moving,
            MoveState::Quarantined,
            MoveState::Restoring,
            MoveState::Restored,
        ] {
            let other = run_with(&cat, vec![a_move(1, MoveState::Planned), a_move(2, s)]);
            assert!(!cat.writer().cleanup_discard_run(other).unwrap(), "{s:?}");
            assert_eq!(
                cat.read().cleanup_run(other).unwrap().unwrap().state,
                CleanupRunState::Planned
            );
            assert_eq!(states(&cat, other)[0].1, MoveState::Planned);
        }
    }

    #[test]
    fn moves_of_one_folder_only() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let mut moves: Vec<CleanupMove> = Vec::new();
        for (i, p) in [
            "a.jpg",
            "DCIM/b.jpg",
            "DCIM/c.jpg",
            "DCIM/100/d.jpg",
            "DCIMX/e.jpg",
            "DCIM 2/f.jpg",
        ]
        .iter()
        .enumerate()
        {
            let mut m = a_move(i as i64 + 1, MoveState::Planned);
            m.rel_path = rel(p);
            moves.push(m);
        }
        let run = run_with(&cat, moves);
        let names = |f: Option<&str>| -> Vec<String> {
            cat.read()
                .cleanup_moves_in_folder(run, f.map(rel).as_ref())
                .unwrap()
                .iter()
                .map(|m| m.rel_path.to_string())
                .collect()
        };
        assert_eq!(names(Some("DCIM")), vec!["DCIM/b.jpg", "DCIM/c.jpg"]);
        assert_eq!(names(Some("DCIM/100")), vec!["DCIM/100/d.jpg"]);
        assert_eq!(names(None), vec!["a.jpg"]);
        assert!(names(Some("Nope")).is_empty());
    }

    /// Selection and discard are durable writes (they go through `exec_durable`, counted by the writer).
    #[test]
    fn select_and_discard_are_durable() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let run = run_with(&cat, vec![a_move(1, MoveState::Planned)]);
        let before = cat.writer().stats().durable_commits;
        cat.writer()
            .cleanup_select_moves(run, vec![FileId(1)])
            .unwrap();
        cat.writer().cleanup_discard_run(run).unwrap();
        assert_eq!(cat.writer().stats().durable_commits, before + 2);
    }

    /// `exec_durable` commits with `synchronous = FULL` (2) and leaves the writer at `NORMAL` (1) after.
    #[test]
    fn durable_writes_use_synchronous_full() {
        let dir = tempfile::tempdir().unwrap();
        let cat = crate::Catalog::open(dir.path()).unwrap();
        let pragma = |c: &rusqlite::Connection| {
            c.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
        };
        assert_eq!(cat.writer().exec_durable(pragma).unwrap(), 2);
        assert_eq!(cat.writer().exec(pragma).unwrap(), 1);
    }
}
