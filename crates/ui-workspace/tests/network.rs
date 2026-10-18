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
    // No HTTP client is linked into the shell's own crates (ps-llm alone may hold one, lazily).
    let manifest = include_str!("../Cargo.toml");
    for client in ["reqwest", "ureq", "hyper", "isahc", "surf"] {
        assert!(
            !manifest.contains(&format!("\n{client}")),
            "ui-workspace depends on {client}"
        );
    }
}
