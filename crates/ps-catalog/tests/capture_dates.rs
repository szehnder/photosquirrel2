//! Migration 17 and the date columns (capture-dates tasks 3.1-3.4; ADR-0069, ADR-0070, ADR-0071).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use ps_catalog::{
    AnalysisResult, Catalog, Command, FileKey, FileRow, OpenOptions, Redate, Results, WriterConfig,
    db,
};
use ps_model::{
    CaptureDate, CaptureTime, DATE_LOGIC_VERSION, DatePrecision, DateProvenance, FileId,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params};

/// Gate review (task 8.3, ADR-0082): migration 18 gives every existing run `refresh_analysis = false`; a run
/// started with the refresh records it.
#[test]
fn migration_18_records_the_refresh_of_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, 17).unwrap();
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "INSERT INTO scan_run (id, status, started_at) VALUES (1, 'cancelled', 100);",
        )
        .unwrap();
    }
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    assert_eq!(
        db::stored_version(&path).unwrap(),
        Some(db::CATALOG_MIGRATIONS.len() as _)
    );
    let old = cat.read().run(ps_model::ScanRunId(1)).unwrap().unwrap();
    assert!(!old.refresh_analysis);
    let run = cat.writer().start_run_with(vec![], true).unwrap();
    assert!(cat.read().run(run).unwrap().unwrap().refresh_analysis);
    let plain = cat.writer().start_run(vec![]).unwrap();
    assert!(!cat.read().run(plain).unwrap().unwrap().refresh_analysis);
}

#[test]
fn migration_17_marks_rows_whose_date_cannot_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    db::create_catalog_at_version(&path, 16).unwrap();
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("INSERT INTO source (id, root_path, added_at) VALUES (1, x'2f61', 100);")
            .unwrap();
        let rows: &[(i64, Option<&str>, Option<i64>)] = &[
            (1, Some("DateTimeOriginal"), None),
            (2, Some("DateTimeOriginal"), Some(120)),
            (3, Some("CreateDate"), None),
            (4, Some("container"), Some(-240)),
            (5, Some("container"), Some(0)),
            (6, Some("file-mtime"), Some(0)),
            (7, None, None),
        ];
        for (id, prov, off) in rows {
            c.execute(
                "INSERT INTO file (id, source_id, rel_path, name, size, mtime_ns, stage, capture_ts,
                                   capture_offset_min, capture_provenance, capture_undated)
                 VALUES (?1, 1, ?2, ?2, 10, 0, 3, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    format!("f{id}.jpg").into_bytes(),
                    prov.map(|_| 1_000_i64),
                    off,
                    prov,
                    *prov == Some("file-mtime")
                ],
            )
            .unwrap();
        }
    }
    let cat = Catalog::open_file(&path, Default::default()).unwrap();
    assert_eq!(
        db::stored_version(&path).unwrap(),
        Some(db::latest(db::CATALOG_MIGRATIONS))
    );
    let versions: Vec<(i64, u8)> = (1..=7)
        .map(|id| {
            let r = cat.read().file_by_id(FileId(id)).unwrap().expect("row");
            assert_eq!(r.capture_precision, DatePrecision::Second);
            (id, r.date_version)
        })
        .collect();
    assert_eq!(
        versions,
        vec![(1, 2), (2, 2), (3, 2), (4, 2), (5, 1), (6, 1), (7, 1)],
        "EXIF and offset container rows current; file-mtime, +00:00 container and undated rows version 1"
    );
    let r = cat.read().file_by_id(FileId(6)).unwrap().unwrap();
    assert_eq!(r.capture_provenance, Some(DateProvenance::FileMtime));
    assert_eq!(r.capture_offset_min, Some(0));
}

fn folder_month_analysis(i: u64) -> AnalysisResult {
    let mut a = analysis_of(i);
    a.metadata.fields.capture_time = None;
    a.metadata.capture_date = Some(CaptureDate {
        value: CaptureTime::from_parts(2019, 7, 1, 0, 0, 0),
        provenance: DateProvenance::FolderName,
        undated: false,
        precision: DatePrecision::Month,
    });
    a
}

#[test]
fn month_precision_row_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    let mut probe = probe_of(1);
    probe.skip = false;
    let a = folder_month_analysis(1);
    let want = a.metadata.capture_date;
    cat.writer()
        .upsert_files(vec![FileRow {
            key: FileKey::new(src, rel("2019-07 Italy/IMG_0001.JPG")),
            stat: stat_of(1),
            results: Results::Analyzed(probe, Box::new(a)),
        }])
        .unwrap();
    cat.writer().flush().unwrap();
    let r = cat
        .read()
        .file(&FileKey::new(src, rel("2019-07 Italy/IMG_0001.JPG")))
        .unwrap()
        .expect("row");
    assert_eq!(r.capture_date(), want);
    assert_eq!(r.capture_precision, DatePrecision::Month);
    assert_eq!(r.capture_provenance, Some(DateProvenance::FolderName));
    assert_eq!(r.date_version, DATE_LOGIC_VERSION);
    assert!(!r.capture_undated);
}

/// Every column of every file row but the date columns.
fn non_date_columns(cat: &Catalog) -> Vec<Vec<Value>> {
    cat.read()
        .with(|c| {
            let mut s = c.prepare(
                "SELECT id, source_id, rel_path, name, size, mtime_ns, file_index, stage, class, reasons,
                        format, media_kind, width, height, content_id, identity_kind, phash, dhash,
                        hash_version, camera_model, has_gps, metadata, attempts, missing_since, kflags,
                        first_seen, class_facts, classifier_hash
                 FROM file ORDER BY id",
            )?;
            let n = s.column_count();
            let rows = s
                .query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect())?
                .collect::<rusqlite::Result<Vec<Vec<Value>>>>()?;
            Ok(rows)
        })
        .unwrap()
}

#[test]
fn redate_of_10000_rows_commits_once_and_leaves_other_columns_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = OpenOptions {
        writer: WriterConfig {
            max_rows: 20_000,
            max_delay: Duration::from_secs(600),
            ..WriterConfig::default()
        },
        ..OpenOptions::default()
    };
    let cat = Catalog::open_with(tmp.path(), opts).unwrap();
    let src = add_source(&cat, "/mnt/a");
    write_rows(&cat, src, 0..10_000, 1, full_row);
    let analysed: Vec<u64> = (0..10_000).filter(|i| !probe_of(*i).skip).collect();
    let before = non_date_columns(&cat);
    let stamps = cat.read().dedup_stamps().unwrap();
    let stats = cat.writer().stats();
    let day = CaptureDate {
        value: CaptureTime::from_parts(2019, 7, 4, 0, 0, 0),
        provenance: DateProvenance::FolderName,
        undated: false,
        precision: DatePrecision::Day,
    };
    let rows: Vec<Redate> = (0..10_000)
        .map(|i| Redate {
            key: FileKey::new(src, path_of(i)),
            capture: Some(day),
            version: DATE_LOGIC_VERSION,
        })
        .collect();
    cat.writer().submit(Command::Redate(rows)).unwrap();
    let errors = cat.writer().flush();
    // Junk rows were never analysed (no metadata): the writer reports them and leaves them alone.
    let skipped = 10_000 - analysed.len();
    match errors {
        Ok(()) => assert_eq!(skipped, 0),
        Err(e) => assert!(e.to_string().contains("unanalysed"), "{e}"),
    }
    let after = cat.writer().stats();
    assert_eq!(after.commits - stats.commits, 1, "one transaction");
    assert_eq!(
        non_date_columns(&cat),
        before,
        "only the date columns change"
    );
    assert_eq!(
        cat.read().dedup_stamps().unwrap(),
        stamps,
        "re-dating is no grouping change"
    );
    for i in analysed.iter().take(50) {
        let r = cat
            .read()
            .file(&FileKey::new(src, path_of(*i)))
            .unwrap()
            .unwrap();
        assert_eq!(r.capture_date(), Some(day));
        assert_eq!(r.date_version, DATE_LOGIC_VERSION);
    }
}

/// Task 3.4: a change of `capture_ts` alone (what re-dating writes) is no grouping work.
#[test]
fn capture_ts_only_change_produces_no_dedup_work() {
    let tmp = tempfile::tempdir().unwrap();
    let cat = Catalog::open(tmp.path()).unwrap();
    let src = add_source(&cat, "/mnt/a");
    write_rows(&cat, src, 0..500, 1, full_row);
    let before = cat.read().dedup_stamps().unwrap();
    assert!(!before.is_empty());
    cat.writer().flush().unwrap();
    let c = Connection::open(db::catalog_path(tmp.path())).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let n = c
        .execute(
            "UPDATE file SET capture_ts = capture_ts + 3600000, capture_offset_min = -420
             WHERE capture_ts IS NOT NULL",
            [],
        )
        .unwrap();
    assert!(n > 0);
    assert_eq!(cat.read().dedup_stamps().unwrap(), before);
}
