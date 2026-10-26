//! Catalog migration 23 (burst-frame-identity, ADR-0105): `file.frame_id` and `file.meta_version`, every non-RAW
//! row marked current without reading it.

use ps_catalog::{Catalog, db};
use rusqlite::{Connection, params};

#[test]
fn migration_23_marks_non_raw_rows_current_and_keeps_raw_rows_for_a_re_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::FRAME_IDENTITY_MIGRATION.version - 1).unwrap();
    let rows: [(&str, Option<&str>); 5] = [
        ("IMG_0034.CR2", Some("raw:cr2")),
        ("IMG_0034.DNG", Some("raw:dng")),
        ("IMG_0034.JPG", Some("jpeg")),
        ("clip.mov", Some("mov")),
        ("notes.txt", None),
    ];
    {
        let c = Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, CAST('/mnt/card' AS BLOB), 100)",
            [],
        )
        .unwrap();
        for (i, (name, format)) in rows.iter().enumerate() {
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                                   kflags, metadata)
                 VALUES (?1, 1, ?2, ?2, 10, 1, 3, 0, '[]', ?3, 0, '{}')",
                params![i as i64 + 1, name.as_bytes(), format],
            )
            .unwrap();
        }
    }
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    drop(cat);
    let c = Connection::open(&path).unwrap();
    let got: Vec<(String, Option<String>, i64)> = c
        .prepare("SELECT CAST(name AS TEXT), frame_id, meta_version FROM file ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        got,
        vec![
            ("IMG_0034.CR2".to_owned(), None, 0),
            ("IMG_0034.DNG".to_owned(), None, 0),
            ("IMG_0034.JPG".to_owned(), None, 1),
            ("clip.mov".to_owned(), None, 1),
            ("notes.txt".to_owned(), None, 1),
        ]
    );
    let v: u32 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 23);
}
