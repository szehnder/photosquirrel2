//! The batching writer (catalog tasks 2.1, 2.3) and stage semantics.

mod common;

use std::time::Duration;

use common::*;
use ps_catalog::{
    Catalog, CatalogError, Command, FileFailure, FileKey, FileRow, OpenOptions, WriterConfig,
};
use ps_model::{Filter, RunStatus, Stage, StatFingerprint};

fn kflags_consistent(cat: &Catalog) {
    let bad: i64 = cat
        .read()
        .with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM file WHERE kflags !=
                 (coalesce(class, 7) | (coalesce(media_kind, 0) << 3) | ((missing_since IS NOT NULL) << 4))",
                [],
                |r| r.get(0),
            )?)
        })
        .expect("kflags");
    assert_eq!(bad, 0, "kflags out of sync");
}

fn count(cat: &Catalog) -> u64 {
    cat.read()
        .count(&Filter {
            include_missing: true,
            ..Filter::default()
        })
        .expect("count")
}

#[test]
fn scenario_catalog_batch_flushes_at_2000_rows() {
    // With the timer effectively disabled, only the row limit can commit the batch.
    let tmp = tempfile::tempdir().expect("tempdir");
    let opts = OpenOptions {
        writer: WriterConfig {
            max_delay: Duration::from_secs(600),
            ..WriterConfig::default()
        },
        ..OpenOptions::default()
    };
    let cat = Catalog::open_with(tmp.path(), opts).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let before = cat.writer().stats();
    let rows: Vec<FileRow> = (0..2000).map(|i| discovered_row(src, i)).collect();
    cat.writer().upsert_files(rows).expect("submit");
    let seen = wait_until(Duration::from_secs(10), || count(&cat) == 2000);
    assert!(seen.is_some(), "2,000 rows committed without the timer");
    let after = cat.writer().stats();
    assert_eq!(after.commits - before.commits, 1, "one transaction");
    assert_eq!(after.rows_committed - before.rows_committed, 2000);

    // 2,001 rows: one full batch commits at once, the extra row waits for the timer / a flush.
    let rows: Vec<FileRow> = (2000..4001).map(|i| discovered_row(src, i)).collect();
    cat.writer().upsert_files(rows).expect("submit");
    assert!(wait_until(Duration::from_secs(10), || count(&cat) == 4000).is_some());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(count(&cat), 4000, "the 2,001st row is in the next batch");
    cat.writer().flush().expect("flush");
    assert_eq!(count(&cat), 4001);
}

#[test]
fn scenario_catalog_batch_flushes_after_100_ms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let rows: Vec<FileRow> = (0..10).map(|i| discovered_row(src, i)).collect();
    cat.writer().upsert_files(rows).expect("submit");
    let seen = wait_until(Duration::from_secs(5), || count(&cat) == 10).expect("visible");
    assert!(seen <= Duration::from_millis(150), "visible after {seen:?}");
    assert_eq!(cat.writer().stats().commits, 2, "source + one timed batch");
}

#[test]
fn scenario_catalog_same_path_upsert_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let row = full_row(src, 1);
    cat.writer().upsert_files(vec![row.clone()]).expect("first");
    cat.writer().flush().expect("flush");
    let first = cat.read().file(&row.key).expect("read").expect("row");
    let meta = cat.read().metadata(first.id).expect("meta");
    // Again, alone and as a discovered-only row with the same fingerprint.
    cat.writer()
        .upsert_files(vec![row.clone()])
        .expect("second");
    cat.writer()
        .upsert_files(vec![FileRow::discovered(row.key.clone(), row.stat)])
        .expect("third");
    cat.writer().flush().expect("flush");
    assert_eq!(count(&cat), 1, "exactly one row");
    let again = cat.read().file(&row.key).expect("read").expect("row");
    assert_eq!(again, first, "values unchanged");
    assert_eq!(cat.read().metadata(first.id).expect("meta"), meta);
    assert_eq!(first.stage, Stage::Analyzed);
    kflags_consistent(&cat);
}

#[test]
fn changed_fingerprint_resets_stage_and_results() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let i = (0..).find(|i| !probe_of(*i).skip).expect("candidate");
    let row = full_row(src, i);
    cat.writer()
        .upsert_files(vec![row.clone()])
        .expect("insert");
    cat.writer()
        .thumbed(vec![row.key.clone()])
        .expect("thumbed");
    cat.writer().flush().expect("flush");
    let r = cat.read().file(&row.key).expect("read").expect("row");
    assert_eq!(r.stage, Stage::Thumbed);
    let id = r.id;

    let changed = StatFingerprint {
        size: row.stat.size + 1,
        ..row.stat
    };
    cat.writer()
        .upsert_files(vec![FileRow::discovered(row.key.clone(), changed)])
        .expect("changed");
    cat.writer().flush().expect("flush");
    let r = cat.read().file(&row.key).expect("read").expect("row");
    assert_eq!(r.id, id, "same row");
    assert_eq!(r.stage, Stage::Discovered);
    assert_eq!(r.stat, changed);
    assert!(r.class.is_none() && r.identity.is_none() && r.hashes.is_none());
    assert!(cat.read().metadata(id).expect("meta").is_none());

    // Missing then seen again: missing_since cleared, results kept (fingerprint unchanged).
    cat.writer()
        .submit(Command::MarkMissing {
            files: vec![row.key.clone()],
            at_ms: 5,
        })
        .expect("missing");
    cat.writer().flush().expect("flush");
    let r = cat.read().file(&row.key).expect("read").expect("row");
    assert_eq!(r.missing_since_ms, Some(5));
    kflags_consistent(&cat);
    assert_eq!(cat.read().count(&Filter::default()).expect("count"), 0);
    cat.writer()
        .upsert_files(vec![FileRow::discovered(row.key.clone(), changed)])
        .expect("seen");
    cat.writer().flush().expect("flush");
    assert_eq!(
        cat.read()
            .file(&row.key)
            .expect("read")
            .expect("row")
            .missing_since_ms,
        None
    );
    kflags_consistent(&cat);
}

#[test]
fn stages_in_order_and_guards_report_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let w = cat.writer();
    let i = (0..).find(|i| !probe_of(*i).skip).expect("candidate");
    let key = FileKey::new(src, path_of(i));
    w.upsert_files(vec![discovered_row(src, i)]).expect("walk");
    // Analysis before probe is refused (stage must not get ahead of results).
    w.analyzed(vec![(key.clone(), Box::new(analysis_of(i)))])
        .expect("submit");
    w.thumbed(vec![key.clone()]).expect("submit");
    let err = w.flush().expect_err("guard errors reported");
    let CatalogError::Write(errs) = err else {
        panic!("unexpected {err:?}")
    };
    assert_eq!(errs.total, 2);
    assert_eq!(errs.errors[0].command, "analyzed");
    assert_eq!(errs.errors[0].file.as_ref(), Some(&key));
    assert_eq!(
        cat.read().file(&key).expect("r").expect("row").stage,
        Stage::Discovered
    );
    w.flush()
        .expect("errors are drained by the flush that reported them");

    w.probed(vec![(key.clone(), probe_of(i))]).expect("probe");
    w.analyzed(vec![(key.clone(), Box::new(analysis_of(i)))])
        .expect("analyze");
    w.flush().expect("flush");
    let r = cat.read().file(&key).expect("r").expect("row");
    assert_eq!(r.stage, Stage::Analyzed);
    assert_eq!(r.identity.map(|c| c.digest), Some(digest_of(i)));
    // Re-probe clears analysis results in the same statement.
    w.probed(vec![(key.clone(), probe_of(i))]).expect("reprobe");
    w.flush().expect("flush");
    let r = cat.read().file(&key).expect("r").expect("row");
    assert_eq!(r.stage, Stage::Probed);
    kflags_consistent(&cat);
    assert!(r.identity.is_none());

    // Failure: stage failed, attempts + 1, error row.
    let run = w.start_run(vec![src]).expect("run");
    w.failed(vec![FileFailure {
        key: key.clone(),
        stage: Stage::Analyzed,
        message: "decode error".into(),
        run: Some(run),
    }])
    .expect("fail");
    w.submit(Command::SetRunStatus {
        run,
        status: RunStatus::Completed,
        stats: Some(serde_json::json!({"files": 1})),
    })
    .expect("status");
    w.flush().expect("flush");
    let r = cat.read().file(&key).expect("r").expect("row");
    assert_eq!((r.stage, r.attempts), (Stage::Failed, 1));
    let errors: i64 = cat
        .read()
        .with(|c| Ok(c.query_row("SELECT count(*) FROM file_error", [], |r| r.get(0))?))
        .expect("errors");
    assert_eq!(errors, 1);
    let rr = cat.read().run(run).expect("run").expect("row");
    assert_eq!(rr.status, RunStatus::Completed);
    assert!(rr.ended_at_ms.is_some());
    assert_eq!(rr.stats["files"], 1);
    assert_eq!(rr.sources, vec![src]);
}

#[test]
fn running_runs_become_interrupted() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let run = cat.writer().start_run(vec![]).expect("run");
    drop(cat); // "crash" with the run left running
    let cat = Catalog::open(tmp.path()).expect("reopen");
    cat.writer()
        .submit(Command::InterruptRunning)
        .expect("submit");
    cat.writer().flush().expect("flush");
    let r = cat.read().run(run).expect("run").expect("row");
    assert_eq!(r.status, RunStatus::Interrupted);
}

#[test]
fn dropped_catalog_commits_pending_rows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    cat.writer()
        .upsert_files((0..10).map(|i| discovered_row(src, i)).collect())
        .expect("submit");
    let w = cat.writer().clone();
    drop(cat);
    // A clone kept alive keeps the writer running; dropping it commits and stops the thread.
    drop(w);
    let cat = Catalog::open(tmp.path()).expect("reopen");
    assert_eq!(count(&cat), 10);
}

#[test]
fn closed_writer_reports_closed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let w = cat.writer().clone();
    cat.close().expect("close");
    assert!(matches!(w.flush(), Err(CatalogError::WriterClosed)));
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_round_trip() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().expect("tempdir");
    let cat = Catalog::open(tmp.path()).expect("open");
    let src = add_source(&cat, "/mnt/a");
    let raw = std::path::Path::new(std::ffi::OsStr::from_bytes(b"F\xf6tos/\xff\xfe IMG.JPG"));
    let key = FileKey::new(src, ps_model::RelPath::from_path(raw).expect("rel"));
    let latin = FileKey::new(src, rel("Fötos/x.JPG"));
    cat.writer()
        .upsert_files(vec![
            FileRow::discovered(key.clone(), stat_of(1)),
            FileRow::discovered(latin.clone(), stat_of(2)),
        ])
        .expect("submit");
    cat.writer().flush().expect("flush");
    let r = cat.read().file(&key).expect("read").expect("row");
    assert_eq!(r.key.path.to_path_buf(), raw);
    assert_ne!(r.id, cat.read().file(&latin).expect("r").expect("row").id);
    let page = cat.read().source_files(src, None, 10).expect("page");
    assert_eq!(page.len(), 2);
    // Keyset continuation after the first path.
    let rest = cat
        .read()
        .source_files(src, Some(&page[0].key.path), 10)
        .expect("page");
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].key, page[1].key);
}
