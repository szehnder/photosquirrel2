//! Headless UI tests for the shell (spec `app-shell`): real views in a test window, driven by keystrokes and
//! pointer events.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, point, px};
use ps_model::ScanRunId;
use support::*;
use ui_workspace::service::InterruptedRun;
use ui_workspace::views::{LibraryMode, ViewId};

#[gpui_kit::test]
fn scenario_app_shell_window_shows_main_areas(cx: &mut TestAppContext) {
    init_app(cx, Arc::new(FakeBackend::small_corpus()), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    for view in ViewId::ALL {
        assert!(
            exists(&format!("nav-{}", view.key()), &mut cx),
            "navigator lists {view:?}"
        );
    }
    assert_eq!(ViewId::ALL.len(), 7);
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).active_view()),
        ViewId::Library
    );
    assert!(
        exists("view-library", &mut cx),
        "the Library view is active"
    );
    assert!(!exists("onboarding", &mut cx));
    assert!(exists("inspector", &mut cx), "the inspector shows");
    assert!(exists("status-bar", &mut cx));
    assert_eq!(
        label("status-photo-count", &mut cx).as_deref(),
        Some("248 photos")
    );
    // The inspector collapses (I) and comes back.
    cx.simulate_keystrokes("i");
    assert!(!exists("inspector", &mut cx));
    cx.simulate_keystrokes("i");
    assert!(exists("inspector", &mut cx));
}

#[gpui_kit::test]
fn scenario_app_shell_first_launch_shows_onboarding(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::default()), Some(&dirs));
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    assert!(
        exists("onboarding", &mut cx),
        "central view shows onboarding"
    );
    assert!(
        !exists("inspector", &mut cx),
        "no inspector beside the welcome view of an empty library"
    );
    assert!(
        exists("onboarding-read-only", &mut cx),
        "explains sources are only read"
    );
    assert!(
        ui_workspace::onboarding::READ_ONLY_PROMISE.contains("never moves, changes or deletes")
    );
    assert!(
        exists("onboarding-add-source", &mut cx),
        "offers Add a source"
    );
    assert!(!dirs.data().join("catalog.db").exists());
}

#[gpui_kit::test]
fn scenario_app_shell_interrupted_run_offers_resume(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::small_corpus());
    *backend.interrupted.lock().unwrap() = Some(InterruptedRun {
        id: ScanRunId(7),
        label: "NAS · Photos".into(),
        files_done: 12_480,
        files_total: Some(30_112),
    });
    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    let s = shell.clone();
    wait_until(&mut cx, "the banner", move |cx| {
        cx.update(|_, cx| s.read(cx).banner().is_some())
    });
    assert!(exists("interrupted-banner", &mut cx));
    assert_eq!(
        label("interrupted-banner-title", &mut cx).as_deref(),
        Some("Scan of NAS · Photos was interrupted")
    );
    click("banner-resume", &mut cx);
    let b = backend.clone();
    wait_until(&mut cx, "resume", move |_| {
        !b.resumed.lock().unwrap().is_empty()
    });
    assert_eq!(*backend.resumed.lock().unwrap(), vec![ScanRunId(7)]);
    let s = shell.clone();
    wait_until(&mut cx, "banner gone", move |cx| {
        cx.update(|_, cx| s.read(cx).banner().is_none())
    });
    assert!(!exists("interrupted-banner", &mut cx));
}

#[gpui_kit::test]
fn scenario_app_shell_window_layout_restored(tcx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(tcx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let (shell, mut cx) = open(tcx);
    wait_loaded(&shell, &mut cx);
    // Resize the inspector by dragging its splitter 120 px to the left.
    let before = bounds("inspector-panel", &mut cx).size.width;
    let grip = bounds("splitter-inspector", &mut cx);
    let from = point(grip.center().x, grip.center().y);
    let to = point(from.x - px(120.0), from.y);
    cx.update(|window, cx| window.drag(from, to, cx));
    cx.run_until_parked();
    let after = bounds("inspector-panel", &mut cx).size.width;
    assert!(
        (after - before - px(120.0)).abs() < px(4.0),
        "inspector {before:?} -> {after:?}"
    );
    cx.simulate_keystrokes("d");
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).active_view()),
        ViewId::Duplicates
    );
    // Closing the window writes the state; a new window restores it.
    cx.update(|window, cx| shell.update(cx, |s, cx| s.save_now(window, cx)));
    let saved_width = cx.update(|_, cx| shell.read(cx).layout().inspector_width);
    assert!(dirs.data().join("ui-state.json").exists());

    let (shell2, mut cx2) = open(tcx);
    wait_loaded(&shell2, &mut cx2);
    let restored = cx2.update(|_, cx| shell2.read(cx).layout().clone());
    assert_eq!(restored.active_view, ViewId::Duplicates);
    assert!((restored.inspector_width - saved_width).abs() < 0.5);
    assert!(exists("view-duplicates", &mut cx2));
    // Duplicates shows its details in place: no inspector there unless asked for.
    assert!(!exists("inspector-panel", &mut cx2));
    cx2.simulate_keystrokes("g");
    cx2.run_until_parked();
    let width = bounds("inspector-panel", &mut cx2).size.width;
    assert!((width - after).abs() < px(1.0), "{width:?} vs {after:?}");
}

#[gpui_kit::test]
fn ui_state_is_saved_off_the_ui_thread_after_changes_settle(tcx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(tcx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let (shell, mut cx) = open(tcx);
    wait_loaded(&shell, &mut cx);
    let file = dirs.data().join("ui-state.json");
    let _ = std::fs::remove_file(&file);
    cx.simulate_keystrokes("d");
    cx.update(|window, cx| shell.update(cx, |s, cx| s.save_in_background(window, cx)));
    cx.run_until_parked();
    let text = std::fs::read_to_string(&file).expect("saved by the background task");
    assert!(text.contains("duplicates"), "{text}");
}

#[gpui_kit::test]
fn scenario_app_shell_shortcuts_switch_views(cx: &mut TestAppContext) {
    init_app(cx, Arc::new(FakeBackend::small_corpus()), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    let active = |cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|_, cx| (shell.read(cx).active_view(), shell.read(cx).library_mode()))
    };
    cx.simulate_keystrokes("d");
    assert_eq!(active(&mut cx), (ViewId::Duplicates, LibraryMode::Grid));
    assert!(exists("view-duplicates", &mut cx));
    cx.simulate_keystrokes("g");
    assert_eq!(active(&mut cx), (ViewId::Library, LibraryMode::Grid));
    assert!(exists("view-library", &mut cx));
    cx.simulate_keystrokes("e");
    assert_eq!(active(&mut cx), (ViewId::Library, LibraryMode::Loupe));
    cx.simulate_keystrokes("ctrl-,");
    assert!(
        exists("settings-window", &mut cx),
        "platform settings shortcut"
    );
    // Letters typed into the settings search do not switch views.
    cx.simulate_keystrokes("d");
    assert_eq!(active(&mut cx).0, ViewId::Library);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!exists("settings-window", &mut cx));
}

#[gpui_kit::test]
fn scenario_app_shell_palette_finds_commands(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::small_corpus());
    init_app(cx, backend.clone(), None);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    cx.simulate_keystrokes("ctrl-k");
    assert!(exists("command-palette", &mut cx));
    // Every registered command is listed with an empty query.
    let all = cx.update(|_, cx| shell.read(cx).palette().unwrap().read(cx).rows().len());
    assert_eq!(
        all,
        cx.update(|_, cx| ui_workspace::palette::CommandRegistry::commands(cx).len())
    );
    cx.simulate_input("rescan");
    cx.run_until_parked();
    let rows = cx.update(|_, cx| shell.read(cx).palette().unwrap().read(cx).rows().to_vec());
    let first = rows.first().expect("a match");
    assert_eq!(first.title, "Rescan All Sources");
    let expected = if cfg!(target_os = "macos") {
        "⌘R"
    } else {
        "Ctrl+R"
    };
    assert_eq!(
        first.shortcut.as_deref(),
        Some(expected),
        "listed with its shortcut"
    );
    assert!(exists("palette-row-0", &mut cx));
    // Enter runs it.
    cx.simulate_keystrokes("enter");
    let b = backend.clone();
    wait_until(&mut cx, "the rescan", move |_| {
        b.rescans.load(std::sync::atomic::Ordering::SeqCst) == 1
    });
    assert!(!exists("command-palette", &mut cx), "the palette closes");
}

#[gpui_kit::test]
fn scenario_app_shell_slow_query_keeps_ui_responsive(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::small_corpus());
    *backend.summary_delay.lock().unwrap() = Duration::from_secs(2);
    init_app(cx, backend, None);
    let (shell, mut cx) = open(cx);
    // The user opens a view while the catalog query is still running.
    cx.simulate_keystrokes("d");
    assert!(
        exists("center-loading", &mut cx),
        "the view shows a loading state"
    );
    let start = Instant::now();
    let frames_before = cx.update(|_, cx| shell.read(cx).frames_drawn());
    let mut times: Vec<Duration> = Vec::new();
    let mut frames = 0;
    while start.elapsed() < Duration::from_millis(1500) {
        let t = Instant::now();
        cx.update(|window, cx| {
            window.refresh();
            window.render_frame(cx);
        });
        cx.run_until_parked();
        times.push(t.elapsed());
        frames += 1;
        assert!(
            cx.update(|_, cx| shell.read(cx).summary().is_loading()),
            "still loading"
        );
        std::thread::sleep(Duration::from_millis(8));
    }
    let frames_after = cx.update(|_, cx| shell.read(cx).frames_drawn());
    assert!(frames > 50, "only {frames} frames in 1.5 s");
    assert!(frames_after > frames_before, "the shell re-rendered");
    // Headless frames share the CPU with the other tests in this binary, so judge the distribution: the
    // p95 frame meets the 60 fps budget and no frame waited anywhere near the 2 s query.
    times.sort();
    let p95 = times[times.len() * 95 / 100];
    let worst = *times.last().unwrap();
    println!("frames while loading: {frames}, p95 {p95:?}, worst {worst:?}");
    assert!(
        p95 < Duration::from_micros(16_700),
        "p95 frame {p95:?} while the query ran"
    );
    assert!(
        worst < Duration::from_millis(250),
        "a frame blocked for {worst:?}"
    );
    wait_loaded(&shell, &mut cx);
    assert!(!exists("center-loading", &mut cx));
    assert!(exists("view-duplicates", &mut cx));
}
