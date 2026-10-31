//! Catalog migration 26 (adobe-rgb-color, ADR-0113): `file.colour`; only thumbed `_`-named JPEG and RAW stills with a
//! real thumbnail are left for the one-time re-check, every other analysed row is marked 'unchecked'.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use ps_catalog::{AnalysisResult, Catalog, ColourStamp, Command, FileKey, FileRow, Results, db};
use ps_model::{COLOUR_UNCHECKED, ColourSpace, StoredColour};
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
    let stamp = |k: &FileKey, colour, copies| {
        cat.writer()
            .submit(Command::SetColour(vec![ColourStamp {
                key: k.clone(),
                colour,
                copies,
            }]))
            .unwrap();
        cat.writer().flush().unwrap();
    };
    stamp(&keys[0], StoredColour::Space(ColourSpace::DisplayP3), true);
    assert_eq!(colour(&keys[0]).as_deref(), Some("display_p3"));
    assert_eq!(colour(&keys[1]).as_deref(), Some("display_p3"));
    assert_eq!(colour(&other).as_deref(), Some("srgb"));
    // A converted space reaches every copy, whatever it recorded.
    stamp(&keys[0], StoredColour::Space(ColourSpace::AdobeRgb), true);
    assert_eq!(colour(&keys[1]).as_deref(), Some("adobe_rgb"));
}

/// A re-check that kept a thumbnail (`unconverted:`) or found sRGB stamps only the copies that know nothing yet: a
/// copy whose converted thumbnail stands keeps its colour.
#[test]
fn a_kept_or_srgb_stamp_never_marks_down_a_converted_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    let mut a = analysis_of(1);
    a.colour = Some(ColourSpace::AdobeRgb);
    let keys: Vec<FileKey> = [
        "_MG_0001.JPG",
        "a/_MG_0001.JPG",
        "b/_MG_0001.JPG",
        "c/_MG_0001.JPG",
    ]
    .iter()
    .map(|n| FileKey::new(src, rel(n)))
    .collect();
    for (i, k) in keys.iter().enumerate() {
        let mut a = a.clone();
        if i == 2 {
            // Analysed before colour spaces were read.
            a.colour = None;
        }
        write_analyzed(&cat, k, 1, a);
    }
    let stamp = |k: &FileKey, colour| {
        cat.writer()
            .submit(Command::SetColour(vec![ColourStamp {
                key: k.clone(),
                colour,
                copies: false,
            }]))
            .unwrap();
        cat.writer().flush().unwrap();
    };
    // keys[0] converted (adobe_rgb, from its analysis), keys[1] unchecked, keys[2] not checked yet, keys[3] sRGB.
    stamp(&keys[1], StoredColour::Unchecked);
    let colour = |k: &FileKey| cat.read().file(k).unwrap().unwrap().colour;
    stamp(&keys[3], StoredColour::Space(ColourSpace::Srgb));
    assert_eq!(colour(&keys[2]), None);
    let copies = |k: &FileKey, colour| {
        cat.writer()
            .submit(Command::SetColour(vec![ColourStamp {
                key: k.clone(),
                colour,
                copies: true,
            }]))
            .unwrap();
        cat.writer().flush().unwrap();
    };
    copies(&keys[3], StoredColour::Unconverted(ColourSpace::AdobeRgb));
    assert_eq!(colour(&keys[3]).as_deref(), Some("unconverted:adobe_rgb"));
    assert_eq!(
        colour(&keys[0]).as_deref(),
        Some("adobe_rgb"),
        "not marked down"
    );
    assert_eq!(colour(&keys[1]).as_deref(), Some("unconverted:adobe_rgb"));
    assert_eq!(colour(&keys[2]).as_deref(), Some("unconverted:adobe_rgb"));
    copies(&keys[3], StoredColour::Space(ColourSpace::Srgb));
    assert_eq!(colour(&keys[3]).as_deref(), Some("srgb"));
    assert_eq!(
        colour(&keys[0]).as_deref(),
        Some("adobe_rgb"),
        "not marked down"
    );
    assert_eq!(colour(&keys[1]).as_deref(), Some("unconverted:adobe_rgb"));
}

/// A file whose bytes are not its row's content (an identity mismatch under an unchanged fingerprint) is marked
/// unchecked alone: the other copies keep the colour their own content recorded.
#[test]
fn an_unchecked_stamp_without_copies_writes_only_its_row() {
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
    cat.writer()
        .submit(Command::SetColour(vec![ColourStamp {
            key: keys[1].clone(),
            colour: StoredColour::Unchecked,
            copies: false,
        }]))
        .unwrap();
    cat.writer().flush().unwrap();
    let colour = |k: &FileKey| cat.read().file(k).unwrap().unwrap().colour;
    assert_eq!(colour(&keys[1]).as_deref(), Some(COLOUR_UNCHECKED));
    assert_eq!(colour(&keys[0]).as_deref(), Some("adobe_rgb"), "untouched");
}
