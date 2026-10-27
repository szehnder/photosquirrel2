//! Catalog migration 23 (burst-frame-identity, ADR-0105): `file.frame_id` and `file.meta_version`; every row whose
//! frame cannot matter marked current without reading it.

use ps_catalog::{Catalog, db};
use rusqlite::{Connection, params};

#[test]
fn migration_23_keeps_raw_rows_and_their_same_second_canon_files_for_a_re_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::FRAME_IDENTITY_MIGRATION.version - 1).unwrap();
    // (name, format, camera model, capture ms): the burst second is 1_587_348_023 s.
    let burst = 1_587_348_023_000i64;
    let rows: [(&str, Option<&str>, Option<&str>, Option<i64>); 9] = [
        (
            "IMG_0034.CR2",
            Some("raw:cr2"),
            Some("Canon EOS 70D"),
            Some(burst),
        ),
        ("IMG_0034.DNG", Some("raw:dng"), Some("Canon EOS 70D"), None),
        // Same second as the CR2, a Canon camera file: its file number matters.
        (
            "IMG_0035.JPG",
            Some("jpeg"),
            Some("Canon EOS 70D"),
            Some(burst + 130),
        ),
        // Another second, or another maker: marked current.
        (
            "IMG_0099.JPG",
            Some("jpeg"),
            Some("Canon EOS 70D"),
            Some(burst + 5_000),
        ),
        (
            "DSC_0001.JPG",
            Some("jpeg"),
            Some("NIKON D750"),
            Some(burst),
        ),
        // No camera model in the RAW's second (a scan or a stripped copy): marked current, not re-read.
        ("scan_0001.jpg", Some("jpeg"), None, Some(burst + 200)),
        ("IMG_1234.HEIC", Some("heic"), Some("iPhone 12"), None),
        ("clip.mov", Some("mov"), None, None),
        ("notes.txt", None, None, None),
    ];
    {
        let c = Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, CAST('/mnt/card' AS BLOB), 100)",
            [],
        )
        .unwrap();
        for (i, (name, format, model, ts)) in rows.iter().enumerate() {
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                                   kflags, metadata, camera_model, capture_ts)
                 VALUES (?1, 1, ?2, ?2, 10, 1, 3, 0, '[]', ?3, 0, '{}', ?4, ?5)",
                params![i as i64 + 1, name.as_bytes(), format, model, ts],
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
    let want = [
        ("IMG_0034.CR2", 0),
        ("IMG_0034.DNG", 0),
        ("IMG_0035.JPG", 0),
        ("IMG_0099.JPG", 1),
        ("DSC_0001.JPG", 1),
        ("scan_0001.jpg", 1),
        ("IMG_1234.HEIC", 1),
        ("clip.mov", 1),
        ("notes.txt", 1),
    ];
    assert_eq!(
        got,
        want.iter()
            .map(|(n, v)| (n.to_string(), None, *v))
            .collect::<Vec<_>>()
    );
    let v: u32 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 23);
}
