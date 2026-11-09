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
    /// `activity()` calls (the shell's activity sampling).
    pub activity_calls: AtomicU32,
    /// What `activity()` reports (idle when `None`).
    pub activity: Mutex<Option<ActivitySnapshot>>,
    /// The gallery's data (an empty library when `None`).
    pub gallery: Option<Arc<dyn ui_gallery::source::GallerySource>>,
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
        self.activity_calls.fetch_add(1, Ordering::SeqCst);
        self.activity.lock().unwrap().clone()
    }
    fn gallery(self: Arc<Self>) -> Arc<dyn ui_gallery::source::GallerySource> {
        self.gallery
            .clone()
            .unwrap_or_else(|| Arc::new(ui_gallery::source::EmptySource))
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

// ---- updates ------------------------------------------------------------------------------------------------

/// The update service of a test: its update folder, and the server (or recorder) it talks to.
pub struct Updates {
    pub _tmp: tempfile::TempDir,
    pub dir: PathBuf,
    pub service: ps_service::updates::UpdateService,
    pub server: Option<ps_update::testing::TestServer>,
    /// URLs requested through the recording transport.
    pub recorded: Arc<Mutex<Vec<String>>>,
}

impl Updates {
    pub fn state(&self) -> ps_update::UpdateState {
        ps_update::UpdateState::load(&ps_update::UpdateDir::at(&self.dir))
    }

    pub fn manifest_requests(&self) -> usize {
        self.server.as_ref().map_or(0, |s| {
            s.requests()
                .iter()
                .filter(|r| r.path == "/update-manifest.json")
                .count()
        })
    }
}

/// Records every request and answers none (no socket is opened).
struct Recorder(Arc<Mutex<Vec<String>>>);

impl ps_update::transport::Transport for Recorder {
    fn open(
        &self,
        _: &ps_update::transport::Endpoint,
        url: &str,
        _: Option<u64>,
    ) -> Result<ps_update::transport::Opened, ps_update::UpdateError> {
        self.0.lock().unwrap().push(url.to_owned());
        Err(ps_update::UpdateError::Failed("offline (recorded)".into()))
    }
}

/// A per-user AppImage installation in `root` (it installs updates itself).
pub fn appimage(root: &std::path::Path) -> ps_service::updates::Installation {
    ps_service::updates::Installation {
        kind: ps_service::updates::Kind::AppImage,
        location: root.join("PhotoSquirrel.AppImage"),
        cannot_install: None,
    }
}

fn install_hub(
    cx: &mut TestAppContext,
    updater: ps_update::check::Updater,
    installation: ps_service::updates::Installation,
) -> ps_service::updates::UpdateService {
    let cell = ps_service::updates::ModeCell::new(ps_model::settings::UpdateMode::Undecided);
    let service = ps_service::updates::UpdateService::new(
        updater,
        installation,
        cell.source(),
        ps_update::now_secs(),
        None,
    );
    let s = service.clone();
    cx.update(|cx| ui_workspace::updates_ui::install(s, cell, cx));
    service
}

/// Updates against GitHub's address through a recording transport (call after `init_app`, before `open`).
pub fn updates_recorded(cx: &mut TestAppContext) -> Updates {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("updates");
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let updater = ps_update::check::Updater {
        dir: ps_update::UpdateDir::at(&dir),
        transport: Arc::new(Recorder(recorded.clone())),
        endpoint: ps_update::transport::Endpoint::GitHub,
        installed: ps_update::build::current_version(),
        release_build: true,
        keys: ps_update::keys::trusted(),
        platform: "linux-x86_64".into(),
        free_space: Arc::new(|_| Some(u64::MAX)),
    };
    let service = install_hub(cx, updater, appimage(tmp.path()));
    Updates {
        _tmp: tmp,
        dir,
        service,
        server: None,
        recorded,
    }
}

/// Updates from a loopback server offering `version` (as 0.4.0 installed) with an AppImage, for the
/// installation `kind` makes from the temp folder (call after `init_app`, before `open`).
pub fn updates_offering(
    cx: &mut TestAppContext,
    version: &str,
    urgent: bool,
    installation: impl FnOnce(&std::path::Path) -> ps_service::updates::Installation,
) -> Updates {
    use ps_update::testing::{Route, TestServer, envelope, sample_manifest, updater};
    let server = TestServer::start();
    let payload = format!("the {version} package").into_bytes();
    let mut m = sample_manifest(
        version,
        &[
            ("linux-x86_64", "appimage", payload.as_slice()),
            ("linux-x86_64", "deb", payload.as_slice()),
        ],
    );
    m.urgent = urgent;
    m.notes = format!("- Faster scans\n- Clearer duplicate review in {version}");
    server.route(
        "/update-manifest.json",
        Route::Body(envelope(&m).into_bytes()),
    );
    for kind in ["appimage", "deb"] {
        let name = m.packages["linux-x86_64"][kind].name.clone();
        server.route(&format!("/{name}"), Route::Body(payload.clone()));
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("updates");
    let inst = installation(tmp.path());
    let service = install_hub(cx, updater(&server, &dir, "0.4.0"), inst);
    Updates {
        _tmp: tmp,
        dir,
        service,
        server: Some(server),
        recorded: Arc::default(),
    }
}

/// Write `settings.toml` in `dirs` (before `init_app`).
pub fn write_settings(dirs: &Dirs, text: &str) {
    std::fs::create_dir_all(dirs.config()).unwrap();
    std::fs::write(dirs.store().path(), text).unwrap();
}

/// Run `n` update ticks of app time (one minute each).
pub fn idle_minutes(cx: &mut VisualTestContext, n: u32) {
    for _ in 0..n {
        cx.executor().advance_clock(ps_service::updates::TICK);
        cx.run_until_parked();
    }
}
