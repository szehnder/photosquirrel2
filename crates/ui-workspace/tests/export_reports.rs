//! Headless UI tests for the Export and Reports views (change `export`, tasks 6.1 and 6.2): confirming starts
//! the export (simulated engine), and the Reports drill-down lists exactly the pending files of a folder (real
//! catalog, scanned and exported in a temp dir).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_model::{Disposition, ExportProfile};
use ps_service::export_progress::ExportProgress;
use ps_service::scans::{FakeConfig, FakeScanService};
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::scan_backend::{DemoBackend, ScanBackend as _};
use ui_workspace::views::ViewId;

fn show(shell: &Entity<Shell>, view: ViewId, cx: &mut VisualTestContext) {
    let shell = shell.clone();
    cx.update(|w, cx| shell.update(cx, |s, cx| s.show_view(view, w, cx)));
    cx.run_until_parked();
}

/// Run until `cond`, advancing the fake clock (the views poll on GPUI timers).
fn wait(
    cx: &mut VisualTestContext,
    what: &str,
    mut cond: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if cond(cx) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}"
        );
        cx.executor().advance_clock(Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn demo_app(
    cx: &mut TestAppContext,
    backend: Arc<DemoBackend>,
) -> (Entity<Shell>, VisualTestContext) {
    init_app(cx, Arc::new(FakeBackend::small_corpus()), None);
    let b = backend.clone();
    cx.update(move |cx| {
        ui_workspace::views::scan::install(b, Arc::new(ui_gallery::source::EmptySource), cx)
    });
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    (shell, vcx)
}

#[gpui_kit::test]
fn confirming_the_export_starts_it(cx: &mut TestAppContext) {
    let backend = Arc::new(DemoBackend::new(FakeScanService::new(
        FakeConfig::two_sources(100.0),
    )));
    let (shell, mut cx) = demo_app(cx, backend.clone());
    show(&shell, ViewId::Export, &mut cx);
    wait(&mut cx, "the plan", |cx| exists("export-plan", cx));
    // Lightroom profile, then review: nothing is copied before the confirmation.
    click("export-profile-2", &mut cx);
    wait(&mut cx, "the Lightroom plan", |cx| {
        exists("export-plan", cx)
    });
    assert_eq!(
        label("export-confirm", &mut cx).as_deref(),
        Some("Export 12,455 files")
    );
    click("export-confirm", &mut cx);
    wait(&mut cx, "the confirmation", |cx| {
        exists("export-confirmation", cx)
    });
    assert!(backend.exports_started.lock().unwrap().is_empty());
    // Back returns to the review without exporting.
    click("export-back", &mut cx);
    assert!(!exists("export-confirmation", &mut cx));
    click("export-confirm", &mut cx);
    click("export-start", &mut cx);
    wait(&mut cx, "the export result", |cx| {
        exists("export-result", cx)
    });
    let started = backend.exports_started.lock().unwrap().clone();
    assert_eq!(started.len(), 1, "confirming started one export");
    assert_eq!(started[0].1, ExportProfile::Lightroom);
    let result = label("export-result", &mut cx).unwrap();
    assert!(result.starts_with("12381 copied"), "{result}");
}

#[gpui_kit::test]
fn insufficient_space_blocks_the_export_button(cx: &mut TestAppContext) {
    let backend = Arc::new(DemoBackend::new(FakeScanService::new(
        FakeConfig::two_sources(100.0),
    )));
    *backend.free_bytes.lock().unwrap() = Some(1_000_000_000);
    let (shell, mut cx) = demo_app(cx, backend.clone());
    show(&shell, ViewId::Export, &mut cx);
    wait(&mut cx, "the plan", |cx| exists("export-plan", cx));
    assert!(exists("plan-blocker", &mut cx));
    click("export-confirm", &mut cx);
    assert!(!exists("export-confirmation", &mut cx), "blocked");
    assert!(backend.exports_started.lock().unwrap().is_empty());
}

/// A tiny corpus over three drives, scanned and exported; one library copy is then removed so its keeper
/// (and the keeper's group) is pending again.
struct Exported {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    library: PathBuf,
}

fn tiny() -> ps_fixtures::Params {
    ps_fixtures::Params {
        standalone: 12,
        sets_per_kind: 2,
        burst_sets: 3,
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

fn exported_corpus() -> Exported {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("corpus");
    let truth = ps_fixtures::generate_with(
        7,
        ps_fixtures::Preset::Small,
        tiny(),
        &ps_fixtures::DirWriter::new(&root).unwrap(),
    )
    .unwrap();
    let data = tmp.path().join("data");
    let library = tmp.path().join("library");
    {
        let backend = CatalogBackend::open(&data).unwrap();
        for d in &truth.drives {
            backend.add_source(&root.join(d)).unwrap();
        }
        let scan = backend.scan().unwrap();
        scan.start(&[]).unwrap();
        scan.wait().unwrap();
        let out = backend
            .export_run(
                &library,
                ExportProfile::ApplePhotos,
                &AtomicBool::new(false),
                &ExportProgress::new(),
            )
            .unwrap();
        assert!(out.copied > 0 && out.failed.is_empty(), "{out:?}");
    }
    Exported {
        _tmp: tmp,
        data,
        library,
    }
}

#[gpui_kit::test]
fn scenario_completion_report_drill_into_pending_files(cx: &mut TestAppContext) {
    let e = exported_corpus();
    let backend = Arc::new(CatalogBackend::open(&e.data).unwrap());
    let ex = backend.scan().unwrap().exports();
    let library = ex.last_library().unwrap().unwrap();
    assert_eq!(
        library,
        e.library.canonicalize().unwrap_or(e.library.clone())
    );
    // Remove the library copy of one exported keeper in the first source (a test-owned temp library).
    let report = backend.report(None, false).unwrap().unwrap();
    let source = report.sources[0].id;
    let (files, d, _) = ex.dispositions(&library, false).unwrap();
    let (keeper, copy) = files
        .iter()
        .zip(&d)
        .find_map(|(f, x)| {
            (f.source == source && x.disposition == Disposition::Exported)
                .then(|| (f.clone(), x.library_path.clone().unwrap()))
        })
        .expect("an exported keeper");
    std::fs::remove_file(copy.under(&library)).unwrap();
    let folder: Option<String> = keeper.rel_path.parent().map(|p| p.display().into_owned());
    let report = backend.report(None, false).unwrap().unwrap();
    let src = report.sources.iter().find(|s| s.id == source).unwrap();
    let (row, folder_row) = src
        .folders
        .iter()
        .enumerate()
        .find(|(_, f)| f.folder == folder)
        .expect("the keeper's folder row");
    let pending = folder_row.counts.files_of(Disposition::Pending);
    assert!(pending > 0, "the keeper is pending again");

    // The expected files: exactly the pending files directly in that folder.
    let (files, d, _) = ex.dispositions(&library, false).unwrap();
    let root = backend
        .scan()
        .unwrap()
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.id == source)
        .unwrap()
        .root;
    let want: BTreeSet<String> = files
        .iter()
        .zip(&d)
        .filter(|(f, x)| {
            f.source == source
                && x.disposition == Disposition::Pending
                && f.rel_path.parent().map(|p| p.display().into_owned()) == folder
        })
        .map(|(f, _)| f.rel_path.under(&root).display().to_string())
        .collect();
    assert_eq!(want.len() as u64, pending);

    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    show(&shell, ViewId::Reports, &mut cx);
    let badge = format!("report-badge-{}", source.0);
    wait(&mut cx, "the report", |cx| exists(&badge, cx));
    assert_eq!(
        label(&badge, &mut cx),
        Some(format!(
            "{} pending",
            src.counts.files_of(Disposition::Pending)
        )),
        "the drive is not fully accounted for"
    );
    // The first source is expanded: click its folder's pending count.
    let cell = format!("report-cell-{}-{row}-pending", source.0);
    wait(&mut cx, "the folder table", |cx| exists(&cell, cx));
    assert_eq!(label(&cell, &mut cx), Some(format!("Pending {pending}")));
    click(&cell, &mut cx);
    wait(&mut cx, "the drill-down", |cx| exists("drill-row-0", cx));
    let mut got = BTreeSet::new();
    for i in 0.. {
        let Some(l) = label(&format!("drill-row-{i}"), &mut cx) else {
            break;
        };
        let (path, reason) = l.split_once(" — ").unwrap();
        assert_eq!(reason, "not in the library yet");
        got.insert(path.to_owned());
    }
    assert_eq!(got, want, "exactly the pending files of that folder");
    let _: &Path = &e.library;
}

/// Verify re-hashes the library: an altered copy is listed as suspect and its keeper turns pending.
#[gpui_kit::test]
fn verify_lists_an_altered_library_copy(cx: &mut TestAppContext) {
    let e = exported_corpus();
    let backend = Arc::new(CatalogBackend::open(&e.data).unwrap());
    let ex = backend.scan().unwrap().exports();
    let library = ex.last_library().unwrap().unwrap();
    let (_, d, _) = ex.dispositions(&library, false).unwrap();
    let copy = d
        .iter()
        .find(|x| x.disposition == Disposition::Exported)
        .and_then(|x| x.library_path.clone())
        .unwrap()
        .under(&library);
    // Same size and modification time, different bytes (a test-owned temp library): only a re-hash notices.
    let mtime = std::fs::metadata(&copy).unwrap().modified().unwrap();
    let mut bytes = std::fs::read(&copy).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&copy, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&copy)
        .and_then(|f| f.set_modified(mtime))
        .unwrap();

    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    show(&shell, ViewId::Reports, &mut cx);
    wait(&mut cx, "the report", |cx| exists("report-badge-all", cx));
    assert_eq!(
        label("report-badge-all", &mut cx).as_deref(),
        Some("100% accounted for"),
        "a size and time check does not notice"
    );
    assert_eq!(
        label("report-check-mode", &mut cx).as_deref(),
        Some("checked for presence, size and time")
    );
    click("report-verify", &mut cx);
    wait(&mut cx, "the verified report", |cx| {
        label("report-check-mode", cx).as_deref() == Some("verified by hash")
    });
    assert_eq!(
        label("report-mismatches", &mut cx).as_deref(),
        Some("1 suspect library copies")
    );
    let row = label("report-mismatch-0", &mut cx).unwrap();
    assert!(
        row.ends_with("contents changed in the library since it was exported"),
        "{row}"
    );
    let badge = label("report-badge-all", &mut cx).unwrap();
    assert!(badge.contains("pending"), "{badge}");
}
