//! Headless UI test of the Cleanup panel in Reports (change `quarantine-cleanup`, task 5.3): on a real catalog
//! (a tiny corpus generated, scanned and exported in a temp dir), "Free up space" shows a plan and moves
//! nothing; only the confirmation's own button moves the files; Undo (after its own confirmation) restores
//! the run, and the drive's tree is as before.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use ps_model::ExportProfile;
use ps_service::export_progress::ExportProgress;
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::scan_backend::{CleanupRunState, ScanBackend as _};
use ui_workspace::views::ViewId;

fn show(shell: &Entity<Shell>, view: ViewId, cx: &mut VisualTestContext) {
    let shell = shell.clone();
    cx.update(|w, cx| shell.update(cx, |s, cx| s.show_view(view, w, cx)));
    cx.run_until_parked();
}

/// Run until `cond`, advancing the fake clock (the panel polls its background work on GPUI timers).
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
            start.elapsed() < Duration::from_secs(120),
            "timed out waiting for {what}"
        );
        cx.executor().advance_clock(Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(1));
    }
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

/// Every file and folder under `root` outside its quarantine folder.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if rel.starts_with(".photosquirrel-quarantine") {
                continue;
            }
            if p.is_dir() {
                out.insert(rel, None);
                stack.push(p);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    out
}

#[gpui_kit::test]
fn cleanup_panel_undo_restores_a_run(cx: &mut TestAppContext) {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(base).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("ui-cleanup-")
        .tempdir_in(base)
        .unwrap();
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
    let backend = Arc::new(CatalogBackend::open(&data).unwrap());
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
    let source = backend.report(None, false).unwrap().unwrap().sources[0].id;
    let drive = backend
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.id == source)
        .unwrap()
        .root;
    let before = snapshot(&drive);

    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    // Tall enough that the whole plan card and the history are painted (headless finds painted elements only).
    cx.simulate_resize(size(px(1400.0), px(3200.0)));
    wait_loaded(&shell, &mut cx);
    show(&shell, ViewId::Reports, &mut cx);
    let free = format!("report-cleanup-{}", source.0);
    wait(&mut cx, "the report", |cx| exists(&free, cx));

    // Free up space: a plan, nothing moved.
    click(&free, &mut cx);
    wait(&mut cx, "the plan", |cx| exists("cleanup-plan", cx));
    let runs = backend.cleanup_runs().unwrap();
    assert_eq!(runs.len(), 1);
    let run = runs[0].clone();
    assert_eq!(run.state, CleanupRunState::Planned);
    assert!(run.planned > 0);
    assert!(
        exists("cleanup-move-0", &mut cx),
        "the plan lists where files go"
    );
    wait(&mut cx, "the folder tree", |cx| {
        exists("cleanup-tree-row-0", cx)
    });
    assert_eq!(snapshot(&drive), before, "planning moves nothing");

    // "Move N files…" only opens the confirmation.
    click("cleanup-confirm", &mut cx);
    wait(&mut cx, "the confirmation", |cx| {
        exists("cleanup-confirmation", cx)
    });
    assert_eq!(snapshot(&drive), before, "the confirmation moves nothing");
    click("cleanup-back", &mut cx);
    assert!(!exists("cleanup-confirmation", &mut cx));
    assert_eq!(snapshot(&drive), before);

    click("cleanup-confirm", &mut cx);
    wait(&mut cx, "the confirmation", |cx| {
        exists("cleanup-confirmation", cx)
    });
    click("cleanup-start", &mut cx);
    wait(&mut cx, "the result", |cx| exists("cleanup-result", cx));
    let result = label("cleanup-result", &mut cx).unwrap();
    assert_eq!(
        result,
        format!("{} moved, 0 left in place", run.planned),
        "{result}"
    );
    assert_ne!(snapshot(&drive), before, "the files moved");
    assert_eq!(backend.cleanup_runs().unwrap()[0].quarantined, run.planned);

    // Undo, after its confirmation, restores the run.
    let undo = format!("cleanup-undo-{}", run.id);
    wait(&mut cx, "the history", |cx| {
        label(&undo, cx).as_deref() == Some("Undo…")
    });
    click(&undo, &mut cx);
    wait(&mut cx, "the undo confirmation", |cx| {
        exists("cleanup-undo-confirmation", cx)
    });
    assert_ne!(snapshot(&drive), before, "asking moves nothing back yet");
    let put_back = format!("Put back {} files", run.planned);
    wait(&mut cx, "the undo tree", |cx| {
        label("cleanup-undo-confirm", cx).as_deref() == Some(put_back.as_str())
    });
    click("cleanup-undo-confirm", &mut cx);
    wait(&mut cx, "the undo result", |cx| {
        exists("cleanup-undo-result", cx)
    });
    let text = label("cleanup-undo-result", &mut cx).unwrap();
    assert!(
        text.starts_with(&format!("Put back {} files", run.planned)),
        "{text}"
    );
    assert_eq!(snapshot(&drive), before, "the drive is as it was");
    let after = backend.cleanup_runs().unwrap();
    assert_eq!(after[0].state, CleanupRunState::Undone);
    assert_eq!(after[0].restored, run.planned);
    assert!(!exists(&undo, &mut cx), "nothing left to undo");
}

/// The plan's sample rows keep the run folder of a long quarantine path visible (`…/quarantine/<run>/`);
/// only the part below it is shortened.
#[test]
fn quarantine_target_keeps_the_run_folder() {
    use ui_workspace::views::cleanup::quarantine_target;
    let to = ".photosquirrel-quarantine/12/Backup 2008/Old Laptop/Users/dad/Pictures/Imports/2007-03-03 \
              Thanksgiving/IMG_5441_384.jpg";
    let shown = quarantine_target(to, 70);
    assert!(
        shown.starts_with(".photosquirrel-quarantine/12/"),
        "{shown}"
    );
    assert!(shown.ends_with("IMG_5441_384.jpg"), "{shown}");
    assert!(shown.chars().count() <= 70, "{shown}");
    assert_eq!(
        quarantine_target(".photosquirrel-quarantine/3/a.jpg", 70),
        ".photosquirrel-quarantine/3/a.jpg"
    );
}

// ---- cleanup-selection: narrowing, undo by folder and discard in the panel ----

/// A tiny corpus generated, scanned and exported into a temp dir, with the app open on Reports.
struct App {
    _tmp: tempfile::TempDir,
    backend: Arc<CatalogBackend>,
    source: ps_model::SourceId,
    drive: PathBuf,
}

fn app(cx: &mut TestAppContext, tag: &str) -> (App, VisualTestContext) {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(base).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix(&format!("ui-cleanup-{tag}-"))
        .tempdir_in(base)
        .unwrap();
    let root = tmp.path().join("corpus");
    let truth = ps_fixtures::generate_with(
        7,
        ps_fixtures::Preset::Small,
        tiny(),
        &ps_fixtures::DirWriter::new(&root).unwrap(),
    )
    .unwrap();
    let backend = Arc::new(CatalogBackend::open(&tmp.path().join("data")).unwrap());
    for d in &truth.drives {
        backend.add_source(&root.join(d)).unwrap();
    }
    let scan = backend.scan().unwrap();
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    let out = backend
        .export_run(
            &tmp.path().join("library"),
            ps_service::export_progress::ProfileSettings::defaults(ExportProfile::ApplePhotos),
            &AtomicBool::new(false),
            &ExportProgress::new(),
        )
        .unwrap();
    assert!(out.copied > 0 && out.failed.is_empty(), "{out:?}");
    let source = backend.report(None, false).unwrap().unwrap().sources[0].id;
    let drive = backend
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.id == source)
        .unwrap()
        .root;
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    vcx.simulate_resize(size(px(1400.0), px(3200.0)));
    wait_loaded(&shell, &mut vcx);
    show(&shell, ViewId::Reports, &mut vcx);
    let free = format!("report-cleanup-{}", source.0);
    wait(&mut vcx, "the report", |cx| exists(&free, cx));
    (
        App {
            _tmp: tmp,
            backend,
            source,
            drive,
        },
        vcx,
    )
}

impl App {
    /// "Free up space": the plan with its folder tree.
    fn plan(&self, cx: &mut VisualTestContext) -> ui_workspace::scan_backend::CleanupRunRow {
        click(&format!("report-cleanup-{}", self.source.0), cx);
        wait(cx, "the plan", |cx| exists("cleanup-plan", cx));
        wait(cx, "the folder tree", |cx| exists("cleanup-tree-row-0", cx));
        self.backend.cleanup_runs().unwrap()[0].clone()
    }

    /// The moves of a run by state.
    fn moves(&self, run: i64) -> Vec<ps_model::CleanupMove> {
        self.backend
            .scan()
            .unwrap()
            .cleanups()
            .run_detail(run)
            .unwrap()
            .1
    }
}

/// The folder path and file count of a tree row (`"<path>: <picked> of <files> files, <state>"`).
fn row_folder(prefix: &str, ix: usize, cx: &mut VisualTestContext) -> (String, u64) {
    let l = label(&format!("{prefix}-row-{ix}"), cx).unwrap();
    let (path, rest) = l.rsplit_once(": ").unwrap();
    let files = rest.split(" of ").nth(1).unwrap();
    let files = files.split(' ').next().unwrap().parse().unwrap();
    (path.to_owned(), files)
}

#[gpui_kit::test]
fn scenario_cleanup_app_narrows_a_plan_before_confirming(cx: &mut TestAppContext) {
    let (app, mut cx) = app(cx, "narrow");
    let before = snapshot(&app.drive);
    let run = app.plan(&mut cx);
    // Untick the first folder.
    let (folder, n) = row_folder("cleanup-tree", 0, &mut cx);
    assert!(n > 0 && n < run.planned, "{folder}: {n} of {}", run.planned);
    click("cleanup-tree-check-0", &mut cx);
    let want = run.planned - n;
    assert_eq!(
        label("cleanup-tree-row-0", &mut cx).unwrap(),
        format!("{folder}: 0 of {n} files, not selected")
    );
    assert_eq!(
        label("cleanup-confirm", &mut cx).as_deref(),
        Some(format!("Move {want} files…").as_str())
    );
    assert_eq!(snapshot(&app.drive), before, "unticking moves nothing");
    // "Move N files…" stores the selection, then confirms with the stored counts.
    click("cleanup-confirm", &mut cx);
    wait(&mut cx, "the confirmation", |cx| {
        exists("cleanup-confirmation", cx)
    });
    let stored = app.backend.cleanup_runs().unwrap()[0].clone();
    assert_eq!(stored.planned, want);
    assert_eq!(stored.unselected, n);
    assert_eq!(
        label("cleanup-confirmation", &mut cx).unwrap(),
        format!(
            "Move {want} files ({}) out of their folders on {}?",
            ui_workspace::views::kit::format_bytes(stored.planned_bytes),
            stored.source_name
        )
    );
    assert_eq!(
        snapshot(&app.drive),
        before,
        "the confirmation moves nothing"
    );
    click("cleanup-start", &mut cx);
    wait(&mut cx, "the result", |cx| exists("cleanup-result", cx));
    assert_eq!(
        label("cleanup-result", &mut cx).unwrap(),
        format!("{want} moved, 0 left in place")
    );
    // That folder's files are still at their paths; every other planned file is quarantined.
    let folder_rel = ps_model::RelPath::from_bytes(folder.clone().into_bytes()).unwrap();
    let now = snapshot(&app.drive);
    for m in app.moves(run.id) {
        let p = m.rel_path.to_path_buf();
        if m.rel_path.is_within(&folder_rel) {
            assert_eq!(m.state, ps_model::MoveState::Unselected);
            assert_eq!(now.get(&p), before.get(&p), "{} untouched", m.rel_path);
        } else {
            assert_eq!(m.state, ps_model::MoveState::Quarantined, "{}", m.rel_path);
            assert!(!now.contains_key(&p));
        }
    }
}

#[gpui_kit::test]
fn scenario_cleanup_app_discards_a_plan(cx: &mut TestAppContext) {
    let (app, mut cx) = app(cx, "discard");
    let before = snapshot(&app.drive);
    let run = app.plan(&mut cx);
    click("cleanup-discard", &mut cx);
    wait(&mut cx, "the discard confirmation", |cx| {
        exists("cleanup-discard-confirmation", cx)
    });
    assert_eq!(
        app.backend.cleanup_runs().unwrap()[0].state,
        CleanupRunState::Planned,
        "asking discards nothing"
    );
    click("cleanup-discard-confirm", &mut cx);
    wait(&mut cx, "the review to close", |cx| {
        !exists("cleanup-plan", cx)
    });
    let row = format!("cleanup-run-{}", run.id);
    wait(&mut cx, "the history", |cx| {
        label(&row, cx).is_some_and(|l| l.contains("Discarded"))
    });
    assert!(!exists(&format!("cleanup-review-{}", run.id), &mut cx));
    assert!(!exists(&format!("cleanup-undo-{}", run.id), &mut cx));
    assert!(!exists(&format!("cleanup-discard-{}", run.id), &mut cx));
    assert_eq!(
        app.backend.cleanup_runs().unwrap()[0].state,
        CleanupRunState::Discarded
    );
    assert_eq!(
        snapshot(&app.drive),
        before,
        "the drive's tree is unchanged"
    );
    assert!(!app.drive.join(".photosquirrel-quarantine").exists());
}

#[gpui_kit::test]
fn scenario_cleanup_app_undoes_one_folder(cx: &mut TestAppContext) {
    let (app, mut cx) = app(cx, "undo-folder");
    let before = snapshot(&app.drive);
    let run = app.plan(&mut cx);
    click("cleanup-confirm", &mut cx);
    wait(&mut cx, "the confirmation", |cx| {
        exists("cleanup-confirmation", cx)
    });
    click("cleanup-start", &mut cx);
    wait(&mut cx, "the result", |cx| exists("cleanup-result", cx));
    let undo = format!("cleanup-undo-{}", run.id);
    wait(&mut cx, "the history", |cx| {
        label(&undo, cx).as_deref() == Some("Undo…")
    });
    click(&undo, &mut cx);
    wait(&mut cx, "the undo tree", |cx| {
        exists("cleanup-undo-tree-row-0", cx)
    });
    // Untick everything, then tick one folder.
    click("cleanup-undo-tree-all", &mut cx);
    assert_eq!(
        label("cleanup-undo-confirm", &mut cx).as_deref(),
        Some("Nothing selected (unavailable)")
    );
    let (folder, n) = row_folder("cleanup-undo-tree", 0, &mut cx);
    assert!(n > 0 && n < run.planned);
    click("cleanup-undo-tree-check-0", &mut cx);
    assert_eq!(
        label("cleanup-undo-confirm", &mut cx).as_deref(),
        Some(format!("Put back {n} files").as_str())
    );
    click("cleanup-undo-confirm", &mut cx);
    wait(&mut cx, "the undo result", |cx| {
        exists("cleanup-undo-result", cx)
    });
    let folder_rel = ps_model::RelPath::from_bytes(folder.into_bytes()).unwrap();
    let now = snapshot(&app.drive);
    for m in app.moves(run.id) {
        let p = m.rel_path.to_path_buf();
        if m.rel_path.is_within(&folder_rel) {
            assert_eq!(m.state, ps_model::MoveState::Restored);
            assert_eq!(now.get(&p), before.get(&p), "{} is back", m.rel_path);
        } else {
            assert_eq!(m.state, ps_model::MoveState::Quarantined, "{}", m.rel_path);
            assert!(!now.contains_key(&p));
        }
    }
    let rest = run.planned - n;
    let row = format!("cleanup-run-{}", run.id);
    wait(&mut cx, "the history", |cx| {
        label(&row, cx).is_some_and(|l| l.contains(&format!("{rest} quarantined")))
    });
}

/// Gate review of `cleanup-selection`: while an older plan is under review with unsaved unticks, discarding
/// another plan from the history (which refreshes the panel, and a newer plan exists) keeps that review and its
/// ticks, and "Move N files…" stores and confirms the reviewed plan, not the newest one. The sample moves say they are the saved
/// plan's while ticks are unsaved.
#[gpui_kit::test]
fn cleanup_review_survives_a_refresh_after_discarding_another_plan(cx: &mut TestAppContext) {
    let (app, mut cx) = app(cx, "keep-review");
    let before = snapshot(&app.drive);
    let older = app.plan(&mut cx);
    // Two more plans of the same source: `other` will be discarded, `newest` stays the newest planned run.
    let other = app.plan(&mut cx);
    let newest = app.plan(&mut cx);
    assert!(older.id < other.id && other.id < newest.id, "{newest:?}");
    let in_run = |run: i64| format!("/{run}/");
    wait(&mut cx, "the newest plan", |cx| {
        label("cleanup-move-0", cx).is_some_and(|l| l.contains(&in_run(newest.id)))
    });
    // Review the older plan and untick its first folder (not saved yet).
    click(&format!("cleanup-review-{}", older.id), &mut cx);
    wait(&mut cx, "the older plan", |cx| {
        label("cleanup-move-0", cx).is_some_and(|l| l.contains(&in_run(older.id)))
            && exists("cleanup-tree-row-0", cx)
    });
    assert!(!exists("cleanup-moves-caption", &mut cx));
    let (folder, n) = row_folder("cleanup-tree", 0, &mut cx);
    click("cleanup-tree-check-0", &mut cx);
    let want = older.planned - n;
    let unticked = format!("{folder}: 0 of {n} files, not selected");
    assert_eq!(label("cleanup-tree-row-0", &mut cx).unwrap(), unticked);
    assert!(
        label("cleanup-moves-caption", &mut cx).is_some_and(|l| l.contains("saved plan")),
        "the sample moves say they are the saved plan's"
    );
    // Discard another plan from the history; the panel refreshes when it is done.
    click(&format!("cleanup-discard-{}", other.id), &mut cx);
    wait(&mut cx, "the discard confirmation", |cx| {
        exists("cleanup-discard-confirmation", cx)
    });
    click("cleanup-discard-confirm", &mut cx);
    let row = format!("cleanup-run-{}", other.id);
    wait(&mut cx, "the discard", |cx| {
        label(&row, cx).is_some_and(|l| l.contains("Discarded"))
    });
    for _ in 0..5 {
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
    }
    // Still the older plan, with the folder still unticked.
    assert!(
        label("cleanup-move-0", &mut cx).is_some_and(|l| l.contains(&in_run(older.id))),
        "the review still shows run {}",
        older.id
    );
    assert_eq!(label("cleanup-tree-row-0", &mut cx).unwrap(), unticked);
    assert_eq!(
        label("cleanup-confirm", &mut cx).as_deref(),
        Some(format!("Move {want} files…").as_str())
    );
    // "Move N files…" stores the older plan's selection and confirms it.
    click("cleanup-confirm", &mut cx);
    wait(&mut cx, "the confirmation", |cx| {
        exists("cleanup-confirmation", cx)
    });
    let runs = app.backend.cleanup_runs().unwrap();
    let get = |id: i64| runs.iter().find(|r| r.id == id).unwrap().clone();
    assert_eq!(get(older.id).planned, want);
    assert_eq!(get(older.id).unselected, n);
    assert_eq!(get(other.id).state, CleanupRunState::Discarded);
    assert_eq!(get(newest.id).state, CleanupRunState::Planned);
    assert_eq!(get(newest.id).unselected, 0, "the newest plan is untouched");
    assert!(
        label("cleanup-confirmation", &mut cx)
            .unwrap()
            .starts_with(&format!("Move {want} files ("))
    );
    assert_eq!(snapshot(&app.drive), before, "nothing moved");
}

/// A plan of 20,000 folders: the tree's first frame and a scroll build only the rows on screen (plus the
/// list's overscan), never one element per folder.
#[gpui_kit::test]
fn cleanup_tree_of_20k_folders_builds_only_visible_rows(cx: &mut TestAppContext) {
    use gpui_kit::AppContext as _;
    use gpui_kit::test::TestWindowExt as _;
    use ui_workspace::scan_backend::{DemoBackend, DemoCleanup};
    use ui_workspace::views::cleanup::{CleanupView, TreeOf};

    const FOLDERS: usize = 20_000;
    let demo = DemoBackend::new(ps_service::scans::FakeScanService::new(
        ps_service::scans::FakeConfig::two_sources(100.0),
    ));
    *demo.cleanup.lock().unwrap() = Some(DemoCleanup::synthetic(7, FOLDERS, 2));
    let demo = Arc::new(demo);
    init_app(cx, Arc::new(FakeBackend::small_corpus()), None);
    let b: ui_workspace::scan_backend::SharedScanBackend = demo.clone();
    let (handle, view) = cx
        .update(|cx| {
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| CleanupView::new(Some(b), cx))
            })
        })
        .unwrap();
    let mut cx = VisualTestContext::from_window(handle, cx);
    cx.simulate_resize(size(px(1400.0), px(1600.0)));
    wait(&mut cx, "the tree", |cx| exists("cleanup-tree-row-0", cx));
    assert_eq!(
        label("cleanup-tree", &mut cx).unwrap(),
        format!("{FOLDERS} folders, {FOLDERS} rows")
    );
    // The 20,000-folder tree was built off the UI thread (here the test's thread, which also runs the
    // dispatcher's background work).
    let built_on = cx.update(|_, cx| view.read(cx).plan_tree.as_ref().map(|t| t.built_on));
    assert!(
        built_on.is_some_and(|t| t != std::thread::current().id()),
        "{built_on:?}"
    );
    let built = |cx: &mut VisualTestContext, range: std::ops::Range<usize>| -> Vec<usize> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            range
                .filter(|ix| {
                    window
                        .try_find(gpui_kit::SharedString::from(format!(
                            "cleanup-tree-row-{ix}"
                        )))
                        .is_some_and(|s| s.visible())
                })
                .collect()
        })
    };
    let first = built(&mut cx, 0..400);
    assert!(first.contains(&0) && first.len() <= 12 + 8, "{first:?}");
    assert!(built(&mut cx, FOLDERS - 100..FOLDERS).is_empty());
    cx.update(|_, cx| view.read(cx).scroll_tree_to(TreeOf::Plan, 10_000));
    cx.run_until_parked();
    let after = built(&mut cx, 9_900..10_400);
    assert!(
        after.contains(&10_000) && after.len() <= 12 + 8,
        "{after:?}"
    );
    assert!(
        built(&mut cx, 0..50).is_empty(),
        "rows scrolled away are gone"
    );
    // Unticking one folder of 20,000 is exact and the stored plan is unchanged until "Move N files…".
    click("cleanup-tree-check-10000", &mut cx);
    let (picked, _) = cx.update(|_, cx| view.read(cx).picked());
    assert_eq!(picked, (FOLDERS as u64 - 1) * 2);
    assert!(
        demo.cleanup
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .selections
            .is_empty()
    );
}

/// The backend's selection tree of a corpus plan holds exactly the plan's folders, files and bytes
/// ([`ps_service::cleanup::totals_of`]), and a folder's file rows hold the same files.
#[test]
fn scan_backend_cleanup_tree_totals_match_the_plan() {
    use ui_workspace::scan_backend::CleanupTreeKind;
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(base).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("ui-cleanup-tree-")
        .tempdir_in(base)
        .unwrap();
    let root = tmp.path().join("corpus");
    let truth = ps_fixtures::generate_with(
        7,
        ps_fixtures::Preset::Small,
        tiny(),
        &ps_fixtures::DirWriter::new(&root).unwrap(),
    )
    .unwrap();
    let backend = CatalogBackend::open(&tmp.path().join("data")).unwrap();
    for d in &truth.drives {
        backend.add_source(&root.join(d)).unwrap();
    }
    let scan = backend.scan().unwrap();
    scan.start(&[]).unwrap();
    scan.wait().unwrap();
    backend
        .export_run(
            &tmp.path().join("library"),
            ps_service::export_progress::ProfileSettings::defaults(ExportProfile::ApplePhotos),
            &AtomicBool::new(false),
            &ExportProgress::new(),
        )
        .unwrap();
    let source = backend.report(None, false).unwrap().unwrap().sources[0].id;
    let review = backend.cleanup_plan(source).unwrap().unwrap();
    let run = review.run.id;
    let moves = scan.cleanups().run_detail(run).unwrap().1;
    let totals = ps_service::cleanup::totals_of(&moves);
    let tree = backend.cleanup_tree(run, CleanupTreeKind::Plan).unwrap();
    assert_eq!(tree.folders.len(), totals.folders.len());
    for (t, f) in tree.folders.iter().zip(&totals.folders) {
        assert_eq!(t.folder, f.folder);
        assert_eq!(t.files.len() as u64, f.files);
        assert_eq!(t.files.iter().map(|x| x.size).sum::<u64>(), f.bytes);
        assert!(t.files.iter().all(|x| x.picked));
        let rows = backend
            .cleanup_folder_files(run, t.folder.clone(), CleanupTreeKind::Plan)
            .unwrap();
        let mut a: Vec<_> = rows.iter().map(|r| (r.file, r.size)).collect();
        let mut b: Vec<_> = t.files.iter().map(|x| (x.file, x.size)).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }
    let sel = ui_workspace::views::cleanup_tree::SelectionTree::new(&tree);
    assert_eq!(sel.total(), (totals.files, totals.bytes));
    assert_eq!(sel.picked(), (totals.files, totals.bytes));
    // Nothing is quarantined yet: the undo tree is empty.
    assert!(
        backend
            .cleanup_tree(run, CleanupTreeKind::Undo)
            .unwrap()
            .folders
            .is_empty()
    );
}
