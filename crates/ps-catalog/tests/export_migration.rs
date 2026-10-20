//! Migration 6 (export ledger): a version-5 catalog upgrades through it, and the ledger enforces its keys.

use ps_catalog::{db, latest_schema_version};
use rusqlite::{Connection, params};

#[test]
fn export_migration_applies_on_latest_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    assert!(latest_schema_version() >= db::EXPORT_MIGRATION.version);
    db::create_catalog_at_version(&path, db::EXPORT_MIGRATION.version - 1).expect("catalog");
    ps_catalog::Catalog::open_file(&path, Default::default())
        .expect("upgrade")
        .close()
        .expect("close");

    let conn = Connection::open(&path).expect("open");
    conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");

    conn.execute(
        "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
        params![b"/lib".to_vec()],
    )
    .expect("library");
    let insert = |path: &[u8], state: i64, profile: &str| {
        conn.execute(
            "INSERT INTO export_ledger (library_id, content_id, identity_kind, full_hash, file_id,
                 library_rel_path, size, state, profile, at)
             VALUES (1, ?1, 0, ?1, 42, ?2, 10, ?3, ?4, 0)",
            params![vec![7u8; 32], path.to_vec(), state, profile],
        )
    };
    insert(b"2011/2011-08-14/IMG_0042.JPG", 1, "lightroom").expect("row");
    // One row per library path.
    assert!(insert(b"2011/2011-08-14/IMG_0042.JPG", 1, "lightroom").is_err());
    // Codes are checked (3 `suspect` since migration 8).
    insert(b"s.jpg", 3, "lightroom").expect("suspect row");
    assert!(insert(b"a.jpg", 4, "lightroom").is_err());
    assert!(insert(b"b.jpg", 1, "flickr").is_err());
    // file_id has no foreign key: file 42 does not exist and the row stands.
    let n: i64 = conn
        .query_row("SELECT count(*) FROM export_ledger", [], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 2);
    let v: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("version");
    assert_eq!(v, latest_schema_version());
}

/// Migration 8 (ADR-0040) rebuilds the ledger for state 3 and keeps every existing row and its indexes.
#[test]
fn ledger_suspect_migration_keeps_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::LEDGER_SUSPECT_MIGRATION.version - 1)
        .expect("catalog");
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
            params![b"/lib".to_vec()],
        )
        .expect("library");
        conn.execute(
            "INSERT INTO export_ledger (library_id, content_id, identity_kind, full_hash, file_id,
                 library_rel_path, size, state, profile, at)
             VALUES (1, ?1, 0, ?1, 42, ?2, 10, 1, 'apple', 5)",
            params![vec![7u8; 32], b"x.jpg".to_vec()],
        )
        .expect("row");
        assert!(
            conn.execute("UPDATE export_ledger SET state = 3", [])
                .is_err(),
            "version 7 refuses state 3"
        );
    }
    ps_catalog::Catalog::open_file(&path, Default::default())
        .expect("upgrade")
        .close()
        .expect("close");
    let conn = Connection::open(&path).expect("open");
    let (state, at): (i64, i64) = conn
        .query_row("SELECT state, at FROM export_ledger", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("row kept");
    assert_eq!((state, at), (1, 5));
    conn.execute("UPDATE export_ledger SET state = 3", [])
        .expect("suspect allowed");
    let idx: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = 'export_ledger'
             AND name IN ('export_ledger_by_content', 'export_ledger_by_file')",
            [],
            |r| r.get(0),
        )
        .expect("indexes");
    assert_eq!(idx, 2);
}

/// Migration 9 (ADR-0044) adds the library copy's modification time: older rows keep their values with no
/// time, and new rows round-trip it through the writer and the reader.
#[test]
fn ledger_mtime_migration_keeps_rows_and_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::LEDGER_MTIME_MIGRATION.version - 1).expect("catalog");
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
            params![b"/lib".to_vec()],
        )
        .expect("library");
        conn.execute(
            "INSERT INTO export_ledger (library_id, content_id, identity_kind, full_hash, file_id,
                 library_rel_path, size, state, profile, at)
             VALUES (1, ?1, 0, ?1, 42, ?2, 10, 1, 'apple', 5)",
            params![vec![7u8; 32], b"old.jpg".to_vec()],
        )
        .expect("row");
    }
    let cat = ps_catalog::Catalog::open_file(&path, Default::default()).expect("upgrade");
    let rows = cat.read().ledger(1).expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].size, rows[0].library_mtime_ns), (10, None));
    let mut e = rows[0].clone();
    e.library_rel_path = ps_model::RelPath::from_bytes(b"new.jpg".to_vec()).expect("rel");
    e.library_mtime_ns = Some(1_600_000_000_123_456_789);
    cat.writer().ledger_record(1, e).expect("record");
    let rows = cat.read().ledger(1).expect("rows");
    assert_eq!(
        rows.iter().map(|r| r.library_mtime_ns).collect::<Vec<_>>(),
        vec![Some(1_600_000_000_123_456_789), None]
    );
    cat.close().expect("close");
}

/// Migration 10 (ADR-0045): every existing row is a file row; sidecar rows round-trip and the code is checked.
#[test]
fn ledger_sidecars_migration_marks_old_rows_as_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::LEDGER_SIDECARS_MIGRATION.version - 1)
        .expect("catalog");
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "INSERT INTO export_library (id, root_path, created_at) VALUES (1, ?1, 0)",
            params![b"/lib".to_vec()],
        )
        .expect("library");
        conn.execute(
            "INSERT INTO export_ledger (library_id, content_id, identity_kind, full_hash, file_id,
                 library_rel_path, size, state, profile, at)
             VALUES (1, ?1, 0, ?1, 42, ?2, 10, 1, 'apple', 5)",
            params![vec![7u8; 32], b"a.jpg".to_vec()],
        )
        .expect("row");
    }
    let cat = ps_catalog::Catalog::open_file(&path, Default::default()).expect("upgrade");
    let rows = cat.read().ledger(1).expect("rows");
    assert_eq!(rows[0].kind, ps_model::LedgerKind::File);
    let mut side = rows[0].clone();
    side.library_rel_path = ps_model::RelPath::from_bytes(b"a.xmp".to_vec()).expect("rel");
    side.kind = ps_model::LedgerKind::Sidecar;
    cat.writer().ledger_record(1, side).expect("record");
    let kinds: Vec<_> = cat
        .read()
        .ledger(1)
        .expect("rows")
        .into_iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![ps_model::LedgerKind::File, ps_model::LedgerKind::Sidecar]
    );
    cat.close().expect("close");
    let conn = Connection::open(&path).expect("open");
    assert!(
        conn.execute("UPDATE export_ledger SET kind = 2", [])
            .is_err(),
        "kind codes are checked"
    );
}

/// Migration 12: a library's last verification time, `None` until recorded.
#[test]
fn library_verified_at_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::LIBRARY_VERIFIED_MIGRATION.version - 1)
        .expect("catalog");
    let cat = ps_catalog::Catalog::open_file(&path, Default::default()).expect("upgrade");
    let id = cat
        .writer()
        .export_library(b"/lib".to_vec(), None)
        .expect("library");
    assert_eq!(
        cat.read().export_libraries().expect("libs")[0].verified_at_ms,
        None
    );
    cat.writer()
        .export_library_verified(id, 1234)
        .expect("verified");
    assert_eq!(
        cat.read().export_libraries().expect("libs")[0].verified_at_ms,
        Some(1234)
    );
    cat.close().expect("close");
}
