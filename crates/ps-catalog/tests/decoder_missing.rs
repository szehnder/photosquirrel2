//! Migration 19 and the re-decode commands (ADR-0091): files analysed without a decoder remember which one was
//! missing; a re-decode writes hashes and clears the marker, touching nothing else.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use ps_catalog::{Catalog, Command, FileKey, FileRow, Results, db};
use ps_model::{DecoderKey, FileId, PerceptualHash, RelPath, SourceId};
use rusqlite::{Connection, params};

/// (id, format, stage, phash, identity_kind)
const ROWS: &[(i64, &str, i64, Option<i64>, i64)] = &[
    (1, "heic", 3, None, 0),     // undecoded HEIC: marked heif
    (2, "avif", 3, None, 0),     // undecoded AVIF: marked avif
    (3, "heic", 3, Some(42), 0), // a HEIC with hashes: not marked
    (4, "heic", 3, None, 1),     // too large to decode (sampled identity): not marked
    (5, "jpeg", 3, None, 0),     // a JPEG without hashes: not marked
    (6, "heic", 2, None, 0),     // analysed, not thumbed yet: not marked
];

#[test]
fn migration_19_marks_undecoded_heic_and_avif_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, 18).unwrap();
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("INSERT INTO source (id, root_path, added_at) VALUES (1, x'2f61', 100);")
            .unwrap();
        for (id, format, stage, phash, kind) in ROWS {
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, class, format,
                                   media_kind, content_id, identity_kind, phash, dhash, hash_version, kflags)
                 VALUES (?1, 1, ?2, ?2, 1000, 5, ?3, 0, ?4, 0, ?5, ?6, ?7, ?7, 1, 0)",
                params![
                    id,
                    format!("IMG_{id}.{format}").into_bytes(),
                    stage,
                    format,
                    vec![*id as u8; 32],
                    kind,
                    phash
                ],
            )
            .unwrap();
        }
    }
    let cat = Catalog::open(dir.path()).unwrap();
    let r = cat.read();
    let marks: Vec<Option<DecoderKey>> = ROWS
        .iter()
        .map(|(id, ..)| r.file_by_id(FileId(*id)).unwrap().unwrap().decoder_missing)
        .collect();
    assert_eq!(
        marks,
        vec![
            Some(DecoderKey::Heif),
            Some(DecoderKey::Avif),
            None,
            None,
            None,
            None
        ]
    );
    let counts = r.undecoded_counts(&[]).unwrap();
    assert_eq!(
        counts,
        vec![
            (SourceId(1), "avif".into(), DecoderKey::Avif, 1),
            (SourceId(1), "heic".into(), DecoderKey::Heif, 1)
        ]
    );
    assert!(r.undecoded_counts(&[SourceId(2)]).unwrap().is_empty());
}

/// The aggregate is answered from the partial index.
#[test]
fn undecoded_counts_use_the_partial_index() {
    let dir = tempfile::tempdir().unwrap();
    let _cat = Catalog::open(dir.path()).unwrap();
    let c = Connection::open(dir.path().join("catalog.db")).unwrap();
    let plan: Vec<String> = c
        .prepare(
            "EXPLAIN QUERY PLAN SELECT source_id, coalesce(format, ''), decoder_missing, count(*) FROM file
             WHERE decoder_missing IS NOT NULL AND missing_since IS NULL AND (0 OR source_id IN (1))
             GROUP BY 1, 2, 3",
        )
        .unwrap()
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter().any(|l| l.contains("file_decoder_missing")),
        "{plan:?}"
    );
}

fn key(src: SourceId, i: u64) -> FileKey {
    FileKey::new(
        src,
        RelPath::from_bytes(format!("d/IMG_{i}.HEIC").into_bytes()).unwrap(),
    )
}

/// A re-decode of 1,000 waiting rows commits once and leaves stage, class, metadata and dates untouched.
#[test]
fn redecode_writes_hashes_and_clears_the_marker_only() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    let n = 1_000u64;
    let rows: Vec<FileRow> = (0..n)
        .map(|i| {
            let mut a = analysis_of(i);
            a.hashes = None;
            a.decoder_missing = Some(DecoderKey::Heif);
            let mut p = probe_of(i);
            p.skip = false;
            p.format = Some(ps_model::Format::Heic);
            FileRow {
                key: key(src, i),
                stat: stat_of(i),
                results: Results::Analyzed(p, Box::new(a)),
            }
        })
        .collect();
    cat.writer().upsert_files(rows).unwrap();
    cat.writer()
        .thumbed((0..n).map(|i| key(src, i)).collect())
        .unwrap();
    cat.writer().flush().unwrap();
    let before: Vec<_> = (0..n)
        .map(|i| cat.read().file(&key(src, i)).unwrap().unwrap())
        .collect();
    assert!(
        before
            .iter()
            .all(|r| r.decoder_missing == Some(DecoderKey::Heif))
    );
    assert_eq!(
        cat.read().undecoded_counts(&[src]).unwrap(),
        vec![(src, "heic".into(), DecoderKey::Heif, n)]
    );
    let commits = cat.writer().stats().commits;
    let h = |i: u64| PerceptualHash {
        phash: i * 7 + 1,
        dhash: i * 11 + 3,
        version: 1,
    };
    cat.writer()
        .submit(Command::Redecoded(
            (0..n).map(|i| (key(src, i), h(i))).collect(),
        ))
        .unwrap();
    cat.writer().flush().unwrap();
    assert_eq!(cat.writer().stats().commits - commits, 1, "one commit");
    for (i, b) in before.iter().enumerate() {
        let a = cat.read().file(&key(src, i as u64)).unwrap().unwrap();
        assert_eq!(a.decoder_missing, None);
        assert_eq!(a.hashes, Some(h(i as u64)));
        assert_eq!(a.stage, b.stage);
        assert_eq!(a.class, b.class);
        assert_eq!(a.identity, b.identity);
        assert_eq!(a.capture_ms, b.capture_ms);
        assert_eq!(a.capture_provenance, b.capture_provenance);
        assert_eq!(a.camera_model, b.camera_model);
        assert_eq!((a.width, a.height), (b.width, b.height));
    }
    assert!(cat.read().undecoded_counts(&[src]).unwrap().is_empty());
    // Clearing without hashes.
    let mut a = analysis_of(5000);
    a.hashes = None;
    a.decoder_missing = Some(DecoderKey::Heif);
    let mut p = probe_of(5000);
    p.skip = false;
    cat.writer()
        .upsert_files(vec![FileRow {
            key: key(src, 5000),
            stat: stat_of(5000),
            results: Results::Analyzed(p, Box::new(a)),
        }])
        .unwrap();
    cat.writer()
        .submit(Command::ClearDecoderMissing(vec![key(src, 5000)]))
        .unwrap();
    cat.writer().flush().unwrap();
    let r = cat.read().file(&key(src, 5000)).unwrap().unwrap();
    assert_eq!((r.decoder_missing, r.hashes), (None, None));
}

/// Migration 20 (ADR-0098): a decode failure is stored with the analysis, replaced by a failed re-decode, and
/// cleared by a re-decode that fills the row in.
#[test]
fn decode_failures_are_stored_replaced_and_cleared() {
    use ps_model::DecodeFailure;
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    let failure = |transient, attempts, backend: &str| DecodeFailure {
        key: DecoderKey::Heif,
        backend: backend.into(),
        transient,
        attempts,
    };
    let mut a = analysis_of(7);
    a.hashes = None;
    a.decode_failure = Some(failure(true, 1, "libheif 1.23.3"));
    let mut p = probe_of(7);
    p.skip = false;
    p.format = Some(ps_model::Format::Heic);
    cat.writer()
        .upsert_files(vec![FileRow {
            key: key(src, 7),
            stat: stat_of(7),
            results: Results::Analyzed(p, Box::new(a)),
        }])
        .unwrap();
    cat.writer().flush().unwrap();
    let r = cat.read().file(&key(src, 7)).unwrap().unwrap();
    assert_eq!(r.decode_failure, Some(failure(true, 1, "libheif 1.23.3")));
    cat.writer()
        .submit(Command::DecodeFailed(vec![(
            key(src, 7),
            failure(true, 2, "libheif 1.23.3"),
        )]))
        .unwrap();
    cat.writer().flush().unwrap();
    let r = cat.read().file(&key(src, 7)).unwrap().unwrap();
    assert_eq!(r.decode_failure.unwrap().attempts, 2);
    // A failure record alone lets a re-decode write its hashes.
    cat.writer()
        .submit(Command::Redecoded(vec![(
            key(src, 7),
            PerceptualHash {
                phash: 1,
                dhash: 2,
                version: 1,
            },
        )]))
        .unwrap();
    cat.writer().flush().unwrap();
    let r = cat.read().file(&key(src, 7)).unwrap().unwrap();
    assert_eq!(r.decode_failure, None);
    assert!(r.hashes.is_some());
    // The stored text form, and a backend name containing the separator.
    let f = failure(false, 1, "WIC | HEVC 2.1");
    assert_eq!(f.to_storage(), "heif|p|1|WIC | HEVC 2.1");
    assert_eq!(DecodeFailure::from_storage(&f.to_storage()), Some(f));
    assert_eq!(DecodeFailure::from_storage("heif|x|1|b"), None);
}
