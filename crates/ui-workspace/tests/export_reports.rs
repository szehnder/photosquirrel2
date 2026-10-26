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
        Some("Copy 12,418 files + 37 already there")
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
    assert_eq!(started[0].1.profile, ExportProfile::Lightroom);
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
                ps_service::export_progress::ProfileSettings::defaults(ExportProfile::ApplePhotos),
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

/// The Export view starts from the settings (profile, library, sidecar preference) and the sidecar switch
/// changes this export only; the library exported to is remembered in `export.library`.
#[gpui_kit::test]
fn scenario_export_sidecars_adjusted_per_export(cx: &mut TestAppContext) {
    let backend = Arc::new(DemoBackend::new(FakeScanService::new(
        FakeConfig::two_sources(100.0),
    )));
    let (shell, mut cx) = demo_app(cx, backend.clone());
    let lib = PathBuf::from("/media/backup/Family Library");
    cx.update(|_, cx| {
        ui_workspace::settings::AppSettings::update(cx, |s| {
            s.export.profile = ps_model::settings::ExportProfile::Google;
            s.export.library = Some(lib.clone());
        })
        .unwrap()
    });
    show(&shell, ViewId::Export, &mut cx);
    wait(&mut cx, "the plan", |cx| exists("export-plan", cx));
    assert_eq!(
        label("export-library", &mut cx).as_deref(),
        Some("/media/backup/Family Library")
    );
    // Google Photos: no sidecars by default; this export turns them on.
    click("export-sidecars", &mut cx);
    wait(&mut cx, "the replan", |cx| exists("export-plan", cx));
    click("export-confirm", &mut cx);
    click("export-start", &mut cx);
    wait(&mut cx, "the export result", |cx| {
        exists("export-result", cx)
    });
    let started = backend.exports_started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].0, lib);
    assert_eq!(started[0].1.profile, ExportProfile::GooglePhotos);
    assert!(started[0].1.sidecars, "adjusted for this export");
    let saved = cx.update(|_, cx| ui_workspace::settings::AppSettings::get(cx).export.clone());
    assert_eq!(saved.library.as_deref(), Some(lib.as_path()));
    assert!(saved.sidecars, "the default itself is unchanged");
    assert_eq!(saved.profile, ps_model::settings::ExportProfile::Google);
}

/// Every file under a library folder (bytes), PhotoSquirrel's own folder left out.
fn library_files(lib: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![lib.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                if !p.ends_with(".photosquirrel") {
                    stack.push(p);
                }
            } else {
                out.insert(
                    p.strip_prefix(lib).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    out
}

/// Two exported groups whose keeper then changed (ADR-0092), plus one new photo to export.
#[gpui_kit::test]
fn scenario_export_export_panel_offers_better_copies(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("phone");
    std::fs::create_dir_all(src.join("web")).unwrap();
    for (i, seed) in [101u64, 202].into_iter().enumerate() {
        let (low, high) = ps_fixtures::media::quality_pair(seed);
        std::fs::write(src.join(format!("IMG_{i}.JPG")), &high).unwrap();
        std::fs::write(src.join(format!("web/IMG_{i} small.jpg")), &low).unwrap();
    }
    let data = tmp.path().join("data");
    let library = tmp.path().join("library");
    let backend = Arc::new(CatalogBackend::open(&data).unwrap());
    backend.add_source(&src).unwrap();
    let scan = backend.scan().unwrap();
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    let google =
        ps_service::export_progress::ProfileSettings::defaults(ExportProfile::GooglePhotos);
    let out = backend
        .export_run(
            &library,
            google,
            &AtomicBool::new(false),
            &ExportProgress::new(),
        )
        .unwrap();
    assert_eq!(out.copied, 2, "{out:?}");
    // The user keeps the other copy of each exported group.
    let groups = scan
        .groups()
        .list(ps_service::groups::GroupOrder::Id, 0, 10)
        .unwrap();
    assert_eq!(groups.len(), 2, "{groups:?}");
    for g in &groups {
        let d = scan.groups().show(g.id).unwrap();
        let other = d.members.iter().find(|m| !m.keeper).unwrap().file;
        scan.groups().set_keeper(g.id, other).unwrap();
    }
    // A new photo, so the export has something to copy.
    std::fs::write(
        src.join("IMG_9.JPG"),
        ps_fixtures::media::quality_pair(909).1,
    )
    .unwrap();
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    let earlier = library_files(&library);
    assert_eq!(earlier.len(), 2);

    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    cx.update(|_, cx| {
        ui_workspace::settings::AppSettings::update(cx, |s| {
            s.export.profile = ps_model::settings::ExportProfile::Google;
        })
        .unwrap()
    });
    let defaults = cx.update(|_, cx| ui_workspace::settings::AppSettings::get(cx).export.clone());
    // Tall enough that the whole plan is painted (headless tests see only painted rows).
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1440.0), gpui_kit::px(2400.0)));
    show(&shell, ViewId::Export, &mut cx);
    wait(&mut cx, "the better copies", |cx| {
        exists("plan-better-copies", cx)
    });
    assert_eq!(
        label("plan-better-copies", &mut cx).as_deref(),
        Some("2 better copies available")
    );
    assert_eq!(
        label("plan-upgrade", &mut cx).as_deref(),
        Some("Also copy the better copies beside the library copies: off"),
        "off each time the panel opens"
    );
    click("plan-better-copies-toggle", &mut cx);
    wait(&mut cx, "the list", |cx| exists("plan-better-copy-1", cx));
    for i in 0..2 {
        let row = label(&format!("plan-better-copy-{i}"), &mut cx).unwrap();
        assert!(row.starts_with("Group "), "{row}");
        let (lib_copy, better) = row.split_once(" in the library; better copy ").unwrap();
        assert!(lib_copy.ends_with(".JPG"), "{row}");
        assert!(
            better.contains("small.jpg") && better.contains("JPEG"),
            "{row}"
        );
    }
    // First export: only the new photo.
    assert_eq!(
        label("export-confirm", &mut cx).as_deref(),
        Some("Copy 1 file")
    );
    click("export-confirm", &mut cx);
    click("export-start", &mut cx);
    wait(&mut cx, "the first export", |cx| {
        exists("export-result", cx)
    });
    let after_first = library_files(&library);
    assert_eq!(after_first.len(), 3, "{:?}", after_first.keys());
    for (k, v) in &earlier {
        assert_eq!(after_first.get(k), Some(v));
    }
    // Then with the better copies.
    click("export-replan", &mut cx);
    wait(&mut cx, "the replan", |cx| exists("plan-upgrade", cx));
    click("plan-upgrade", &mut cx);
    wait(&mut cx, "the upgrading plan", |cx| {
        label("export-confirm", cx).as_deref() == Some("Copy 2 files")
    });
    click("export-confirm", &mut cx);
    click("export-start", &mut cx);
    wait(&mut cx, "the second export", |cx| {
        exists("export-result", cx)
    });
    let after = library_files(&library);
    assert_eq!(after.len(), 5, "{:?}", after.keys());
    for (k, v) in &earlier {
        assert_eq!(after.get(k), Some(v), "{} unchanged", k.display());
    }
    // Each better copy sits in its earlier copy's folder.
    for k in earlier.keys() {
        let folder = k.parent().unwrap();
        assert!(
            after
                .keys()
                .any(|p| p.parent() == Some(folder) && p.to_string_lossy().contains("small")),
            "a better copy beside {}",
            k.display()
        );
    }
    let saved = cx.update(|_, cx| ui_workspace::settings::AppSettings::get(cx).export.clone());
    assert_eq!(saved.profile, defaults.profile);
    assert_eq!(
        saved.sidecars, defaults.sidecars,
        "the defaults are unchanged"
    );
}

/// The Reports view names better copies not in the library (ADR-0093): after the keeper of an exported group is
/// changed to another member, the badge says "1 better copy not in the library yet", the legend lists it and a
/// note points to `export run --upgrade`.
#[gpui_kit::test]
fn reports_view_lists_better_copies_not_in_the_library(cx: &mut TestAppContext) {
    let e = exported_corpus();
    let backend = Arc::new(CatalogBackend::open(&e.data).unwrap());
    {
        let svc = backend.scan().unwrap();
        let groups = svc
            .groups()
            .list(ps_service::groups::GroupOrder::Reclaimable, 0, 100)
            .unwrap();
        let (gid, other) = groups
            .iter()
            .find_map(|g| {
                let d = svc.groups().show(g.id).ok()?;
                // Another content than the exported keeper: a derivative (an exact copy is in the library).
                let m = d.members.iter().find(|m| {
                    !d.keeper_set.contains(&m.file)
                        && m.edge == ps_service::groups::MemberEdge::Derivative
                })?;
                Some((d.id, m.file))
            })
            .expect("a group with a derivative member");
        svc.groups().set_keeper(gid, other).unwrap();
    }
    let report = backend.report(None, false).unwrap().unwrap();
    let src = report
        .sources
        .iter()
        .find(|s| s.counts.files_of(Disposition::Superseded) > 0)
        .expect("a source with a better copy not in the library");
    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    show(&shell, ViewId::Reports, &mut cx);
    let badge = format!("report-badge-{}", src.id.0);
    wait(&mut cx, "the report", |cx| exists(&badge, cx));
    let l = label(&badge, &mut cx).unwrap();
    assert!(l.contains("1 better copy not in the library yet"), "{l}");
    assert!(exists("report-total-superseded", &mut cx));
    assert!(exists("report-superseded-note", &mut cx));
}
