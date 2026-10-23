//! Migration 14 (quarantine-cleanup, ADR-0061): a version-13 catalog upgrades, keeps its rows, and the cleanup
//! tables enforce their keys and codes; the typed reads and writes round-trip.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_catalog::{NewCleanupRun, db, latest_schema_version};
use ps_model::{CleanupMove, CleanupRunState, ContentId, Disposition, FileId, MoveState, RelPath};
use rusqlite::{Connection, params};

fn rel(s: &str) -> RelPath {
    RelPath::from_bytes(s.as_bytes().to_vec()).unwrap()
}

#[test]
fn cleanup_migration_upgrades_version_13() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    assert_eq!(db::CLEANUP_MIGRATION.version, 14);
    db::create_catalog_at_version(&path, 13).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
            params![b"/lib".to_vec()],
        )
        .unwrap();
        assert!(
            conn.execute("SELECT count(*) FROM cleanup_run", [])
                .is_err(),
            "no cleanup tables before migration 14"
        );
    }
    ps_catalog::Catalog::open_file(&path, Default::default())
        .unwrap()
        .close()
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let v: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, latest_schema_version());
    let n: i64 = conn
        .query_row("SELECT count(*) FROM export_library", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "existing rows kept");

    let run = |state: &str| {
        conn.execute(
            "INSERT INTO cleanup_run (source_id, root_path, library_id, uid, state, created_at)
             VALUES (7, ?1, 1, 'u', ?2, 0)",
            params![b"/src".to_vec(), state],
        )
    };
    run("planned").unwrap();
    assert!(run("deleted").is_err(), "run states are checked");
    let mv = |file: i64, state: i64, disposition: i64| {
        conn.execute(
            "INSERT INTO cleanup_move (run_id, file_id, rel_path, quarantine_rel_path, size, mtime_ns, content_id,
                                       identity_kind, disposition, covering_library_path, state)
             VALUES (1, ?1, ?2, ?3, 10, 0, ?4, 0, ?5, ?6, ?7)",
            params![
                file,
                b"a.jpg".to_vec(),
                b".photosquirrel-quarantine/1/a.jpg".to_vec(),
                vec![7u8; 32],
                disposition,
                b"2011/a.jpg".to_vec(),
                state
            ],
        )
    };
    // file_id has no foreign key: file 42 does not exist and the row stands.
    mv(42, 0, 0).unwrap();
    assert!(mv(42, 0, 0).is_err(), "one move per file and run");
    assert!(
        mv(43, 7, 0).is_err(),
        "move states are checked (0-6 since migration 16)"
    );
    assert!(
        mv(44, 0, 2).is_err(),
        "only exported or covered files are moved"
    );
    mv(45, 5, 1).unwrap();
}

#[test]
fn cleanup_rows_round_trip_through_writer_and_readers() {
    let dir = tempfile::tempdir().unwrap();
    let cat = ps_catalog::Catalog::open(dir.path()).unwrap();
    let lib = cat.writer().export_library(b"/lib".to_vec(), None).unwrap();
    let m = |i: i64, p: &str| CleanupMove {
        file: FileId(i),
        rel_path: rel(p),
        quarantine_rel_path: rel(p),
        size: 10 + i as u64,
        mtime_ns: 5,
        file_index: Some(99),
        content: ContentId::full([i as u8; 32]),
        disposition: if i == 1 {
            Disposition::Exported
        } else {
            Disposition::Covered
        },
        covering_library_path: rel("2011/x.jpg"),
        state: MoveState::Planned,
        note: None,
        at_ms: None,
    };
    let source = cat
        .writer()
        .add_source(ps_catalog::NewSource {
            root: b"/src".to_vec(),
            volume_uuid: None,
            device_hint: None,
            total_bytes: None,
            fingerprint: None,
        })
        .unwrap();
    let added_at = cat.read().sources().unwrap()[0].added_at_ms;
    let id = cat
        .writer()
        .cleanup_create_run(NewCleanupRun {
            source,
            source_added_at_ms: added_at,
            root: b"/src".to_vec(),
            library_id: lib,
            folder: Some(rel("DCIM")),
            uid: "uid-1".into(),
            moves: Box::new(move |id| {
                vec![
                    CleanupMove {
                        quarantine_rel_path: rel(&format!(
                            ".photosquirrel-quarantine/{id}/DCIM/b.jpg"
                        )),
                        ..m(2, "DCIM/b.jpg")
                    },
                    m(1, "DCIM/a.jpg"),
                ]
            }),
        })
        .unwrap();
    let r = cat.read();
    let run = r.cleanup_run(id).unwrap().unwrap();
    assert_eq!(run.state, CleanupRunState::Planned);
    assert_eq!(run.folder, Some(rel("DCIM")));
    assert_eq!(run.uid, "uid-1");
    assert_eq!(run.source_added_at_ms, Some(added_at));
    let moves = r.cleanup_moves(id).unwrap();
    assert_eq!(moves.len(), 2);
    assert_eq!(moves[0].rel_path, rel("DCIM/a.jpg"));
    assert_eq!(moves[0].disposition, Disposition::Exported);
    assert_eq!(moves[1].disposition, Disposition::Covered);
    assert_eq!(
        moves[1].quarantine_rel_path,
        rel(&format!(".photosquirrel-quarantine/{id}/DCIM/b.jpg"))
    );
    assert_eq!(moves[1].file_index, Some(99));

    cat.writer()
        .cleanup_set_move(id, FileId(2), MoveState::Moving, None)
        .unwrap();
    assert_eq!(r.cleanup_runs_to_reconcile().unwrap(), vec![id]);
    cat.writer()
        .cleanup_set_move(id, FileId(2), MoveState::Quarantined, None)
        .unwrap();
    cat.writer()
        .cleanup_set_move(id, FileId(1), MoveState::Skipped, Some("changed".into()))
        .unwrap();
    assert!(
        cat.writer()
            .cleanup_set_move(id, FileId(9), MoveState::Skipped, None)
            .is_err(),
        "unknown move"
    );
    cat.writer()
        .cleanup_set_run_state(id, CleanupRunState::Completed)
        .unwrap();
    assert!(r.cleanup_runs_to_reconcile().unwrap().is_empty());
    let q = r.quarantined_files().unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].file, FileId(2));
    assert_eq!((q[0].source, &q[0].rel_path), (source, &rel("DCIM/b.jpg")));
    assert_eq!(r.quarantined_count(source).unwrap(), 1);
    let moves = r.cleanup_moves(id).unwrap();
    assert_eq!(moves[0].note.as_deref(), Some("changed"));
    assert!(moves[0].at_ms.is_some());
    let run = r.cleanup_run(id).unwrap().unwrap();
    assert_eq!(run.state, CleanupRunState::Completed);
    assert!(run.ended_at_ms.is_some());
    assert_eq!(r.cleanup_runs().unwrap().len(), 1);
}
