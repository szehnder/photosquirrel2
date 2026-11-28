//! Migration 27 (best-of-stacks task 2.2; ADR-0135): a version-26 catalog gains the shot-quality columns and the
//! stack decision tables, empty, and nothing else changes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_catalog::{Catalog, db};
use rusqlite::Connection;

fn columns(c: &Connection, table: &str) -> Vec<String> {
    let mut stmt = c.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn migration_27_adds_shot_quality_and_stack_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, 26).unwrap();
    let before = columns(&Connection::open(&path).unwrap(), "file");
    assert!(!before.contains(&"sharpness".to_owned()));

    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    assert_eq!(db::stored_version(&path).unwrap(), Some(27));
    let c = Connection::open(&path).unwrap();
    let after = columns(&c, "file");
    for col in [
        "shot_quality_version",
        "sharpness",
        "clipped_high",
        "clipped_low",
    ] {
        assert!(after.contains(&col.to_owned()), "{col}");
    }
    assert_eq!(
        after.len(),
        before.len() + 4,
        "only the four columns are added"
    );
    assert!(!columns(&c, "stack_separation").is_empty());
    assert!(!columns(&c, "stack_pick").is_empty());
    let d = cat.read().stack_decisions().unwrap();
    assert!(d.separations.is_empty() && d.picks.is_empty());
    assert!(cat.read().shot_qualities().unwrap().is_empty());
}
