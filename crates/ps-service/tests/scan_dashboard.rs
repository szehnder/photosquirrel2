//! Scan dashboard service additions (change `scan-dashboard`): the run summary of a real scan of a small
//! corpus equals the catalog's totals, and the full error listing.

use std::path::{Path, PathBuf};

use ps_catalog::library_classes;
use ps_fixtures::{DirWriter, Params, Preset};
use ps_model::{Filter, MediaKind};
use ps_service::scan::{RunStatus, ScanOptions, ScanService};

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

#[test]
fn scenario_scan_progress_run_summary_on_completion() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (root, truth) = corpus(tmp.path());
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    let ids: Vec<_> = truth
        .drives
        .iter()
        .map(|d| svc.add_source(&root.join(d)).expect("add").source.id)
        .collect();
    assert_eq!(svc.last_ended_run().expect("runs"), None);
    let run = svc.start(&[]).expect("start");
    let summary = svc.wait().expect("summary");
    assert_eq!(summary.status, RunStatus::Completed);
    assert_eq!(svc.last_ended_run().expect("runs"), Some(run));

    let o = svc.run_overview(run).expect("overview");
    assert_eq!(o.run, run);
    assert_eq!(o.status, RunStatus::Completed);
    let mut sources = o.sources.clone();
    sources.sort();
    assert_eq!(sources, ids);
    assert!(o.elapsed_ms > 0, "the summary shows a duration");
    assert!(o.ended_at_ms.is_some());

    // Counts equal the catalog's totals for the run.
    let all = truth.files.len() as u64;
    assert_eq!(o.processed, all, "every file was processed");
    assert_eq!(o.new, all, "a first scan finds every file new");
    assert_eq!((o.changed, o.missing), (0, 0));
    assert_eq!(o.skipped, summary.stages.skipped);
    assert_eq!(o.failed, summary.stages.failed);
    assert_eq!(o.bytes_read, summary.bytes_read);
    assert_eq!((o.damaged, summary.damaged), (0, 0), "an intact corpus");
    let r = svc.engine().catalog().read();
    let count = |kind| {
        r.count(&Filter {
            classes: Some(library_classes()),
            kind: Some(kind),
            sources: Some(ids.clone()),
            ..Filter::default()
        })
        .expect("count")
    };
    assert_eq!(o.photos, count(MediaKind::Image));
    assert_eq!(o.videos, count(MediaKind::Video));
    assert!(o.photos > 0 && o.videos > 0, "{o:?}");
    let groups = r.group_totals().expect("groups").0;
    assert!(groups > 0);
    assert_eq!(o.duplicate_groups, groups);
    assert!(o.incomplete_sources.is_empty());

    // The full error listing matches the stored error count.
    let errors = svc.run_errors(run, 10_000).expect("errors");
    assert_eq!(errors.len() as u64, o.errors);
}

/// The run summary's damaged count (ADR-0115) is the catalog's, in the service overview and the scan summary.
#[test]
fn run_summary_counts_damaged_files() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (root, truth) = corpus(tmp.path());
    let shot = ps_fixtures::dng::converted_burst_frame(61, 0, 0, 960);
    let cut = ps_fixtures::dng::cut(&shot.cr2, ps_fixtures::dng::Cut::AfterPreview).expect("cut");
    std::fs::write(root.join(&truth.drives[0]).join("IMG_CUT.CR2"), cut).expect("write");
    let svc = ScanService::open(&tmp.path().join("data"), ScanOptions::default()).expect("open");
    let ids: Vec<_> = truth
        .drives
        .iter()
        .map(|d| svc.add_source(&root.join(d)).expect("add").source.id)
        .collect();
    let run = svc.start(&[]).expect("start");
    let summary = svc.wait().expect("summary");
    let o = svc.run_overview(run).expect("overview");
    assert_eq!((o.damaged, summary.damaged), (1, 1), "the cut CR2");
    assert_eq!(
        svc.engine()
            .catalog()
            .read()
            .damaged_count(&ids)
            .expect("damaged"),
        1
    );
}
