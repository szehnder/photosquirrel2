//! Catalog migration 24 (preview-placeholders, ADR-0109): placeholder rows of older catalogs marked at generation
//! 0, and the waiting rule that says which placeholders can be filled in now.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ps_catalog::{Catalog, WaitingRule, db};
use ps_model::{
    DecodeFamily, DecoderKey, FillScope, Format, Generations, PreviewReason, PreviewStamp,
};
use rusqlite::{Connection, params};

/// (name, format, media_kind, stage, phash, identity_kind, decoder_missing, decode_failure)
type Row = (
    &'static str,
    Option<&'static str>,
    i64,
    i64,
    Option<i64>,
    i64,
    Option<&'static str>,
    Option<&'static str>,
);

fn catalog_at_23(rows: &[Row]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, db::PREVIEW_GEN_MIGRATION.version - 1).unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute(
        "INSERT INTO source (id, root_path, added_at) VALUES (1, CAST('/mnt/card' AS BLOB), 100)",
        [],
    )
    .unwrap();
    for (i, (name, format, kind, stage, phash, ik, dm, df)) in rows.iter().enumerate() {
        c.execute(
            "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                               media_kind, kflags, content_id, identity_kind, phash, dhash, hash_version,
                               decoder_missing, decode_failure)
             VALUES (?1, 1, ?2, ?2, 10, 1, ?3, 0, '[]', ?4, ?5, 0, ?6, ?7, ?8, ?8, 1, ?9, ?10)",
            params![
                i as i64 + 1,
                name.as_bytes(),
                stage,
                format,
                kind,
                [i as u8 + 1; 32].to_vec(),
                ik,
                phash,
                dm,
                df
            ],
        )
        .unwrap();
    }
    (dir, path)
}

#[test]
fn migration_24_marks_placeholder_stills_of_older_catalogs_at_generation_0() {
    let rows: [Row; 9] = [
        // A 0.2.0 TIFF placeholder: thumbed, no perceptual hashes.
        ("scan.tif", Some("tiff"), 0, 3, None, 0, None, None),
        // HEIC waiting for its decoder (migration 19's marker).
        (
            "IMG_1.HEIC",
            Some("heic"),
            0,
            3,
            None,
            0,
            Some("heif"),
            None,
        ),
        // A HEIC whose decode failed with the decoder present (migration 20).
        (
            "IMG_2.HEIC",
            Some("heic"),
            0,
            3,
            None,
            0,
            None,
            Some("heif|p|1|libheif 1.17.6"),
        ),
        // A RAW without an embedded preview, and a file too large to decode (sampled identity).
        ("IMG_3.CR2", Some("raw:cr2"), 0, 3, None, 0, None, None),
        ("huge.png", Some("png"), 0, 3, None, 1, None, None),
        // A real thumbnail, a video (no hashes either way), a file not thumbed yet, a probed file.
        ("IMG_4.JPG", Some("jpeg"), 0, 3, Some(7), 0, None, None),
        ("clip.mov", Some("mov"), 1, 3, None, 0, None, None),
        ("IMG_5.JPG", Some("jpeg"), 0, 2, None, 0, None, None),
        ("IMG_6.JPG", Some("jpeg"), 0, 1, None, 0, None, None),
    ];
    let (_dir, path) = catalog_at_23(&rows);
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    drop(cat);
    let c = Connection::open(&path).unwrap();
    let got: Vec<(String, Option<i64>, Option<String>)> = c
        .prepare("SELECT CAST(name AS TEXT), preview_gen, preview_reason FROM file ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let want: [(&str, Option<i64>, Option<&str>); 9] = [
        ("scan.tif", Some(0), Some("unreadable")),
        ("IMG_1.HEIC", Some(0), Some("decoder_missing")),
        ("IMG_2.HEIC", Some(0), Some("unreadable")),
        ("IMG_3.CR2", Some(0), Some("unsupported_variant")),
        ("huge.png", Some(0), Some("unsupported_variant")),
        ("IMG_4.JPG", None, None),
        ("clip.mov", None, None),
        ("IMG_5.JPG", None, None),
        ("IMG_6.JPG", None, None),
    ];
    assert_eq!(
        got,
        want.iter()
            .map(|(n, g, r)| (n.to_string(), *g, r.map(str::to_owned)))
            .collect::<Vec<_>>()
    );
    let v: u32 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::CATALOG_MIGRATIONS.last().unwrap().version);
    // The thumbed video without hashes is left to migration 25's one-time task (ADR-0111), done at open by the
    // engine, which can read thumbs.db.
    let tasks: Vec<String> = c
        .prepare("SELECT name FROM catalog_task")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tasks, ["video_placeholder_reasons"]);
    let kinds: Vec<String> = c
        .prepare("SELECT kind FROM scan_run")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(kinds.is_empty());
}

fn generations() -> Generations {
    Generations::from_pairs(&[
        (DecodeFamily::Tiff, 1),
        (DecodeFamily::Heif, 1),
        (DecodeFamily::Avif, 1),
    ])
}

/// Every row of the waiting rule, through the SQL predicate and the Rust one.
#[test]
fn the_waiting_rule_row_by_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    drop(cat);
    let c = Connection::open(&path).unwrap();
    c.execute(
        "INSERT INTO source (id, root_path, added_at) VALUES (1, CAST('/mnt/card' AS BLOB), 100)",
        [],
    )
    .unwrap();
    // (name, format, kind, preview_gen, preview_reason, decoder_missing, decode_failure, missing, fillable?)
    #[allow(clippy::type_complexity)]
    let rows: [(
        &str,
        &str,
        i64,
        Option<i64>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        bool,
        bool,
    ); 12] = [
        // An old TIFF placeholder: generation 0 below TIFF 1.
        (
            "old.tif",
            "tiff",
            0,
            Some(0),
            Some("unreadable"),
            None,
            None,
            false,
            true,
        ),
        // A current-generation BigTIFF and a current-generation damaged TIFF wait for a change.
        (
            "big.tif",
            "tiff",
            0,
            Some(1),
            Some("unsupported_variant"),
            None,
            None,
            false,
            false,
        ),
        (
            "bad.tif",
            "tiff",
            0,
            Some(1),
            Some("unreadable"),
            None,
            None,
            false,
            false,
        ),
        // A damaged JPEG at JPEG generation 0 (current).
        (
            "bad.jpg",
            "jpeg",
            0,
            Some(0),
            Some("unreadable"),
            None,
            None,
            false,
            false,
        ),
        // HEIC waiting for the decoder this machine has; AVIF waiting for one it lacks.
        (
            "a.heic",
            "heic",
            0,
            Some(1),
            Some("decoder_missing"),
            Some("heif"),
            None,
            false,
            true,
        ),
        (
            "b.avif",
            "avif",
            0,
            Some(0),
            Some("decoder_missing"),
            Some("avif"),
            None,
            false,
            false,
        ),
        // A decode failure follows ADR-0098's retry rule, never the generation rule.
        (
            "c.heic",
            "heic",
            0,
            Some(0),
            Some("unreadable"),
            None,
            Some("heif|p|1|x"),
            false,
            false,
        ),
        // An old placeholder of a file that went missing.
        (
            "gone.tif",
            "tiff",
            0,
            Some(0),
            Some("unreadable"),
            None,
            None,
            true,
            false,
        ),
        // A video without a poster tool; the poster generation is 0 anyway.
        (
            "clip.mov",
            "mov",
            1,
            Some(0),
            Some("no_poster_tool"),
            None,
            None,
            false,
            false,
        ),
        // A video whose poster frame failed with a tool present (ADR-0114): waits for a change or an explicit retry.
        (
            "failed.mov",
            "mov",
            1,
            Some(0),
            Some("poster_failed"),
            None,
            None,
            false,
            false,
        ),
        // A real thumbnail.
        ("ok.tif", "tiff", 0, None, None, None, None, false, false),
        // A generation without a reason (never written; the row reads as having a preview): not fillable, and
        // the SQL rule is never NULL for it.
        (
            "odd.tif",
            "tiff",
            0,
            Some(0),
            None,
            None,
            None,
            false,
            false,
        ),
    ];
    for (i, (name, format, kind, g, r, dm, df, missing, _)) in rows.iter().enumerate() {
        c.execute(
            "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                               media_kind, kflags, content_id, identity_kind, preview_gen, preview_reason,
                               decoder_missing, decode_failure, missing_since)
             VALUES (?1, 1, ?2, ?2, 10, 1, 3, 0, '[]', ?3, ?4, 0, ?5, 0, ?6, ?7, ?8, ?9, ?10)",
            params![
                i as i64 + 1,
                name.as_bytes(),
                format,
                kind,
                [i as u8 + 1; 32].to_vec(),
                g,
                r,
                dm,
                df,
                missing.then_some(5i64)
            ],
        )
        .unwrap();
    }
    drop(c);
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    let rule = WaitingRule {
        decodable: vec![DecoderKey::Heif],
        generations: generations(),
        poster_tool: false,
    };
    let fillable: Vec<String> = cat
        .read()
        .fillable_files(&rule, &FillScope::all(), &[], false)
        .unwrap()
        .into_iter()
        .map(|r| String::from_utf8_lossy(r.key.path.as_bytes()).into_owned())
        .collect();
    let want: Vec<&str> = rows.iter().filter(|r| r.8).map(|r| r.0).collect();
    let mut sorted = want.clone();
    sorted.sort();
    assert_eq!(fillable, sorted, "the SQL rule");
    // The Rust rule agrees on every row.
    for (name, ..) in &rows {
        let rec = cat
            .read()
            .file(&ps_catalog::FileKey::new(
                ps_model::SourceId(1),
                ps_model::RelPath::from_bytes(name.as_bytes().to_vec()).unwrap(),
            ))
            .unwrap()
            .unwrap();
        assert_eq!(
            rule.fillable(&rec),
            want.contains(name),
            "{name}: {:?}",
            rec.preview
        );
    }
    // Current-generation unreadable rows are not fillable, and stay so with the TIFF generation unchanged.
    let tiff = cat
        .read()
        .fillable_files(&rule, &FillScope::format(Format::Tiff), &[], false)
        .unwrap();
    assert_eq!(tiff.len(), 1, "only the old TIFF placeholder");
    // A bump of the TIFF generation makes every TIFF placeholder fillable once.
    let bumped = WaitingRule {
        generations: generations().with(DecodeFamily::Tiff, 2),
        ..rule.clone()
    };
    let tiff = cat
        .read()
        .fillable_files(&bumped, &FillScope::format(Format::Tiff), &[], false)
        .unwrap();
    assert_eq!(tiff.len(), 3, "old, BigTIFF and damaged TIFF");
    // The counts by format and reason.
    let status = cat.read().preview_status(&rule, &[]).unwrap();
    let row = |f: &str, r: PreviewReason| {
        status
            .iter()
            .find(|s| s.format == f && s.reason == r)
            .map(|s| (s.count, s.fillable))
    };
    assert_eq!(row("tiff", PreviewReason::Unreadable), Some((2, 1)));
    assert_eq!(row("tiff", PreviewReason::UnsupportedVariant), Some((1, 0)));
    assert_eq!(row("heic", PreviewReason::DecoderMissing), Some((1, 1)));
    assert_eq!(row("avif", PreviewReason::DecoderMissing), Some((1, 0)));
    assert_eq!(row("jpeg", PreviewReason::Unreadable), Some((1, 0)));
    // The record carries the stamp.
    let rec = cat
        .read()
        .file(&ps_catalog::FileKey::new(
            ps_model::SourceId(1),
            ps_model::RelPath::from_bytes(b"big.tif".to_vec()).unwrap(),
        ))
        .unwrap()
        .unwrap();
    assert_eq!(
        rec.preview,
        Some(PreviewStamp {
            generation: 1,
            reason: PreviewReason::UnsupportedVariant
        })
    );
    // The tool clause (ADR-0114): with a poster-frame tool available, the video waiting for one is fillable whatever
    // its generation (the poster generation is 0); the failed poster is not, until the explicit retry.
    let tool = WaitingRule {
        poster_tool: true,
        ..rule.clone()
    };
    let names = |rule: &WaitingRule, retry: bool| -> Vec<String> {
        cat.read()
            .fillable_files(rule, &FillScope::format(Format::Mov), &[], retry)
            .unwrap()
            .into_iter()
            .map(|r| String::from_utf8_lossy(r.key.path.as_bytes()).into_owned())
            .collect()
    };
    assert_eq!(names(&tool, false), ["clip.mov"]);
    assert_eq!(names(&tool, true), ["clip.mov", "failed.mov"]);
    // Without a tool nothing waits for one, retry or not.
    assert!(names(&rule, false).is_empty());
    assert!(names(&rule, true).is_empty());
    // A bump of the poster generation retries the failed poster, but only with a tool.
    let bumped_tool = WaitingRule {
        generations: generations().with(DecodeFamily::VideoPoster, 1),
        ..tool.clone()
    };
    assert_eq!(names(&bumped_tool, false), ["clip.mov", "failed.mov"]);
    let bumped_no_tool = WaitingRule {
        poster_tool: false,
        ..bumped_tool.clone()
    };
    assert!(names(&bumped_no_tool, false).is_empty());
    // The Rust rule agrees with the SQL rule on every row, for every one of these rules.
    for r in [&tool, &bumped_tool, &bumped_no_tool] {
        let all: Vec<String> = cat
            .read()
            .fillable_files(r, &FillScope::all(), &[], false)
            .unwrap()
            .into_iter()
            .map(|r| String::from_utf8_lossy(r.key.path.as_bytes()).into_owned())
            .collect();
        let retry: Vec<String> = cat
            .read()
            .fillable_files(r, &FillScope::all(), &[], true)
            .unwrap()
            .into_iter()
            .map(|r| String::from_utf8_lossy(r.key.path.as_bytes()).into_owned())
            .collect();
        for (name, ..) in &rows {
            let rec = cat
                .read()
                .file(&ps_catalog::FileKey::new(
                    ps_model::SourceId(1),
                    ps_model::RelPath::from_bytes(name.as_bytes().to_vec()).unwrap(),
                ))
                .unwrap()
                .unwrap();
            assert_eq!(
                r.fillable(&rec),
                all.iter().any(|n| n == name),
                "{name} {r:?}"
            );
            assert_eq!(
                r.fillable_retrying_failed_posters(&rec),
                retry.iter().any(|n| n == name),
                "{name} {r:?} (retry)"
            );
        }
    }
    // The counts: the waiting video counts as fillable with a tool, the failed one never does.
    let status = cat.read().preview_status(&tool, &[]).unwrap();
    let row = |r: PreviewReason| {
        status
            .iter()
            .find(|s| s.format == "mov" && s.reason == r)
            .map(|s| (s.count, s.fillable))
    };
    assert_eq!(row(PreviewReason::NoPosterTool), Some((1, 1)));
    assert_eq!(row(PreviewReason::PosterFailed), Some((1, 0)));
}

/// Migration 25 records its task only when the catalog has a thumbed video without a placeholder reason.
#[test]
fn migration_25_records_the_video_task_only_when_needed() {
    let rows: [Row; 1] = [("IMG_1.JPG", Some("jpeg"), 0, 3, Some(7), 0, None, None)];
    let (_dir, path) = catalog_at_23(&rows);
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    assert!(
        !cat.read()
            .catalog_task_pending(ps_catalog::previews::VIDEO_PLACEHOLDER_TASK)
            .unwrap()
    );
    assert!(cat.read().unmarked_video_rows().unwrap().is_empty());
}

/// `damaged_count` (damaged-raw-reason, ADR-0115): present files catalogued as could not be read, whatever their
/// format, per source; one aggregate answered from the partial index `file_preview`.
#[test]
fn damaged_count_reads_the_partial_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    drop(Catalog::open_file(&path, Default::default()).unwrap());
    let c = Connection::open(&path).unwrap();
    for s in [1, 2] {
        c.execute(
            "INSERT INTO source (id, root_path, added_at) VALUES (?1, CAST(?2 AS BLOB), 100)",
            params![s, format!("/mnt/card{s}")],
        )
        .unwrap();
    }
    // (source, name, format, reason, missing)
    let rows: [(i64, &str, &str, Option<&str>, bool); 6] = [
        (1, "IMG_1.CR2", "raw:cr2", Some("unreadable"), false),
        (1, "IMG_2.JPG", "jpeg", Some("unreadable"), false),
        (
            1,
            "IMG_3.CR2",
            "raw:cr2",
            Some("unsupported_variant"),
            false,
        ),
        (1, "IMG_4.CR2", "raw:cr2", Some("unreadable"), true),
        (1, "IMG_5.JPG", "jpeg", None, false),
        (2, "IMG_6.CR2", "raw:cr2", Some("unreadable"), false),
    ];
    for (i, (src, name, format, reason, missing)) in rows.iter().enumerate() {
        c.execute(
            "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                               media_kind, kflags, content_id, identity_kind, preview_gen, preview_reason,
                               missing_since)
             VALUES (?1, ?2, ?3, ?3, 10, 1, 3, 0, '[]', ?4, 0, 0, ?5, 0, ?6, ?7, ?8)",
            params![
                i as i64 + 1,
                src,
                name.as_bytes(),
                format,
                [i as u8 + 1; 32].to_vec(),
                reason.map(|_| 1i64),
                reason,
                missing.then_some(5i64)
            ],
        )
        .unwrap();
    }
    let plan: Vec<String> = c
        .prepare(&format!(
            "EXPLAIN QUERY PLAN SELECT count(*) FROM file WHERE {} AND source_id IN (1)",
            ps_catalog::previews::DAMAGED_WHERE
        ))
        .unwrap()
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(plan.iter().any(|l| l.contains("file_preview")), "{plan:?}");
    drop(c);
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    let r = cat.read();
    assert_eq!(
        r.damaged_count(&[ps_model::SourceId(1)]).unwrap(),
        2,
        "present only"
    );
    assert_eq!(r.damaged_count(&[ps_model::SourceId(2)]).unwrap(), 1);
    assert_eq!(r.damaged_count(&[]).unwrap(), 3, "every source");
}

/// A poster item's outcome (ADR-0114) reaches every thumbed copy of the content still waiting for a poster, and no
/// other row.
#[test]
fn a_poster_outcome_reaches_the_waiting_copies_of_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    drop(Catalog::open_file(&path, Default::default()).unwrap());
    let c = Connection::open(&path).unwrap();
    c.execute(
        "INSERT INTO source (id, root_path, added_at) VALUES (1, CAST('/mnt/card' AS BLOB), 100)",
        [],
    )
    .unwrap();
    // (name, content, preview_gen, preview_reason)
    let rows: [(&str, u8, Option<i64>, Option<&str>); 4] = [
        ("a.mov", 1, Some(0), Some("no_poster_tool")),
        ("copy/a.mov", 1, Some(0), Some("poster_failed")),
        ("b.mov", 2, Some(0), Some("no_poster_tool")),
        ("c.jpg", 1, Some(0), Some("unreadable")),
    ];
    for (i, (name, content, g, r)) in rows.iter().enumerate() {
        c.execute(
            "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, reasons, format,
                               media_kind, kflags, content_id, identity_kind, preview_gen, preview_reason)
             VALUES (?1, 1, ?2, ?2, 10, 1, 3, 0, '[]', 'mov', 1, 0, ?3, 0, ?4, ?5)",
            params![
                i as i64 + 1,
                name.as_bytes(),
                [*content; 32].to_vec(),
                g,
                r
            ],
        )
        .unwrap();
    }
    drop(c);
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    let key = |n: &str| {
        ps_catalog::FileKey::new(
            ps_model::SourceId(1),
            ps_model::RelPath::from_bytes(n.as_bytes().to_vec()).unwrap(),
        )
    };
    let preview = |n: &str| cat.read().file(&key(n)).unwrap().unwrap().preview;
    let failed = PreviewStamp {
        generation: 0,
        reason: PreviewReason::PosterFailed,
    };
    cat.writer()
        .submit(ps_catalog::Command::SetPosterPreview(vec![(
            key("a.mov"),
            Some(failed),
        )]))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(preview("a.mov"), Some(failed));
    assert_eq!(preview("copy/a.mov"), Some(failed));
    assert_eq!(
        preview("b.mov").map(|p| p.reason),
        Some(PreviewReason::NoPosterTool),
        "another content"
    );
    assert_eq!(
        preview("c.jpg").map(|p| p.reason),
        Some(PreviewReason::Unreadable),
        "not a poster placeholder"
    );
    cat.writer()
        .submit(ps_catalog::Command::SetPosterPreview(vec![(
            key("copy/a.mov"),
            None,
        )]))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(preview("a.mov"), None);
    assert_eq!(preview("copy/a.mov"), None);
    assert!(preview("b.mov").is_some());
    assert!(preview("c.jpg").is_some());
}
