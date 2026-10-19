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
    // Codes are checked.
    assert!(insert(b"a.jpg", 3, "lightroom").is_err());
    assert!(insert(b"b.jpg", 1, "flickr").is_err());
    // file_id has no foreign key: file 42 does not exist and the row stands.
    let n: i64 = conn
        .query_row("SELECT count(*) FROM export_ledger", [], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 1);
    let v: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("version");
    assert_eq!(v, latest_schema_version());
}
