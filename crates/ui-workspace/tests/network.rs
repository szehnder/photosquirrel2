//! Spec `app-shell`, "Quiet start": no network connection at start-up or while idle (ADR-0029).
//!
//! Its own test binary, so no other test's sockets share the process. The shell starts with default settings
//! (LM Studio configured, AI queue stopped), idles for 10 s of app time, and the process must hold no TCP or
//! UDP socket. The dependency check makes sure no HTTP client is linked into the UI crates except through
//! `ps-llm` (which is constructed lazily, never at start-up).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::TestAppContext;
use support::*;

#[gpui_kit::test]
fn scenario_app_shell_no_network_at_startup(cx: &mut TestAppContext) {
    let before = ui_workspace::net::inet_sockets(None);
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    // Idle for 10 s of app time (timers, debounced saves, toasts all run).
    for _ in 0..100 {
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
    }
    std::thread::sleep(Duration::from_millis(200));
    match (before, ui_workspace::net::inet_sockets(None)) {
        (Some(before), Some(after)) => {
            assert!(before.is_empty(), "sockets before start: {before:?}");
            assert!(
                after.is_empty(),
                "network sockets after start-up and idle: {after:?}"
            );
        }
        _ => eprintln!("skipping the socket check: not available on this platform"),
    }
    // No HTTP client is linked into the app's own crates (ps-llm and ps-update hold theirs, behind ps-service).
    for (name, manifest) in [
        ("ui-workspace", include_str!("../Cargo.toml")),
        ("ui-gallery", include_str!("../../ui-gallery/Cargo.toml")),
        ("ui-theme", include_str!("../../ui-theme/Cargo.toml")),
        (
            "photosquirrel",
            include_str!("../../photosquirrel/Cargo.toml"),
        ),
    ] {
        for client in ["reqwest", "ureq", "hyper", "isahc", "surf"] {
            assert!(
                !manifest.contains(&format!("\n{client}")),
                "{name} depends on {client}"
            );
        }
    }
}

fn assert_no_sockets(before: Option<Vec<String>>) {
    std::thread::sleep(Duration::from_millis(200));
    match (before, ui_workspace::net::inet_sockets(None)) {
        (Some(before), Some(after)) => {
            assert!(before.is_empty(), "sockets before start: {before:?}");
            assert!(after.is_empty(), "network sockets while idle: {after:?}");
        }
        _ => eprintln!("skipping the socket check: not available on this platform"),
    }
}

#[gpui_kit::test]
fn scenario_app_shell_no_network_while_updates_are_undecided_or_off(cx: &mut TestAppContext) {
    use ps_model::settings::UpdateMode;
    let before = ui_workspace::net::inet_sockets(None);
    let dirs = Dirs::new();
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_recorded(cx);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    // Undecided: 48 hours of app time.
    assert_eq!(updates.service.status().mode, UpdateMode::Undecided);
    idle_minutes(&mut cx, 48 * 60);
    assert!(
        updates.recorded.lock().unwrap().is_empty(),
        "undecided sent nothing"
    );
    assert_no_sockets(before.clone());
    // Off: another 48 hours.
    cx.update(|_, cx| {
        ui_workspace::settings::AppSettings::update(cx, |s| s.updates.set_mode(UpdateMode::Off))
            .unwrap()
    });
    cx.run_until_parked();
    assert_eq!(updates.service.status().mode, UpdateMode::Off);
    idle_minutes(&mut cx, 48 * 60);
    assert!(
        updates.recorded.lock().unwrap().is_empty(),
        "off sent nothing"
    );
    assert_no_sockets(before);
}

#[gpui_kit::test]
fn scenario_app_shell_only_the_update_host_while_updates_are_on(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    std::fs::create_dir_all(dirs.config()).unwrap();
    std::fs::write(
        dirs.store().path(),
        "settings_version = 4\n[updates]\nmode = \"automatic\"\n",
    )
    .unwrap();
    init_app(cx, Arc::new(FakeBackend::small_corpus()), Some(&dirs));
    let updates = updates_recorded(cx);
    let (shell, mut cx) = open(cx);
    wait_loaded(&shell, &mut cx);
    // Nothing in the first minute.
    for _ in 0..59 {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
    }
    assert!(
        updates.recorded.lock().unwrap().is_empty(),
        "nothing in the first 60 s"
    );
    // Ten minutes of app time.
    idle_minutes(&mut cx, 10);
    let requests = updates.recorded.lock().unwrap().clone();
    assert!(!requests.is_empty(), "a scheduled check ran");
    for url in &requests {
        assert_eq!(
            url,
            ps_update::build::MANIFEST_URL,
            "only the manifest address"
        );
    }
}
