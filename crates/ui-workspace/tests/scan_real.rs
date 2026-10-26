//! Scan dashboard gate (change `scan-dashboard`, task 3.2): live progress, pause, resume and cancel from the
//! dashboard against the real `ps-service` engine, headless.
//!
//! `dashboard_controls_on_the_real_engine` runs on a tiny generated corpus with a simulated slow device.
//! The gate on the medium corpus is opt-in (reads ~5 GB):
//! `cargo run --release -p photosquirrel-cli -- fixtures generate --preset medium --out target/corpus-medium`, then
//! `cargo test -p ui-workspace --test scan_real -- --ignored --exact dashboard_controls_on_the_medium_corpus`.
//! `dashboard_controls_on_the_real_engine` asserts a wall-clock bound, so it is ignored in the main test pass and
//! run alone by `cargo xtask timing` (part of `cargo xtask ci`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_model::{ProgressSnapshot, RunStatus};
use ps_service::scan::{ScanOptions, ScanService};
use support::*;
use ui_workspace::Shell;
use ui_workspace::library_backend::CatalogBackend;
use ui_workspace::scan_backend::ScanBackend as _;
use ui_workspace::views::ViewId;
use ui_workspace::views::scan::ScanHub;

struct Real {
    shell: Entity<Shell>,
    cx: VisualTestContext,
    backend: Arc<CatalogBackend>,
}

impl Real {
    fn snap(&mut self) -> Option<ProgressSnapshot> {
        self.cx
            .update(|_, cx| ScanHub::monitor(cx).and_then(|m| m.read(cx).snapshot.clone()))
    }

    /// Let real time pass for the engine while the fake GPUI clock drives the 100 ms sampling loop.
    fn pump(&mut self, ms: u64) {
        for _ in 0..ms / 10 {
            std::thread::sleep(Duration::from_millis(10));
            self.cx.executor().advance_clock(Duration::from_millis(10));
            self.cx.run_until_parked();
        }
    }

    fn wait(&mut self, what: &str, limit: Duration, mut cond: impl FnMut(&mut Real) -> bool) {
        let start = Instant::now();
        loop {
            self.cx.run_until_parked();
            if cond(self) {
                return;
            }
            assert!(start.elapsed() < limit, "timed out waiting for {what}");
            self.pump(10);
        }
    }

    /// The engine's own status, read directly (not through the dashboard's samples).
    fn engine_status(&self) -> Option<RunStatus> {
        self.backend.scan().ok()?.snapshot().map(|s| s.status)
    }

    /// Click `id`; the status line must then show `want` within 250 ms of real time from the click (the frame
    /// the click lands on is rendered first, outside the bound). `engine` is the run status the engine reports
    /// once it has acted: the time is split into click to engine and engine to label (printed, and in the failure
    /// message), so a recurrence points at the engine or at the dashboard.
    fn control(&mut self, id: &str, want: &str, engine: RunStatus) {
        use gpui_kit::test::TestWindowExt as _;
        let target = gpui_kit::SharedString::from(id.to_owned());
        self.cx.update(|window, cx| window.render_frame(cx));
        let clicked = Instant::now();
        self.cx.update(|window, cx| window.click(target, cx));
        self.cx.run_until_parked();
        let mut engine_at = None;
        loop {
            if engine_at.is_none() && self.engine_status() == Some(engine) {
                engine_at = Some(clicked.elapsed());
            }
            let shown = label("scan-status", &mut self.cx);
            let since = clicked.elapsed();
            let split = format!(
                "click to engine {}, engine to label {}",
                engine_at.map_or("not yet".into(), |t: Duration| format!("{t:?}")),
                engine_at.map_or("-".into(), |t| format!("{:?}", since.saturating_sub(t)))
            );
            assert!(
                since <= ps_fixtures::ci_slack(Duration::from_millis(250)),
                "status {shown:?} is not {want:?} within 250 ms: {since:?} since the click; {split}"
            );
            if shown.as_deref() == Some(want) {
                eprintln!("{want} after {since:?}: {split}");
                return;
            }
            self.pump(10);
        }
    }
}

fn open_real(cx: &mut TestAppContext, data: &Path, roots: &[PathBuf], slow: bool) -> Real {
    let options = ScanOptions {
        read_delay: slow.then_some(Duration::from_millis(40)),
        ..ScanOptions::default()
    };
    let svc = ScanService::open(data, options).unwrap();
    let backend = Arc::new(CatalogBackend::with_service(data, svc));
    for r in roots {
        backend.add_source(r).unwrap();
    }
    init_app(cx, backend.clone(), None);
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    let s = shell.clone();
    vcx.update(|w, cx| s.update(cx, |s, cx| s.show_view(ViewId::Sources, w, cx)));
    vcx.run_until_parked();
    Real {
        shell,
        cx: vcx,
        backend,
    }
}

fn exercise(r: &mut Real, sources: usize, limit: Duration) {
    r.wait("the sources", limit, |r| {
        label("scan-start", &mut r.cx).as_deref() == Some(&format!("Scan {sources} sources"))
    });
    click("scan-start", &mut r.cx);
    // Live progress.
    r.wait("analysis progress", limit, |r| {
        r.snap()
            .is_some_and(|s| s.status == RunStatus::Running && s.processed > 0)
    });
    let total = r.snap().unwrap().to_process;
    let start = label("scan-start", &mut r.cx).unwrap();
    assert!(start.ends_with("(unavailable)"), "{start}");
    let lanes: Vec<i64> = r
        .snap()
        .unwrap()
        .sources
        .iter()
        .map(|s| s.source.0)
        .collect();
    assert_eq!(lanes.len(), sources, "one lane per source");
    // The first lane is on screen (the others are below the fold of the test window).
    let lane = label(&format!("lane-{}", lanes[0]), &mut r.cx).unwrap();
    assert!(
        lane.contains("reader") && lane.contains("files/s"),
        "{lane}"
    );

    // Pause: shown within 250 ms, then counts stop increasing once the readers have drained.
    r.control("scan-pause", "Paused", RunStatus::Paused);
    r.wait("the workers to drain", limit, |r| {
        r.snap().is_some_and(|s| s.active_workers == 0)
    });
    let paused = r.snap().unwrap().processed;
    r.pump(400);
    assert_eq!(
        r.snap().unwrap().processed,
        paused,
        "counts stop increasing"
    );

    // Resume continues the run.
    r.control("scan-resume", "Scanning", RunStatus::Running);
    r.wait("more progress", limit, |r| {
        r.snap().is_some_and(|s| s.processed > paused)
    });

    // Cancel: shown within 250 ms with Resume offered; resuming continues the same run.
    r.control("scan-cancel", "Cancelled", RunStatus::Cancelled);
    assert_eq!(label("scan-resume", &mut r.cx).as_deref(), Some("Resume"));
    let (run, done) = r.snap().map(|s| (s.run, s.processed)).unwrap();
    click("scan-resume", &mut r.cx);
    r.wait("the resumed run", limit, |r| {
        r.snap().is_some_and(|s| s.status == RunStatus::Running)
    });
    // The continuation only has the unfinished files to do: finished ones are not processed again.
    let s = r.snap().unwrap();
    assert_eq!(s.run, run, "the same run continues");
    let resumed_work = s.to_process;

    // Let it finish: the summary appears.
    r.wait("the run summary", limit * 4, |r| {
        exists("run-summary", &mut r.cx)
    });
    let summary = label("run-summary", &mut r.cx).unwrap();
    assert!(summary.starts_with("Scan complete"), "{summary}");
    let end = r.snap().unwrap();
    assert_eq!(end.run, run);
    assert!(
        end.to_process.max(resumed_work) + done <= total,
        "resumed work {} + done {done} exceeds the run's {total} files",
        end.to_process.max(resumed_work)
    );
    let _ = &r.shell;
}

#[gpui_kit::test]
// A wall-clock budget: run alone, after the other tests, by `cargo xtask timing` (part of `cargo xtask ci`).
#[ignore = "timing"]
fn dashboard_controls_on_the_real_engine(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("corpus");
    let truth = ps_fixtures::generate_with(
        7,
        ps_fixtures::Preset::Small,
        ps_fixtures::Params {
            standalone: 40,
            sets_per_kind: 3,
            burst_sets: 3,
            converted_raw_bursts: 0,
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
        },
        &ps_fixtures::DirWriter::new(&root).unwrap(),
    )
    .unwrap();
    let roots: Vec<PathBuf> = truth.drives.iter().map(|d| root.join(d)).collect();
    let mut r = open_real(cx, &tmp.path().join("data"), &roots, true);
    exercise(&mut r, roots.len(), Duration::from_secs(60));
}

/// The milestone gate on the medium corpus (opt-in; see the module docs).
#[gpui_kit::test]
#[ignore = "reads the ~5 GB medium corpus from target/corpus-medium"]
fn dashboard_controls_on_the_medium_corpus(cx: &mut TestAppContext) {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-medium");
    assert!(
        corpus.join("truth.json").is_file(),
        "generate the medium corpus first (module docs)"
    );
    let roots: Vec<PathBuf> = ["drive-a", "drive-b", "drive-c"]
        .iter()
        .map(|d| corpus.join(d))
        .collect();
    let tmp = tempfile::tempdir().unwrap();
    let mut r = open_real(cx, &tmp.path().join("data"), &roots, false);
    exercise(&mut r, roots.len(), Duration::from_secs(300));
}
