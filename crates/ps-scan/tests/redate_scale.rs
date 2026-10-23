//! Task 4.4 (ADR-0071): the cost of re-dating a synthetic 1M-row catalog, 30% of whose rows are dated only by
//! their file time (the stale rows after migration 17). A measurement, not a budget:
//! `cargo test --release -p ps-scan --test redate_scale -- --ignored --nocapture`.

use std::time::Instant;

use ps_catalog::{AnalysisResult, Catalog, FileKey, FileRow, NewSource, ProbeResult, Results};
use ps_media::RuleZone;
use ps_model::{
    CaptureDate, CaptureTime, ContentId, DatePrecision, DateProvenance, Format, MediaMetadata,
    Metadata, PhotoClass, RelPath, StatFingerprint,
};
use ps_scan::Control;
use ps_scan::redate::{REDATE_CHUNK, StaleDate, redate};

const ROWS: u64 = 1_000_000;

fn row(source: ps_model::SourceId, i: u64) -> FileRow {
    let stale = i % 10 < 3;
    let folder = if i.is_multiple_of(4) {
        format!(
            "{:04}-{:02}-{:02} Trip {}",
            2000 + i % 20,
            1 + i % 12,
            1 + i % 28,
            i / 1000
        )
    } else {
        format!("DCIM/{:03}CANON", i / 1000)
    };
    let mtime_s = 1_300_000_000 + (i as i64 * 7919) % 400_000_000;
    let exif = CaptureTime::from_parts(
        2010 + (i % 10) as i32,
        1 + (i % 12) as u8,
        1 + (i % 28) as u8,
        10,
        0,
        0,
    );
    let mut tags: Vec<(String, String, String)> = (0..20)
        .map(|k| {
            (
                "Exif".to_string(),
                format!("Tag{k}"),
                format!("value {i} {k}"),
            )
        })
        .collect();
    tags.push(("IFD0".into(), "Make".into(), "Canon".into()));
    let capture_date = if stale {
        CaptureDate {
            value: CaptureTime {
                local_seconds: mtime_s,
                millis: 0,
                offset_minutes: Some(0),
            },
            provenance: DateProvenance::FileMtime,
            undated: true,
            precision: DatePrecision::Second,
        }
    } else {
        CaptureDate {
            value: exif,
            provenance: DateProvenance::DateTimeOriginal,
            undated: false,
            precision: DatePrecision::Second,
        }
    };
    let mut digest = [0u8; 32];
    digest[..8].copy_from_slice(&i.to_le_bytes());
    FileRow {
        key: FileKey::new(
            source,
            RelPath::from_bytes(format!("{folder}/IMG_{i:07}.JPG").into_bytes()).expect("rel"),
        ),
        stat: StatFingerprint {
            size: 3_000_000,
            mtime_ns: mtime_s * 1_000_000_000,
            file_index: Some(i),
        },
        results: Results::Analyzed(
            ProbeResult {
                format: Some(Format::Jpeg),
                class: PhotoClass::Photo,
                reasons: vec!["camera EXIF".into()],
                width: Some(4000),
                height: Some(3000),
                skip: false,
                facts: None,
                classifier_hash: Some(1),
            },
            Box::new(AnalysisResult {
                identity: ContentId::full(digest),
                hashes: None,
                metadata: Metadata {
                    fields: MediaMetadata {
                        width: Some(4000),
                        height: Some(3000),
                        capture_time: (!stale).then_some(exif),
                        camera_make: Some("Canon".into()),
                        all_tags: tags,
                        ..MediaMetadata::default()
                    },
                    capture_date: Some(capture_date),
                    ..Metadata::default()
                },
            }),
        ),
    }
}

#[test]
#[ignore = "measurement on a 1M-row catalog: run with --release --ignored --nocapture"]
fn re_dating_a_million_row_catalog() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let tmp = tempfile::tempdir_in(&base).expect("tmp");
    let cat = Catalog::open(tmp.path()).expect("catalog");
    let source = cat
        .writer()
        .add_source(NewSource {
            root: b"/mnt/scale".to_vec(),
            ..NewSource::default()
        })
        .expect("source");
    let t = Instant::now();
    for start in (0..ROWS).step_by(2_000) {
        let rows = (start..(start + 2_000).min(ROWS))
            .map(|i| row(source, i))
            .collect();
        cat.writer().upsert_files(rows).expect("rows");
    }
    cat.writer().flush().expect("flush");
    eprintln!("wrote {ROWS} rows in {:?}", t.elapsed());
    // The previous date rules wrote these rows: the file-mtime ones are stale (migration 17 leaves them at 1).
    let c = rusqlite::Connection::open(ps_catalog::db::catalog_path(tmp.path())).expect("db");
    c.execute(
        "UPDATE file SET date_version = 1 WHERE capture_provenance = 'file-mtime'",
        [],
    )
    .expect("stale");
    drop(c);

    let t = Instant::now();
    let mut stale = Vec::new();
    let mut after = None;
    loop {
        let page = cat
            .read()
            .source_files(source, after.as_ref(), 4096)
            .expect("page");
        for r in &page {
            if r.date_version < ps_model::DATE_LOGIC_VERSION && r.capture_provenance.is_some() {
                stale.push(StaleDate {
                    id: r.id,
                    path: r.key.path.clone(),
                    current: r.capture_date(),
                });
            }
        }
        if page.len() < 4096 {
            break;
        }
        after = page.last().map(|r| r.key.path.clone());
    }
    let listed = t.elapsed();
    let t = Instant::now();
    let ctl = Control::new();
    let writer = cat.writer().clone();
    let out = redate(
        source,
        &stale,
        cat.read(),
        &RuleZone::LOS_ANGELES,
        &ctl,
        REDATE_CHUNK,
        |cmd| writer.submit(cmd).expect("submit"),
    )
    .expect("redate");
    writer.flush().expect("flush");
    let pass = t.elapsed();
    eprintln!(
        "re-dated {} stale rows of {ROWS} ({} dates changed): listing {listed:?}, re-date pass {pass:?}",
        out.written, out.changed
    );
    assert_eq!(out.written, stale.len() as u64);
    assert!(out.written >= ROWS * 3 / 10 - 1);
}
