//! The scanning scenarios through the service facade (scanning task 6.1): sources, a full scan, progress,
//! pause/resume, cancel + resume, a no-read rescan, crash recovery, offline and re-link, removal.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ps_fixtures::{DirWriter, Params, Preset};
use ps_model::Stage;
use ps_service::scan::{
    Confirmation, RunStatus, ScanError, ScanOptions, ScanService, SourceStatus,
};

fn tiny() -> Params {
    Params {
        standalone: 12,
        sets_per_kind: 2,
        burst_sets: 3,
        converted_raw_bursts: 0,
        videos: 2,
        scans: 2,
        icons: 3,
        ui_graphics: 3,
        tiny_web: 4,
        junk_videos: 2,
        raw_share: 0.25,
        dim_scale: (1, 2),
        twelve_mp: 0,
        backups: (1, 2),
    }
}

fn corpus(dir: &Path) -> (PathBuf, ps_fixtures::Truth) {
    let root = dir.join("corpus");
    let w = DirWriter::new(&root).expect("writer");
    let truth = ps_fixtures::generate_with(7, Preset::Small, tiny(), &w).expect("generate");
    (root, truth)
}

fn wait_for(s: &ScanService, what: &str, f: impl Fn(&ps_service::scan::ProgressSnapshot) -> bool) {
    let t = Instant::now();
    loop {
        if s.snapshot().as_ref().is_some_and(&f) {
            return;
        }
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn facade_scan_lifecycle() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (root, truth) = corpus(tmp.path());
    let data = tmp.path().join("data");
    let svc = ScanService::open(&data, ScanOptions::default()).expect("open");
    assert!(matches!(
        svc.add_source(&tmp.path().join("nope")),
        Err(ScanError::SourceMissing(_))
    ));
    let ids: Vec<_> = truth
        .drives
        .iter()
        .map(|d| svc.add_source(&root.join(d)).expect("add").source.id)
        .collect();
    assert_eq!(svc.sources().expect("sources").len(), ids.len());

    // Full scan with pause/resume in the middle.
    let run = svc.start(&[]).expect("start");
    assert!(matches!(svc.start(&[]), Err(ScanError::RunActive(r)) if r == run));
    wait_for(&svc, "analysis", |s| s.processed > 0);
    svc.pause().expect("pause");
    wait_for(&svc, "paused", |s| {
        s.active_workers == 0 && s.status == RunStatus::Paused
    });
    svc.resume().expect("resume");
    let summary = svc.wait().expect("summary");
    assert_eq!(summary.status, RunStatus::Completed);
    assert_eq!(summary.stages.total(), truth.files.len() as u64);
    assert_eq!(summary.stages.discovered + summary.stages.probed, 0);
    let report = svc.report(run).expect("report");
    assert_eq!(report.record.status, RunStatus::Completed);
    assert_eq!(report.summary.as_ref(), Some(&summary));

    // A rescan of the unchanged sources reads nothing.
    let s2 = {
        svc.start(&[]).expect("rescan");
        svc.wait().expect("summary")
    };
    assert_eq!(s2.bytes_read, 0);
    assert_eq!(s2.diff.unchanged, truth.files.len() as u64);

    // Hash upgrade at the current version is a no-op.
    assert_eq!(svc.upgrade_hashes().expect("upgrade").stale, 0);

    // Removal needs confirmation and leaves the disk alone.
    assert!(matches!(
        svc.remove_source(ids[0], Confirmation::NotConfirmed),
        Err(ScanError::NotConfirmed)
    ));
    svc.remove_source(ids[0], Confirmation::Confirmed)
        .expect("remove");
    assert_eq!(svc.sources().expect("sources").len(), ids.len() - 1);
    assert!(root.join(&truth.drives[0]).is_dir());
}

#[test]
fn facade_cancel_resume_and_relink() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (root, truth) = corpus(tmp.path());
    let data = tmp.path().join("data");
    let svc = ScanService::open(
        &data,
        ScanOptions {
            read_delay: Some(Duration::from_millis(5)),
            ..ScanOptions::default()
        },
    )
    .expect("open");
    for d in &truth.drives {
        svc.add_source(&root.join(d)).expect("add");
    }
    let run = svc.start(&[]).expect("start");
    wait_for(&svc, "some progress", |s| s.processed >= 10);
    svc.cancel().expect("cancel");
    assert_eq!(svc.wait().expect("summary").status, RunStatus::Cancelled);
    assert_eq!(svc.resume_run(run).expect("resume"), run);
    assert_eq!(svc.wait().expect("summary").status, RunStatus::Completed);

    // Unplug a drive, re-link it at a new mount point.
    let src = svc.sources().expect("sources")[0].clone();
    let moved = tmp.path().join("remounted");
    std::fs::rename(&src.root, &moved).expect("move");
    let list = svc.refresh_sources().expect("refresh");
    assert_eq!(list[0].status, SourceStatus::Offline);
    svc.relink_source(src.id, &moved).expect("relink");
    svc.start(&[src.id]).expect("rescan");
    let s = svc.wait().expect("summary");
    assert_eq!((s.diff.new, s.diff.changed), (0, 0));
    assert!(
        svc.engine()
            .catalog()
            .read()
            .stage_counts(src.id)
            .expect("counts")
            .iter()
            .all(|(st, _)| st.is_terminal() || *st == Stage::Failed)
    );
    drop(svc);
    // Re-opening finds nothing to recover after a clean shutdown.
    let again = ScanService::open(&data, ScanOptions::default()).expect("reopen");
    assert!(again.interrupted_runs().is_empty());
    assert_eq!(again.runs().expect("runs").len(), 2);
}
