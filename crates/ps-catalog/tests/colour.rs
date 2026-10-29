//! Catalog migration 26 (adobe-rgb-color, ADR-0113): `file.colour`; only thumbed `_`-named JPEG and RAW stills with a
//! real thumbnail are left for the one-time re-check, every other analysed row is marked 'unchecked'.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use ps_catalog::{AnalysisResult, Catalog, Command, FileKey, FileRow, Results, db};
use ps_model::{COLOUR_UNCHECKED, ColourSpace};
use rusqlite::{Connection, params};

#[test]
fn migration_26_leaves_only_underscore_jpeg_and_raw_rows_for_the_re_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::COLOUR_MIGRATION.version - 1).unwrap();
    // (name, format, stage, analysed, placeholder reason)
    let rows: [(&str, Option<&str>, i64, bool, Option<&str>); 9] = [
        ("_MG_0001.JPG", Some("jpeg"), 3, true, None),
        ("_MG_0002.CR2", Some("raw:cr2"), 3, true, None),
        ("IMG_0003.JPG", Some("jpeg"), 3, true, None),
        // A placeholder has no thumbnail to recolour.
        (
            "_MG_0004.CR2",
            Some("raw:cr2"),
            3,
            true,
            Some("unsupported_variant"),
        ),
        // Other formats are not re-checked.
        ("_MG_0005.HEIC", Some("heic"), 3, true, None),
        ("_MG_0006.TIF", Some("tiff"), 3, true, None),
        // Not analysed yet: its analysis records the colour.
        ("_MG_0007.JPG", None, 0, false, None),
        // Analysed, not thumbed (a crash between the two): not re-checked.
        ("_MG_0008.JPG", Some("jpeg"), 2, true, None),
        ("clip.mov", Some("mov"), 3, true, None),
    ];
    {
        let c = Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (1, CAST('/mnt/card' AS BLOB), 100)",
            [],
        )
        .unwrap();
        for (i, (name, format, stage, analysed, reason)) in rows.iter().enumerate() {
            let id = vec![i as u8 + 1; 32];
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                                   kflags, metadata, content_id, identity_kind, preview_gen, preview_reason)
                 VALUES (?1, 1, ?2, ?2, 10, 1, ?3, 0, '[]', ?4, 0, '{}', ?5, ?6, ?7, ?8)",
                params![
                    i as i64 + 1,
                    name.as_bytes(),
                    stage,
                    format,
                    analysed.then_some(id),
                    analysed.then_some(0),
                    reason.map(|_| 0),
                    reason
                ],
            )
            .unwrap();
        }
    }
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    let got: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|(name, ..)| {
            let r = cat
                .read()
                .file(&FileKey::new(ps_model::SourceId(1), rel(name)))
                .unwrap()
                .unwrap();
            (name.to_string(), r.colour)
        })
        .collect();
    let un = Some(COLOUR_UNCHECKED.to_owned());
    let want = vec![
        ("_MG_0001.JPG", None),
        ("_MG_0002.CR2", None),
        ("IMG_0003.JPG", un.clone()),
        ("_MG_0004.CR2", un.clone()),
        ("_MG_0005.HEIC", un.clone()),
        ("_MG_0006.TIF", un.clone()),
        ("_MG_0007.JPG", None),
        ("_MG_0008.JPG", un.clone()),
        ("clip.mov", un),
    ];
    assert_eq!(
        got,
        want.into_iter()
            .map(|(n, c)| (n.to_owned(), c))
            .collect::<Vec<_>>()
    );
    // The reset SQL tests use to rebuild an old catalog leaves the same rows NULL.
    let c = Connection::open(&path).unwrap();
    c.execute_batch(&db::colour_reset_sql()).unwrap();
    let nulls: Vec<String> = c
        .prepare("SELECT CAST(name AS TEXT) FROM file WHERE colour IS NULL ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(nulls, ["_MG_0001.JPG", "_MG_0002.CR2", "_MG_0007.JPG"]);
}

fn write_analyzed(cat: &Catalog, k: &FileKey, i: u64, a: AnalysisResult) {
    let mut probe = probe_of(i);
    probe.skip = false;
    cat.writer()
        .upsert_files(vec![FileRow {
            key: k.clone(),
            stat: stat_of(i),
            results: Results::Analyzed(probe, Box::new(a)),
        }])
        .unwrap();
    cat.writer().flush().unwrap();
}

#[test]
fn analysis_writes_the_colour_and_the_re_check_stamps_every_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    let mut a = analysis_of(1);
    a.colour = Some(ColourSpace::AdobeRgb);
    let keys: Vec<FileKey> = ["_MG_0001.JPG", "copy/_MG_0001.JPG"]
        .iter()
        .map(|n| FileKey::new(src, rel(n)))
        .collect();
    for k in &keys {
        write_analyzed(&cat, k, 1, a.clone());
    }
    let colour = |k: &FileKey| cat.read().file(k).unwrap().unwrap().colour;
    assert_eq!(colour(&keys[0]).as_deref(), Some("adobe_rgb"));
    // The re-check writes every row of the content, and only those.
    let other = FileKey::new(src, rel("IMG_0002.JPG"));
    write_analyzed(&cat, &other, 2, analysis_of(2));
    cat.writer()
        .submit(Command::SetColour(vec![(
            keys[0].clone(),
            Some(ColourSpace::DisplayP3),
        )]))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(colour(&keys[0]).as_deref(), Some("display_p3"));
    assert_eq!(colour(&keys[1]).as_deref(), Some("display_p3"));
    assert_eq!(colour(&other).as_deref(), Some("srgb"));
    cat.writer()
        .submit(Command::SetColour(vec![(keys[1].clone(), None)]))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(colour(&keys[0]).as_deref(), Some(COLOUR_UNCHECKED));
}
