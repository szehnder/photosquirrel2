//! Headless UI tests of updates in the app (spec `app-updates`, `app-shell` "First-launch onboarding", `settings`
//! "Settings window"; design D8): the update question, the notice banner and its actions, the status-bar item,
//! "Check for Updates…" and the Settings > General > Updates group. The update service talks to a loopback test
//! server, and app time is the test executor's clock.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::TestAppContext;
use ps_model::settings::UpdateMode;
use support::*;
use ui_workspace::settings::AppSettings;
use ui_workspace::updates_ui::UpdateHub;

fn saved_mode(dirs: &Dirs) -> UpdateMode {
    dirs.store().load().settings.updates.mode
}

fn mode(cx: &mut gpui_kit::VisualTestContext) -> UpdateMode {
    cx.update(|_, cx| AppSettings::get(cx).updates.mode)
}

/// Wait for the settings write that runs on the background executor.
fn wait_saved(cx: &mut gpui_kit::VisualTestContext, dirs: &Dirs, want: UpdateMode) {
    wait_until(cx, "the settings file", |_| saved_mode(dirs) == want);
}

#[gpui_kit::test]
fn scenario_app_updates_keep_me_up_to_date_enables_automatic_updates(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::default()), Some(&dirs));
    let updates = updates_recorded(cx);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    assert!(exists("onboarding", &mut cx));
    assert!(exists("updates-question", &mut cx), "the question is asked");
    assert_eq!(
        label("updates-question-yes", &mut cx).as_deref(),
        Some("Keep me up to date")
    );
    click("updates-question-yes", &mut cx);
    assert_eq!(mode(&mut cx), UpdateMode::Automatic);
    wait_saved(&mut cx, &dirs, UpdateMode::Automatic);
    assert!(
        !exists("updates-question", &mut cx),
        "answered: not asked again"
    );
    assert!(
        !exists("updates-question-banner", &mut cx),
        "nor in the banner"
    );
    // The next start reads the saved answer: the service's mode is automatic and nothing asks.
    assert_eq!(updates.service.status().mode, UpdateMode::Automatic);
    assert!(
        updates.recorded.lock().unwrap().is_empty(),
        "nothing sent yet"
    );
}

#[gpui_kit::test]
fn scenario_app_updates_upgraded_user_asked_once(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    // A settings file from before updates (version 3), and a library already scanned.
    write_settings(&dirs, "settings_version = 3\n");
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_recorded(cx);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    assert_eq!(mode(&mut cx), UpdateMode::Undecided);
    assert!(
        exists("updates-question-banner", &mut cx),
        "the one-time card above the library"
    );
    assert_eq!(
        label("updates-question-banner-title", &mut cx).as_deref(),
        Some(ui_workspace::updates_ui::QUESTION_TITLE)
    );
    click("updates-question-banner-no", &mut cx);
    assert_eq!(mode(&mut cx), UpdateMode::Off);
    wait_saved(&mut cx, &dirs, UpdateMode::Off);
    assert!(!exists("updates-question-banner", &mut cx));
    // Restarted: the saved mode is off, so the card is not shown; and two days pass without a request.
    let hub = cx.update(|_, cx| UpdateHub::model(cx).unwrap());
    assert!(!cx.update(|_, cx| hub.read(cx).asks_in_banner()));
    assert_eq!(dirs.store().load().settings.updates.mode, UpdateMode::Off);
    idle_minutes(&mut cx, 48 * 60);
    assert!(updates.recorded.lock().unwrap().is_empty(), "no request");
}

#[gpui_kit::test]
fn the_question_banner_stops_asking_after_three_closes(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    write_settings(
        &dirs,
        "settings_version = 4\n[updates]\nmode = \"undecided\"\nprompt_dismissals = 2\n",
    );
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let _updates = updates_recorded(cx);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    assert!(exists("updates-question-banner", &mut cx));
    click("updates-question-banner-close", &mut cx);
    assert!(!exists("updates-question-banner", &mut cx));
    assert_eq!(
        mode(&mut cx),
        UpdateMode::Off,
        "the third close turns updates off"
    );
    wait_saved(&mut cx, &dirs, UpdateMode::Off);
    assert_eq!(dirs.store().load().settings.updates.prompt_dismissals, 3);
}

fn automatic(dirs: &Dirs) {
    write_settings(
        dirs,
        "settings_version = 4\n[updates]\nmode = \"automatic\"\n",
    );
}

#[gpui_kit::test]
fn restart_now_button_waits_for_a_scan(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    automatic(&dirs);
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_offering(cx, "0.4.1", false, appimage);
    let scanning = Arc::new(AtomicBool::new(true));
    let probe = scanning.clone();
    updates.service.set_busy_probe(Arc::new(move || {
        probe.load(Ordering::Relaxed).then_some("the scan")
    }));
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    // The first scheduled check, a minute after start, finds and downloads 0.4.1.
    idle_minutes(&mut cx, 2);
    assert_eq!(updates.service.status().ready.as_deref(), Some("0.4.1"));
    assert_eq!(
        label("update-banner-title", &mut cx).as_deref(),
        Some("PhotoSquirrel 0.4.1 is ready. It installs when you quit.")
    );
    // The notes' Markdown bullets read as plain sentences, not raw "- " lines run together.
    assert_eq!(
        label("update-banner-title-body", &mut cx).as_deref(),
        Some("Faster scans. Clearer duplicate review in 0.4.1.")
    );
    assert_eq!(
        label("status-update", &mut cx).as_deref(),
        Some("Update ready: 0.4.1")
    );
    assert_eq!(
        label("update-restart", &mut cx).as_deref(),
        Some("Available when the scan finishes (unavailable)")
    );
    // The scan completes: Restart now is available.
    scanning.store(false, Ordering::Relaxed);
    idle_minutes(&mut cx, 1);
    assert_eq!(
        label("update-restart", &mut cx).as_deref(),
        Some("Restart now")
    );
    assert_eq!(updates.manifest_requests(), 1);
}

#[gpui_kit::test]
fn notify_mode_offers_install_on_quit_and_skip(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    write_settings(
        &dirs,
        "settings_version = 4\n[updates]\nmode = \"notify\"\n",
    );
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_offering(cx, "0.4.1", false, appimage);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    idle_minutes(&mut cx, 2);
    assert_eq!(
        label("update-banner-title", &mut cx).as_deref(),
        Some("PhotoSquirrel 0.4.1 is available.")
    );
    assert!(exists("update-install-on-quit", &mut cx));
    assert!(exists("update-whats-new", &mut cx));
    assert!(updates.state().ready.is_none(), "nothing downloaded yet");
    click("update-skip", &mut cx);
    assert!(!exists("update-banner", &mut cx));
    assert!(updates.state().is_skipped("0.4.1"));
}

#[gpui_kit::test]
fn urgent_updates_are_marked_until_dismissed(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    write_settings(
        &dirs,
        "settings_version = 4\n[updates]\nmode = \"notify\"\n",
    );
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_offering(cx, "0.4.1", true, appimage);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    idle_minutes(&mut cx, 2);
    assert_eq!(
        label("update-banner-title", &mut cx).as_deref(),
        Some("Important update. PhotoSquirrel 0.4.1 is available.")
    );
    // A day later it is still there.
    idle_minutes(&mut cx, 24 * 60);
    assert!(exists("update-banner", &mut cx));
    click("update-dismiss", &mut cx);
    assert!(!exists("update-banner", &mut cx));
    assert!(updates.state().dismissed.contains(&"0.4.1".to_owned()));
}

#[gpui_kit::test]
fn a_debian_package_offers_a_download(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    automatic(&dirs);
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let _updates = updates_offering(cx, "0.4.1", false, |_| ps_service::updates::Installation {
        kind: ps_service::updates::Kind::Deb,
        location: "/usr/bin".into(),
        cannot_install: Some("Installed with the system package manager".into()),
    });
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    idle_minutes(&mut cx, 2);
    assert_eq!(
        label("update-banner-title", &mut cx).as_deref(),
        Some("PhotoSquirrel 0.4.1 is available.")
    );
    assert_eq!(
        label("update-download", &mut cx).as_deref(),
        Some("Download")
    );
    assert!(
        !exists("update-restart", &mut cx),
        "it does not install itself"
    );
}

#[gpui_kit::test]
fn a_failed_install_names_both_versions_and_opens_the_details(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    automatic(&dirs);
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_offering(cx, "0.4.1", false, appimage);
    let updater =
        ps_update::testing::updater(updates.server.as_ref().unwrap(), &updates.dir, "0.4.0");
    ps_service::updates::record_failure(
        &updater,
        "0.4.1",
        "the new version did not report its version",
    );
    cx.update(|cx| {
        let hub = UpdateHub::model(cx).unwrap();
        hub.update(cx, |m, cx| m.refresh(cx));
    });
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    assert_eq!(
        label("update-banner-title", &mut cx).as_deref(),
        Some("Couldn't install PhotoSquirrel 0.4.1. You're still on 0.4.0.")
    );
    click("update-details", &mut cx);
    assert!(cx.update(|_, cx| shell.read(cx).settings_window().is_some()));
    assert!(
        exists("settings-updates-history", &mut cx),
        "the history is shown"
    );
}

#[gpui_kit::test]
fn check_for_updates_in_the_palette_makes_one_check(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    write_settings(&dirs, "settings_version = 4\n[updates]\nmode = \"off\"\n");
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_offering(cx, "0.4.1", false, appimage);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    cx.dispatch_action(ui_workspace::actions::CheckForUpdates);
    let s = shell.clone();
    wait_until(&mut cx, "the result", move |cx| {
        cx.update(|_, cx| s.read(cx).toast().is_some())
    });
    let toast = cx.update(|_, cx| shell.read(cx).toast().unwrap().body.to_string());
    assert_eq!(toast, "PhotoSquirrel 0.4.1 is available. You have 0.4.0.");
    idle_minutes(&mut cx, 60);
    assert_eq!(
        updates.manifest_requests(),
        1,
        "one request, and none on its own"
    );
}

#[gpui_kit::test]
fn scenario_settings_updates_group_shows_the_update_state(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    automatic(&dirs);
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_offering(cx, "0.4.0", false, appimage);
    // A completed check (up to date) and a kept previous version.
    updates.service.check_now().unwrap();
    let mut state = updates.state();
    state.kept = Some(ps_update::state::Kept {
        version: "0.3.1".into(),
        path: updates.dir.join("previous/0.3.1/PhotoSquirrel.AppImage"),
        max_catalog_schema: 27,
    });
    state.save(&ps_update::UpdateDir::at(&updates.dir)).unwrap();
    cx.update(|cx| {
        let hub = UpdateHub::model(cx).unwrap();
        hub.update(cx, |m, cx| m.refresh(cx));
    });
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    cx.dispatch_action(ui_workspace::actions::OpenSettings);
    assert!(exists("settings-updates", &mut cx));
    assert_eq!(
        label("settings-updates-version", &mut cx).as_deref(),
        Some("Version: PhotoSquirrel 0.4.0")
    );
    assert_eq!(
        label("settings-updates-mode", &mut cx).as_deref(),
        Some("Updates: Automatic")
    );
    assert_eq!(
        label("settings-updates-installation", &mut cx).as_deref(),
        Some("Installed as: AppImage, updates itself")
    );
    let last = label("settings-updates-last-check", &mut cx).unwrap();
    assert_eq!(last, "Last check: just now: up to date");
    assert_eq!(
        label("settings-updates-check", &mut cx).as_deref(),
        Some("Check for updates")
    );
    assert_eq!(
        label("settings-updates-go-back", &mut cx).as_deref(),
        Some("Go back to 0.3.1")
    );
    assert!(
        exists("setting-updates.mode", &mut cx),
        "the mode is a setting"
    );
}
