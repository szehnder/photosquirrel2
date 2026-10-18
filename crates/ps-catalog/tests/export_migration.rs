//! Migration 6 (export ledger) applies on top of the current schema. It is not registered in
//! `CATALOG_MIGRATIONS` until migrations 4 and 5 land (see `db::EXPORT_MIGRATION`), so this test applies it the
//! way `migrate` will: in one transaction with the `user_version` bump.

use ps_catalog::{db, latest_schema_version};
use rusqlite::{Connection, params};

#[test]
fn export_migration_applies_on_latest_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    let latest = latest_schema_version();
    assert!(latest < db::EXPORT_MIGRATION.version);
    db::create_catalog_at_version(&path, latest).expect("catalog");

    let mut conn = Connection::open(&path).expect("open");
    conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
    let tx = conn.transaction().expect("tx");
    tx.execute_batch(db::EXPORT_MIGRATION.sql)
        .expect("migration 6 applies");
    tx.pragma_update(None, "user_version", db::EXPORT_MIGRATION.version)
        .expect("version");
    tx.commit().expect("commit");

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
    assert_eq!(v, 6);
}
