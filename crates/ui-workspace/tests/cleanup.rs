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
    assert!(exists("cleanup-folder-0", &mut cx), "and the folders");
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
        label(&undo, cx).as_deref() == Some("Undo")
    });
    click(&undo, &mut cx);
    wait(&mut cx, "the undo confirmation", |cx| {
        exists("cleanup-undo-confirmation", cx)
    });
    assert_ne!(snapshot(&drive), before, "asking moves nothing back yet");
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
