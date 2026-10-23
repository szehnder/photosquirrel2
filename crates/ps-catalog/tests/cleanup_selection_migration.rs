//! Migration 16 (ADR-0068): both cleanup tables are rebuilt to accept run state `discarded` and move state 6
//! (`unselected`), keeping every row, both indexes and the foreign key.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_catalog::{Catalog, db};
use ps_model::{CleanupRunState, MoveState};
use rusqlite::{Connection, params};

use rusqlite::types::Value;

/// Every column of every row of `sql`, as SQLite values.
fn rows(c: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    s.query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn moves(c: &Connection) -> Vec<Vec<Value>> {
    rows(
        c,
        "SELECT run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns, file_index, content_id,
                identity_kind, disposition, covering_library_path, state, note, at
         FROM cleanup_move ORDER BY run_id, file_id",
    )
}

fn runs(c: &Connection) -> Vec<Vec<Value>> {
    rows(
        c,
        "SELECT id, source_id, root_path, library_id, folder, uid, state, created_at, started_at, ended_at,
                source_added_at FROM cleanup_run ORDER BY id",
    )
}

#[test]
fn migration_16_keeps_every_cleanup_row_and_widens_the_states() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, 15).unwrap();
    let (runs_before, moves_before) = {
        let c = Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
            params![b"/lib".to_vec()],
        )
        .unwrap();
        c.execute_batch("INSERT INTO source (id, root_path, added_at) VALUES (1, x'2f61', 100);")
            .unwrap();
        // A run in every state of migration 14, each with a move in every code 0-5.
        for (id, state) in [
            (1, "planned"),
            (2, "running"),
            (3, "completed"),
            (4, "undone"),
        ] {
            c.execute(
                "INSERT INTO cleanup_run (id, source_id, root_path, library_id, folder, uid, state, created_at,
                                          started_at, ended_at, source_added_at)
                 VALUES (?1, 1, x'2f61', 1, ?2, ?3, ?4, 500, ?5, NULL, 100)",
                params![
                    id,
                    (id == 2).then(|| b"DCIM".to_vec()),
                    format!("uid-{id}"),
                    state,
                    (id > 1).then_some(600)
                ],
            )
            .unwrap();
            for code in 0..=5i64 {
                c.execute(
                    "INSERT INTO cleanup_move (run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns,
                                               file_index, content_id, identity_kind, disposition,
                                               covering_library_path, state, note, at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 7, ?6, ?7, ?8, ?9, x'6c6962', ?10, ?11, ?12)",
                    params![
                        id,
                        code + 10,
                        format!("DCIM/{code}.jpg").into_bytes(),
                        format!(".photosquirrel-quarantine/{id}/DCIM/{code}.jpg").into_bytes(),
                        1000 + code,
                        (code % 2 == 0).then_some(code),
                        vec![code as u8; 32],
                        code % 2,
                        (code / 2) % 2,
                        code,
                        (code == 3).then(|| "changed".to_owned()),
                        (code > 0).then_some(900 + code)
                    ],
                )
                .unwrap();
            }
        }
        (runs(&c), moves(&c))
    };
    assert_eq!(runs_before.len(), 4);
    assert_eq!(moves_before.len(), 24);

    // Open (migrates to the latest version), then check through a plain connection.
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    assert_eq!(db::stored_version(&path).unwrap(), Some(16));
    let read = cat.read();
    assert_eq!(read.cleanup_runs().unwrap().len(), 4);
    assert_eq!(read.cleanup_moves(3).unwrap().len(), 6);
    drop(cat);

    let c = Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    assert_eq!(runs(&c), runs_before, "every run row unchanged");
    assert_eq!(moves(&c), moves_before, "every move row unchanged");
    let fk_problems: i64 = c
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk_problems, 0);
    let indexes: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'cleanup_move' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        indexes,
        vec!["cleanup_move_by_path", "cleanup_move_by_state"]
    );
    let leftovers: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE '%_old'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leftovers, 0, "the old tables are gone");
    // The foreign key still points at the rebuilt run table.
    let parent: String = c
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_list('cleanup_move')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, "cleanup_run");
    assert!(
        c.execute(
            "INSERT INTO cleanup_move (run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns, content_id,
                                       identity_kind, disposition, covering_library_path, state)
             VALUES (99, 1, x'61', x'62', 1, 1, x'00', 0, 0, x'63', 0)",
            [],
        )
        .is_err(),
        "a move of an unknown run is refused"
    );

    // The new states are accepted; unknown ones are still refused.
    c.execute(
        "UPDATE cleanup_run SET state = 'discarded' WHERE id = 1",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE cleanup_move SET state = ?1 WHERE run_id = 1 AND file_id = 10",
        [MoveState::Unselected.code()],
    )
    .unwrap();
    assert!(
        c.execute("UPDATE cleanup_run SET state = 'deleted' WHERE id = 1", [])
            .is_err()
    );
    assert!(
        c.execute("UPDATE cleanup_move SET state = 7 WHERE run_id = 1", [])
            .is_err()
    );
    drop(c);
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    let r = cat.read();
    assert_eq!(
        r.cleanup_run(1).unwrap().unwrap().state,
        CleanupRunState::Discarded
    );
    assert_eq!(
        r.cleanup_moves(1)
            .unwrap()
            .iter()
            .find(|m| m.file.0 == 10)
            .unwrap()
            .state,
        MoveState::Unselected
    );
}
