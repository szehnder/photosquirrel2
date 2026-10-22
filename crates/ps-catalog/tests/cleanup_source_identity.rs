//! Migration 15 (ADR-0063): cleanup runs know their source by identity, source ids a run refers to are never
//! handed out again, and quarantined files are found by source and path, not by a reusable file id.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_catalog::{Catalog, Command, NewCleanupRun, NewSource, db};
use ps_model::{CleanupMove, ContentId, Disposition, FileId, MoveState, RelPath, SourceId};
use rusqlite::{Connection, params};

fn rel(s: &str) -> RelPath {
    RelPath::from_bytes(s.as_bytes().to_vec()).unwrap()
}

fn new_source(root: &str) -> NewSource {
    NewSource {
        root: root.as_bytes().to_vec(),
        volume_uuid: None,
        device_hint: None,
        total_bytes: None,
        fingerprint: None,
    }
}

fn a_move(file: i64, path: &str, run: i64) -> CleanupMove {
    CleanupMove {
        file: FileId(file),
        rel_path: rel(path),
        quarantine_rel_path: rel(&format!(".photosquirrel-quarantine/{run}/{path}")),
        size: 10,
        mtime_ns: 5,
        file_index: None,
        content: ContentId::full([1; 32]),
        disposition: Disposition::Exported,
        covering_library_path: rel("2011/x.jpg"),
        state: MoveState::Planned,
        note: None,
        at_ms: None,
    }
}

#[test]
fn migration_15_backfills_the_source_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, 14).unwrap();
    {
        let c = Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
            params![b"/lib".to_vec()],
        )
        .unwrap();
        // Source 1 was added before its run; source 2 (the id of a removed source, reused) after its run.
        c.execute_batch(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, x'2f61', 100), (2, x'2f62', 900);
             INSERT INTO cleanup_run (id, source_id, root_path, library_id, uid, state, created_at)
                 VALUES (1, 1, x'2f61', 1, 'u1', 'completed', 500), (2, 2, x'2f6f6c64', 1, 'u2', 'completed', 500);",
        )
        .unwrap();
    }
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    let r = cat.read();
    assert_eq!(
        r.cleanup_run(1).unwrap().unwrap().source_added_at_ms,
        Some(100)
    );
    assert_eq!(
        r.cleanup_run(2).unwrap().unwrap().source_added_at_ms,
        None,
        "a source added after the run reuses the id; it is not the run's source"
    );
}

#[test]
fn source_ids_of_cleanup_runs_are_never_reused() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path()).unwrap();
    let lib = cat.writer().export_library(b"/lib".to_vec(), None).unwrap();
    let a = cat.writer().add_source(new_source("/a")).unwrap();
    let c = cat.writer().add_source(new_source("/c")).unwrap();
    let added = cat
        .read()
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.id == c)
        .unwrap()
        .added_at_ms;
    cat.writer()
        .cleanup_create_run(NewCleanupRun {
            source: c,
            source_added_at_ms: added,
            root: b"/c".to_vec(),
            library_id: lib,
            folder: None,
            uid: "u".into(),
            moves: Box::new(|id| vec![a_move(1, "x.jpg", id)]),
        })
        .unwrap();
    // Remove the newest source (its id is the highest) and add another folder: it gets a new id.
    cat.writer()
        .submit(Command::RemoveSource { source: c })
        .unwrap();
    cat.writer().flush().unwrap();
    let d = cat.writer().add_source(new_source("/d")).unwrap();
    assert_ne!(
        d, c,
        "a removed source's id that a cleanup run names is not reused"
    );
    assert!(d.0 > c.0 && a.0 < c.0);
}

#[test]
fn quarantined_files_match_by_source_identity_and_path() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path()).unwrap();
    let lib = cat.writer().export_library(b"/lib".to_vec(), None).unwrap();
    let s = cat.writer().add_source(new_source("/s")).unwrap();
    let added = cat.read().sources().unwrap()[0].added_at_ms;
    let run = |added_at: i64| {
        cat.writer()
            .cleanup_create_run(NewCleanupRun {
                source: s,
                source_added_at_ms: added_at,
                root: b"/s".to_vec(),
                library_id: lib,
                folder: None,
                uid: "u".into(),
                moves: Box::new(|id| vec![a_move(1, "DCIM/a.jpg", id)]),
            })
            .unwrap()
    };
    let ours = run(added);
    let foreign = run(added - 1);
    for id in [ours, foreign] {
        cat.writer()
            .cleanup_set_move(id, FileId(1), MoveState::Quarantined, None)
            .unwrap();
    }
    let q = cat.read().quarantined_files().unwrap();
    assert_eq!(q.len(), 1, "only the run of this very source counts: {q:?}");
    assert_eq!(q[0].run, ours);
    assert_eq!(cat.read().quarantined_count(s).unwrap(), 1);
    assert_eq!(cat.read().quarantined_count(SourceId(99)).unwrap(), 0);
}

#[test]
fn cleanup_writes_are_committed_durably() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path()).unwrap();
    let lib = cat.writer().export_library(b"/lib".to_vec(), None).unwrap();
    let s = cat.writer().add_source(new_source("/s")).unwrap();
    let before = cat.writer().stats().durable_commits;
    let id = cat
        .writer()
        .cleanup_create_run(NewCleanupRun {
            source: s,
            source_added_at_ms: 0,
            root: b"/s".to_vec(),
            library_id: lib,
            folder: None,
            uid: "u".into(),
            moves: Box::new(|id| vec![a_move(1, "a.jpg", id)]),
        })
        .unwrap();
    cat.writer()
        .cleanup_set_move(id, FileId(1), MoveState::Moving, None)
        .unwrap();
    cat.writer()
        .cleanup_set_run_state(id, ps_model::CleanupRunState::Running)
        .unwrap();
    assert_eq!(cat.writer().stats().durable_commits - before, 3);
}

/// Make every commit that sets a move of `file` to `moving` fail: the trigger writes a row whose deferred
/// foreign key is checked, and fails, at `COMMIT` (the writer connection has `foreign_keys = ON`). A real
/// commit failure, without a hook in the writer.
fn fail_commits_of_moving_intent(catalog_db: &std::path::Path, file: i64) {
    let c = Connection::open(catalog_db).unwrap();
    c.execute_batch(&format!(
        "CREATE TABLE fault_parent (id INTEGER PRIMARY KEY);
         CREATE TABLE fault_child (parent INTEGER REFERENCES fault_parent (id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER fault_on_moving AFTER UPDATE OF state ON cleanup_move
         WHEN NEW.state = 1 AND NEW.file_id = {file}
         BEGIN INSERT INTO fault_child (parent) VALUES (42); END;"
    ))
    .unwrap();
}

/// A durable write whose commit fails is an error, not a silent success: the cleanup run would otherwise
/// rename a file with no intent in the catalog. The writer stays usable afterwards.
#[test]
fn a_failed_durable_commit_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let cat = Catalog::open(dir.path()).unwrap();
        let lib = cat.writer().export_library(b"/lib".to_vec(), None).unwrap();
        let s = cat.writer().add_source(new_source("/s")).unwrap();
        let id = cat
            .writer()
            .cleanup_create_run(NewCleanupRun {
                source: s,
                source_added_at_ms: 0,
                root: b"/s".to_vec(),
                library_id: lib,
                folder: None,
                uid: "u".into(),
                moves: Box::new(|id| vec![a_move(1, "a.jpg", id), a_move(2, "b.jpg", id)]),
            })
            .unwrap();
        cat.close().unwrap();
        id
    };
    fail_commits_of_moving_intent(&dir.path().join(ps_catalog::CATALOG_FILE), 1);
    let cat = Catalog::open(dir.path()).unwrap();
    let before = cat.writer().stats();
    let r = cat
        .writer()
        .cleanup_set_move(id, FileId(1), MoveState::Moving, None);
    assert!(
        matches!(r, Err(ps_catalog::CatalogError::Write(_))),
        "a failed commit must fail the write: {r:?}"
    );
    let after = cat.writer().stats();
    assert_eq!(after.durable_commits, before.durable_commits);
    assert_eq!(after.commits, before.commits);
    let moves = cat.read().cleanup_moves(id).unwrap();
    assert_eq!(moves[0].state, MoveState::Planned, "rolled back");
    // Other writes still commit.
    cat.writer()
        .cleanup_set_move(id, FileId(2), MoveState::Moving, None)
        .unwrap();
    assert_eq!(
        cat.read().cleanup_moves(id).unwrap()[1].state,
        MoveState::Moving
    );
}
