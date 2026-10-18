//! Helpers shared by the UI test files: app set-up, a fake engine, waits and element queries.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, SharedString, TestAppContext, VisualTestContext,
    WindowOptions, px, size,
};
use ps_model::ScanRunId;
use ps_service::settings::SettingsStore;
use ui_workspace::service::{ActivitySnapshot, CatalogSummary, InterruptedRun, LibraryBackend};
use ui_workspace::{Shell, ShellConfig};

pub const WINDOW: [f32; 2] = [1440.0, 900.0];

/// A scriptable engine.
#[derive(Default)]
pub struct FakeBackend {
    pub summary: Mutex<CatalogSummary>,
    pub interrupted: Mutex<Option<InterruptedRun>>,
    /// Every `summary()` call blocks this long (the service thread, not the UI).
    pub summary_delay: Mutex<Duration>,
    pub resumed: Mutex<Vec<ScanRunId>>,
    pub rescans: AtomicU32,
}

impl FakeBackend {
    /// A scanned small corpus.
    pub fn small_corpus() -> Self {
        let b = Self::default();
        *b.summary.lock().unwrap() = CatalogSummary {
            sources: 2,
            photos: 248,
            videos: 31,
            duplicate_groups: 57,
            excluded: 112,
            bytes: 1_900_000_000,
        };
        b
    }
}

impl LibraryBackend for FakeBackend {
    fn summary(&self) -> anyhow::Result<CatalogSummary> {
        let delay = *self.summary_delay.lock().unwrap();
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        Ok(self.summary.lock().unwrap().clone())
    }
    fn interrupted_run(&self) -> anyhow::Result<Option<InterruptedRun>> {
        Ok(self.interrupted.lock().unwrap().clone())
    }
    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()> {
        self.resumed.lock().unwrap().push(run);
        *self.interrupted.lock().unwrap() = None;
        Ok(())
    }
    fn rescan(&self) -> anyhow::Result<()> {
        self.rescans.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn add_source(&self, _: PathBuf) -> anyhow::Result<()> {
        Ok(())
    }
    fn activity(&self) -> Option<ActivitySnapshot> {
        None
    }
}

/// Per-test directories (settings + data), removed on drop.
pub struct Dirs {
    pub root: tempfile::TempDir,
}

impl Dirs {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }
    pub fn data(&self) -> PathBuf {
        self.root.path().join("data")
    }
    pub fn config(&self) -> PathBuf {
        self.root.path().join("config")
    }
    pub fn store(&self) -> SettingsStore {
        SettingsStore::in_dir(self.config())
    }
}

pub fn init_app(cx: &mut TestAppContext, backend: Arc<dyn LibraryBackend>, dirs: Option<&Dirs>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ui_workspace::init(
            ShellConfig {
                settings: dirs.map(Dirs::store),
                theme_override: None,
                data_dir: dirs.map(Dirs::data),
                backend,
            },
            cx,
        );
    });
}

/// Open the shell in a window wrapped in gpui-kit's Root.
pub fn open(cx: &mut TestAppContext) -> (Entity<Shell>, VisualTestContext) {
    let (handle, shell): (AnyWindowHandle, Entity<Shell>) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let shell = cx.new(|cx| Shell::new(window, cx));
                window.focus(&shell.read(cx).focus_handle().clone(), cx);
                shell
            })
        })
        .unwrap();
    let vcx = VisualTestContext::from_window(handle, cx);
    vcx.simulate_resize(size(px(WINDOW[0]), px(WINDOW[1])));
    vcx.run_until_parked();
    (shell, vcx)
}

/// Run until `cond` holds (the service thread answers from another thread), or panic after 5 s.
pub fn wait_until(
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
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
        // Replies from the service thread are polled on (fake-clock) timers.
        cx.executor().advance_clock(Duration::from_millis(10));
    }
}

/// Wait for the catalog summary to arrive.
pub fn wait_loaded(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let shell = shell.clone();
    wait_until(cx, "the catalog summary", move |cx| {
        cx.update(|_, cx| !shell.read(cx).summary().is_loading())
    });
}

pub fn exists(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = SharedString::from(id.to_owned());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|s| s.visible())
    })
}

pub fn label(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = SharedString::from(id.to_owned());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|s| s.label().map(|l| l.to_string()))
    })
}

pub fn click(id: &str, cx: &mut VisualTestContext) {
    let id = SharedString::from(id.to_owned());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
}

pub fn bounds(id: &str, cx: &mut VisualTestContext) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let id = SharedString::from(id.to_owned());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find(id).bounds()
    })
}
