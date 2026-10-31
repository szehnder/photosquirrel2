//! Headless UI tests for the scan dashboard (change `scan-dashboard`, capability `scan-progress`) on the
//! simulated engine (`ps_service::scans::FakeScanService` behind `DemoBackend`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use ps_model::{DeviceClass, RunStatus, ScanRunId, SourceId, StageCounts};
use ps_service::scans::{FakeConfig, FakeScanService, FakeSource};
use support::*;
use ui_gallery::source::{GallerySource, SharedSource, ThumbKey};
use ui_workspace::Shell;
use ui_workspace::scan_backend::{DemoBackend, DemoThumbs};
use ui_workspace::views::ViewId;
use ui_workspace::views::scan::{ScanHub, ScanMonitor};

/// A gallery whose every thumbnail is a small JPEG.
struct JpegThumbs(Vec<u8>);

impl JpegThumbs {
    fn new() -> Self {
        let img = image::RgbImage::from_fn(96, 72, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 3) as u8, 90])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&img)
            .unwrap();
        Self(out)
    }
}

impl GallerySource for JpegThumbs {
    fn id_list(&self, q: &ui_gallery::source::GalleryQuery) -> anyhow::Result<ps_model::IdList> {
        ui_gallery::source::EmptySource.id_list(q)
    }
    fn summaries(
        &self,
        _: &[ps_model::ItemId],
    ) -> anyhow::Result<Vec<ui_gallery::source::ItemSummary>> {
        Ok(Vec::new())
    }
    fn thumbnail(&self, _: &ThumbKey) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(Some(self.0.clone()))
    }
    fn preview(&self, id: ps_model::ItemId) -> anyhow::Result<ui_gallery::source::Preview> {
        ui_gallery::source::EmptySource.preview(id)
    }
    fn stack_members(&self, k: ps_model::ItemId) -> anyhow::Result<Vec<ps_model::ItemId>> {
        Ok(vec![k])
    }
    fn detail(
        &self,
        _: ps_model::ItemId,
    ) -> anyhow::Result<Option<ui_gallery::source::ItemDetail>> {
        Ok(None)
    }
    fn set_decision(
        &self,
        _: ps_model::ItemId,
        _: Option<ui_gallery::source::Decision>,
        _: Option<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn camera_models(&self) -> anyhow::Result<Vec<(String, u64)>> {
        Ok(Vec::new())
    }
}

struct Rig {
    shell: Entity<Shell>,
    cx: VisualTestContext,
    backend: Arc<DemoBackend>,
    /// The shell's own engine (its activity calls show the shell sampling).
    shell_backend: Arc<FakeBackend>,
    _dirs: Option<Dirs>,
}

impl Rig {
    fn monitor(&mut self) -> Entity<ScanMonitor> {
        self.cx.update(|_, cx| ScanHub::monitor(cx).unwrap())
    }

    fn snap<T>(&mut self, f: impl FnOnce(&ps_model::ProgressSnapshot) -> T) -> Option<T> {
        let m = self.monitor();
        self.cx.update(|_, cx| m.read(cx).snapshot.as_ref().map(f))
    }

    fn show(&mut self, view: ViewId) {
        let shell = self.shell.clone();
        self.cx
            .update(|w, cx| shell.update(cx, |s, cx| s.show_view(view, w, cx)));
        self.cx.run_until_parked();
    }

    /// Advance the fake clock by `ms` in 10 ms steps (the sampling loop runs on GPUI timers).
    fn tick(&mut self, ms: u64) {
        for _ in 0..ms / 10 {
            self.cx.executor().advance_clock(Duration::from_millis(10));
            self.cx.run_until_parked();
        }
    }

    fn wait(&mut self, what: &str, mut cond: impl FnMut(&mut Rig) -> bool) {
        let start = Instant::now();
        loop {
            self.cx.run_until_parked();
            if cond(self) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "timed out waiting for {what}"
            );
            self.tick(20);
        }
    }
}

fn rig_with(cx: &mut TestAppContext, config: FakeConfig, gallery: SharedSource) -> Rig {
    rig_backend(
        cx,
        Arc::new(DemoBackend::new(FakeScanService::new(config))),
        gallery,
    )
}

fn rig_backend(cx: &mut TestAppContext, backend: Arc<DemoBackend>, gallery: SharedSource) -> Rig {
    let rig = rig_full(cx, backend, gallery, FakeBackend::small_corpus(), None);
    let mut rig = rig;
    rig.show(ViewId::Sources);
    rig
}

/// A rig with a given shell engine and settings directory (no view switch).
fn rig_full(
    cx: &mut TestAppContext,
    backend: Arc<DemoBackend>,
    gallery: SharedSource,
    shell_backend: FakeBackend,
    dirs: Option<Dirs>,
) -> Rig {
    let shell_backend = Arc::new(shell_backend);
    init_app(cx, shell_backend.clone(), dirs.as_ref());
    let b = backend.clone();
    cx.update(move |cx| ui_workspace::views::scan::install(b, gallery, cx));
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    Rig {
        shell,
        cx: vcx,
        backend,
        shell_backend,
        _dirs: dirs,
    }
}

fn rig(cx: &mut TestAppContext, config: FakeConfig) -> Rig {
    rig_with(cx, config, Arc::new(ui_gallery::source::EmptySource))
}

fn three_sources(fps: f64) -> FakeConfig {
    let mut c = FakeConfig::two_sources(fps);
    c.sources.push(FakeSource::new(
        3,
        "USB · Backup 2014",
        DeviceClass::Rotational,
        9_000,
    ));
    c
}

fn start_all(r: &mut Rig) {
    click("scan-start", &mut r.cx);
    r.wait("the run to start", |r| {
        r.snap(|s| s.status) == Some(RunStatus::Running)
    });
}

#[gpui_kit::test]
fn scenario_scan_progress_start_scan_from_dashboard(cx: &mut TestAppContext) {
    let mut r = rig(cx, three_sources(500.0));
    r.wait("sources", |r| label("source-row-3", &mut r.cx).is_some());
    assert_eq!(
        label("scan-start", &mut r.cx).as_deref(),
        Some("Scan 3 sources")
    );
    // Pick two: drop the third.
    click("source-select-3", &mut r.cx);
    assert_eq!(
        label("scan-start", &mut r.cx).as_deref(),
        Some("Scan 2 sources")
    );
    click("scan-start", &mut r.cx);
    r.wait("the run", |r| {
        r.snap(|s| s.status) == Some(RunStatus::Running)
    });
    assert_eq!(
        r.backend.started.lock().unwrap().clone(),
        vec![vec![SourceId(1), SourceId(2)]]
    );
    let sources = r
        .snap(|s| s.sources.iter().map(|x| x.source).collect::<Vec<_>>())
        .unwrap();
    assert_eq!(sources, vec![SourceId(1), SourceId(2)]);
    let start = label("scan-start", &mut r.cx).unwrap();
    assert!(start.ends_with("(unavailable)"), "{start}");
    // Clicking it does nothing while the run is active.
    click("scan-start", &mut r.cx);
    assert_eq!(r.backend.started.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
fn scenario_scan_progress_rescan_diff_shown(cx: &mut TestAppContext) {
    let mut c = FakeConfig::two_sources(2.0);
    c.sources = vec![FakeSource::new(1, "Pictures", DeviceClass::SolidState, 100).rescan(3, 0, 1)];
    let mut r = rig(cx, c);
    start_all(&mut r);
    r.wait("the diff", |r| {
        label("diff-1", &mut r.cx).is_some_and(|l| l.starts_with("new 3"))
    });
    let processed = r.snap(|s| (s.processed, s.stages.analyzed)).unwrap();
    assert_eq!(
        processed.0, 0,
        "the diff shows before any analysis count increases"
    );
    assert_eq!(
        label("diff-1", &mut r.cx).as_deref(),
        Some("new 3 · changed 0 · unchanged 97 · missing 1")
    );
    assert_eq!(label("diff-new", &mut r.cx).as_deref(), Some("new 3"));
    assert_eq!(
        label("diff-missing", &mut r.cx).as_deref(),
        Some("missing 1")
    );
}

#[gpui_kit::test]
fn scenario_scan_progress_stage_counts_shown(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(100.0));
    let fake = FakeScanService::new(FakeConfig::two_sources(100.0));
    fake.start_at(&[], 5_000, RunStatus::Running);
    let mut snap = fake.peek().unwrap();
    snap.stages = StageCounts {
        discovered: 1_000,
        probed: 600,
        analyzed: 400,
        thumbed: 300,
        skipped: 150,
        failed: 5,
    };
    *r.backend.pinned.lock().unwrap() = Some(snap);
    let m = r.monitor();
    r.cx.update(|_, cx| m.update(cx, |m, cx| m.ensure_sampling(cx)));
    r.wait("the snapshot", |r| {
        r.snap(|s| s.stages.discovered) == Some(1_000)
    });
    for (id, want) in [
        ("stage-discovered", "Waiting 1000"),
        ("stage-probed", "Probed 600"),
        ("stage-analyzed", "Analyzed 400"),
        ("stage-thumbed", "Done 300"),
        ("stage-skipped", "Skipped 150"),
        ("stage-failed", "Failed 5"),
    ] {
        assert_eq!(label(id, &mut r.cx).as_deref(), Some(want), "{id}");
    }
}

#[gpui_kit::test]
fn scenario_scan_progress_per_source_rows_shown(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(400.0));
    start_all(&mut r);
    r.tick(2_500);
    for (id, readers) in [(1, "SSD · 8 readers"), (2, "Network · 2 readers")] {
        let lane = label(&format!("lane-{id}"), &mut r.cx).unwrap();
        assert!(lane.contains(readers), "{lane}");
        assert!(lane.contains("files/s"), "{lane}");
        assert!(lane.contains("/s"), "throughput in {lane}");
        let dirs = label(&format!("lane-{id}-dirs"), &mut r.cx).unwrap();
        assert!(!dirs.is_empty(), "current folders of source {id}");
    }
    // Per-lane MB/s has samples once a second of run time has passed.
    let m = r.monitor();
    let rate = r.cx.update(|_, cx| {
        m.read(cx)
            .lanes
            .get(&SourceId(1))
            .map(|l| l.current_bytes_per_s())
    });
    assert!(rate.unwrap_or(0.0) > 0.0);
}

#[gpui_kit::test]
fn scenario_scan_progress_recent_finds_show_thumbnails(cx: &mut TestAppContext) {
    let mut c = FakeConfig::two_sources(200.0);
    c.skip_percent = 0;
    c.walk_ms = 0;
    let mut r = rig_with(cx, c, Arc::new(JpegThumbs::new()));
    start_all(&mut r);
    // 100 photos found.
    r.wait("100 photos", |r| {
        r.snap(|s| s.stages.thumbed).unwrap_or(0) >= 100
    });
    assert_eq!(r.snap(|s| s.recent_photos.len()), Some(64));
    let m = r.monitor();
    r.wait("64 thumbnails", |r| {
        r.cx.update(|_, cx| m.read(cx).strip.read(cx).loaded()) == 64
    });
    assert_eq!(
        label("recent-finds", &mut r.cx).as_deref(),
        Some("64 recent photos")
    );
    for i in 0..64 {
        let l = label(&format!("recent-find-{i}"), &mut r.cx);
        assert!(l.is_some(), "tile {i}");
    }
    assert!(label("recent-find-64", &mut r.cx).is_none());
}

#[gpui_kit::test]
fn scenario_scan_progress_errors_listed_with_reason(cx: &mut TestAppContext) {
    let mut c = FakeConfig::two_sources(500.0);
    c.error_every = 100;
    let mut r = rig(cx, c);
    start_all(&mut r);
    r.wait("an error", |r| r.snap(|s| s.errors).unwrap_or(0) > 0);
    let e = label("error-0", &mut r.cx).unwrap();
    assert!(e.contains("Private/locked/"), "{e}");
    assert!(e.contains("Permission denied"), "{e}");
}

fn wait_status(r: &mut Rig, want: &str, within_ms: u64) {
    let mut waited = 0;
    loop {
        if label("scan-status", &mut r.cx).as_deref() == Some(want) {
            return;
        }
        assert!(
            waited <= within_ms,
            "status {:?} is not {want:?} within {within_ms} ms",
            label("scan-status", &mut r.cx)
        );
        r.tick(10);
        waited += 10;
    }
}

#[gpui_kit::test]
fn scenario_scan_progress_pause_from_dashboard(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(500.0));
    start_all(&mut r);
    r.tick(1_500);
    click("scan-pause", &mut r.cx);
    wait_status(&mut r, "Paused", 250);
    let before = r.snap(|s| s.processed).unwrap();
    r.tick(1_000);
    assert_eq!(
        r.snap(|s| s.processed).unwrap(),
        before,
        "counts stop increasing"
    );
}

#[gpui_kit::test]
fn scenario_scan_progress_cancel_from_dashboard(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(500.0));
    start_all(&mut r);
    r.tick(1_000);
    click("scan-cancel", &mut r.cx);
    wait_status(&mut r, "Cancelled", 250);
    assert_eq!(label("scan-resume", &mut r.cx).as_deref(), Some("Resume"));
}

/// The engine needs 30 ms to stop a cancelled run and the next periodic sample is 100 ms away: the dashboard
/// shows Cancelled as soon as the run has stopped, with the GPUI clock not advanced at all.
#[gpui_kit::test]
fn scenario_scan_progress_cancelled_shown_when_the_run_stops(cx: &mut TestAppContext) {
    let mut config = FakeConfig::two_sources(500.0);
    config.cancel_ms = 30;
    let mut r = rig(cx, config);
    start_all(&mut r);
    r.tick(1_000);
    let before = r.backend.scan.snapshots_taken();
    click("scan-cancel", &mut r.cx);
    assert_eq!(
        label("scan-status", &mut r.cx).as_deref(),
        Some("Cancelled"),
        "not shown when the run stopped"
    );
    assert_eq!(label("scan-resume", &mut r.cx).as_deref(), Some("Resume"));
    // Only the sample the cancel itself takes: no periodic sample happened (the clock did not move).
    assert_eq!(r.backend.scan.snapshots_taken() - before, 1);
    // Without waiting for the engine the dashboard would still read the running run.
    let fake = FakeScanService::new({
        let mut c = FakeConfig::two_sources(500.0);
        c.cancel_ms = 30;
        c
    });
    fake.start(&[]).unwrap();
    fake.cancel().unwrap();
    assert_eq!(fake.peek().unwrap().status, RunStatus::Running);
    assert!(fake.wait_stopped(100));
    assert_eq!(fake.peek().unwrap().status, RunStatus::Cancelled);
}

#[gpui_kit::test]
fn scenario_scan_progress_resume_from_dashboard(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(500.0));
    start_all(&mut r);
    r.tick(2_000);
    click("scan-cancel", &mut r.cx);
    wait_status(&mut r, "Cancelled", 250);
    let (run, done) = r.snap(|s| (s.run, s.processed)).unwrap();
    assert!(done > 0);
    click("scan-resume", &mut r.cx);
    wait_status(&mut r, "Scanning", 250);
    r.tick(500);
    let (run2, done2) = r.snap(|s| (s.run, s.processed)).unwrap();
    assert_eq!(run2, run, "the same run continues");
    assert!(
        done2 > done,
        "it continues from {done}, not from zero ({done2})"
    );
    assert_eq!(r.backend.started.lock().unwrap().len(), 1, "no new run");
    let _ = ScanRunId(0);
}

/// After a restart the interrupted run is offered for resuming, and resuming continues it.
#[gpui_kit::test]
fn interrupted_run_offered_for_resume_after_restart(cx: &mut TestAppContext) {
    let fake = FakeScanService::new(FakeConfig::two_sources(500.0));
    let run = fake.start_at(&[], 3_000, RunStatus::Interrupted);
    let done = fake.peek().unwrap().processed;
    assert!(done > 0);
    let backend = Arc::new(DemoBackend::new(fake));
    backend
        .restarted
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut r = rig_backend(cx, backend, Arc::new(ui_gallery::source::EmptySource));
    r.wait("the interrupted run", |r| {
        label("run-summary", &mut r.cx).is_some_and(|l| l.contains("interrupted"))
    });
    assert!(label("scan-status", &mut r.cx).is_none(), "no live run yet");
    click("scan-resume-last", &mut r.cx);
    wait_status(&mut r, "Scanning", 250);
    r.tick(300);
    let (run2, done2) = r.snap(|s| (s.run, s.processed)).unwrap();
    assert_eq!(run2, run);
    assert!(done2 > done, "continues from {done} ({done2})");
    assert!(r.backend.started.lock().unwrap().is_empty(), "no new run");
}

#[gpui_kit::test]
fn scenario_scan_progress_snapshot_sampled_at_most_10_hz(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(100.0));
    start_all(&mut r);
    let before = r.backend.scan.snapshots_taken();
    r.tick(5_000);
    let taken = r.backend.scan.snapshots_taken() - before;
    assert!(taken <= 51, "{taken} snapshots in 5 s");
    assert!(taken >= 40, "the dashboard keeps sampling ({taken})");
}

/// The calling thread's CPU time, where the platform reports it per thread.
fn thread_cpu_time() -> Option<Duration> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        Some(Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// One dashboard frame: wall time and the rendering thread's CPU time (wall time where there is no per-thread
/// clock).
#[derive(Clone, Copy, Debug)]
struct Frame {
    wall: Duration,
    cpu: Duration,
}

/// Let the live sampling loop take `frames` snapshots of the running scan (one per 100 ms of the fake clock) and
/// time the dashboard frame that renders each.
fn frame_times(r: &mut Rig, frames: usize) -> Vec<Frame> {
    let mut times = Vec::with_capacity(frames);
    for _ in 0..frames {
        r.tick(100);
        let t = r.cx.update(|window, cx| {
            let (t0, c0) = (Instant::now(), thread_cpu_time());
            window.refresh();
            window.render_frame(cx);
            let wall = t0.elapsed();
            let cpu = c0
                .zip(thread_cpu_time())
                .map_or(wall, |(c0, c1)| c1.saturating_sub(c0));
            Frame { wall, cpu }
        });
        times.push(t);
    }
    times
}

fn p95(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[(times.len() * 95) / 100]
}

#[gpui_kit::test]
// A wall-clock budget: run alone, after the other tests, by `cargo xtask timing` (part of `cargo xtask ci`).
#[ignore = "timing"]
fn scenario_scan_progress_ui_cost_independent_of_rate(cx: &mut TestAppContext) {
    // Two apps, each with a live run on the simulated engine sampled by the dashboard's own loop.
    let mut slow_app = cx.new_app();
    let mut fast_app = cx.new_app();
    // Large sources, so the fast run is still going after ~41 s of sampling (the layout is the same at both rates).
    let big = |fps: f64| {
        let mut c = FakeConfig::two_sources(fps);
        for s in &mut c.sources {
            s.files = 1_000_000;
        }
        c
    };
    let mut slow_rig = rig(&mut slow_app, big(100.0));
    let mut fast_rig = rig(&mut fast_app, big(20_000.0));
    start_all(&mut slow_rig);
    start_all(&mut fast_rig);
    let taken = |r: &Rig| r.backend.scan.snapshots_taken();
    let done = |r: &mut Rig| r.snap(|s| s.processed).unwrap_or(0);
    let (slow0, fast0) = (taken(&slow_rig), taken(&fast_rig));
    let (slow_done0, fast_done0) = (done(&mut slow_rig), done(&mut fast_rig));
    // Alternate the two rates frame by frame (and which goes first, so neither always renders right after the
    // other app), so a slow period of the machine (other tests running in parallel) hits both series alike; the
    // first 10 pairs warm up layout and text caches. 400 frames per rate: on a loaded machine single frames
    // vary a lot, and the p95 of a short series moves with which frames a slow period happened to hit.
    let (mut slow, mut fast) = (Vec::new(), Vec::new());
    for i in 0..410 {
        let (s, f) = if i % 2 == 0 {
            let s = frame_times(&mut slow_rig, 1);
            (s, frame_times(&mut fast_rig, 1))
        } else {
            let f = frame_times(&mut fast_rig, 1);
            (frame_times(&mut slow_rig, 1), f)
        };
        if i >= 10 {
            slow.extend(s);
            fast.extend(f);
        }
    }
    // The loop really sampled both runs and the shown progress moved at very different rates.
    assert!(taken(&slow_rig) - slow0 >= 100, "slow run sampled");
    assert!(taken(&fast_rig) - fast0 >= 100, "fast run sampled");
    let slow_moved = done(&mut slow_rig) - slow_done0;
    let fast_moved = done(&mut fast_rig) - fast_done0;
    assert!(
        fast_moved > slow_moved * 20,
        "progress {slow_moved} vs {fast_moved} files"
    );
    // Frame time is the rendering thread's CPU time (scan-progress "Constant UI cost"): time the thread spends
    // preempted by other processes is not the dashboard's cost. Wall time is printed beside it.
    let wall = |v: &[Frame]| p95(v.iter().map(|f| f.wall).collect());
    let cpu = |v: &[Frame]| p95(v.iter().map(|f| f.cpu).collect());
    eprintln!(
        "dashboard frame p95 (render-thread CPU): {:?} at 100 files/s, {:?} at 20,000 files/s; wall {:?} / {:?}",
        cpu(&slow),
        cpu(&fast),
        wall(&slow),
        wall(&fast)
    );
    let (slow, fast) = (cpu(&slow), cpu(&fast));
    let diff = slow.abs_diff(fast);
    assert!(
        diff < ps_fixtures::ci_slack(Duration::from_millis(1)),
        "p95 differs by {diff:?}"
    );
    assert!(
        slow.max(fast) <= ps_fixtures::ci_slack(Duration::from_micros(8_300)),
        "p95 {:?} over 8.3 ms",
        slow.max(fast)
    );
}

#[gpui_kit::test]
fn scenario_scan_progress_status_bar_shows_progress(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(500.0));
    start_all(&mut r);
    r.tick(1_000);
    r.show(ViewId::Library);
    let status = label("status-activity", &mut r.cx).unwrap();
    assert!(status.starts_with("Scanning"), "{status}");
    assert!(status.contains('%'), "{status}");
    assert!(
        status.contains("estimating") || status.contains("left"),
        "{status}"
    );
    click("status-activity", &mut r.cx);
    r.cx.run_until_parked();
    let shell = r.shell.clone();
    assert_eq!(
        r.cx.update(|_, cx| shell.read(cx).active_view()),
        ViewId::Sources
    );
}

#[gpui_kit::test]
fn demo_thumbs_map_synthetic_keys_to_real_thumbnails(_cx: &mut TestAppContext) {
    let inner: SharedSource = Arc::new(JpegThumbs::new());
    let t = DemoThumbs {
        inner,
        real: vec![ThumbKey([7; 32])],
    };
    let mut k = [0u8; 32];
    k[8] = 0xfa;
    assert!(t.thumbnail(&ThumbKey(k)).unwrap().is_some());
}

// ---- responsive-add-source -------------------------------------------------------------------------------------

/// One small SSD source of 2,000 files at 4,000 files/s: a run takes about a second of simulated time.
fn one_small_source() -> FakeConfig {
    let mut c = FakeConfig::two_sources(4_000.0);
    c.sources = vec![FakeSource::new(
        1,
        "Macintosh HD · Pictures",
        DeviceClass::SolidState,
        2_000,
    )];
    c
}

fn holiday() -> std::path::PathBuf {
    std::path::PathBuf::from("/demo/Holiday 2024")
}

impl Rig {
    /// Answer an open folder picker with `path`, then wait until the add has answered.
    fn pick(&mut self, path: std::path::PathBuf) {
        assert!(self.cx.did_prompt_for_paths(), "a folder picker is open");
        self.cx
            .simulate_path_prompt_response(move |_| Some(vec![path]));
        let m = self.monitor();
        self.wait("the add to answer", |r| {
            r.cx.update(|_, cx| {
                let m = m.read(cx);
                !m.picking && m.adding.is_empty()
            })
        });
    }

    fn started(&self) -> Vec<Vec<SourceId>> {
        self.backend.started.lock().unwrap().clone()
    }

    fn add_from_shell(&mut self) {
        let shell = self.shell.clone();
        self.cx
            .update(|w, cx| shell.update(cx, |s, cx| s.add_source(w, cx)));
        self.cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn scenario_scan_progress_pending_row_shown_at_once(cx: &mut TestAppContext) {
    let mut r = rig(cx, one_small_source());
    *r.backend.add_delay.lock().unwrap() = Duration::from_millis(300);
    click("source-add", &mut r.cx);
    assert!(r.cx.did_prompt_for_paths());
    assert_eq!(
        label("source-add", &mut r.cx).as_deref(),
        Some("Adding… (unavailable)"),
        "Add is unavailable while the picker is open"
    );
    r.cx.simulate_path_prompt_response(|_| Some(vec![holiday()]));
    // Step the executor until the picker's answer is handled; the add itself (300 ms) has not run yet.
    let m = r.monitor();
    for _ in 0..200 {
        if r.cx.update(|_, cx| !m.read(cx).adding.is_empty()) {
            break;
        }
        r.cx.executor().tick();
    }
    assert!(
        r.backend.added.lock().unwrap().is_empty(),
        "the pending row comes before the add"
    );
    // The very next frame draws it.
    let t = Instant::now();
    let row = label("source-pending-1", &mut r.cx).expect("pending row");
    assert!(
        t.elapsed() < Duration::from_millis(250),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(row, "Holiday 2024: Adding…");
    assert_eq!(
        label("status-activity", &mut r.cx).as_deref(),
        Some("Adding Holiday 2024…")
    );
    assert_eq!(
        label("source-add", &mut r.cx).as_deref(),
        Some("Adding… (unavailable)")
    );
    // The add answers (after its delay): the pending row is the source's row, and its scan starts.
    r.wait("the add", |r| {
        r.cx.update(|_, cx| m.read(cx).adding.is_empty())
    });
    assert!(!exists("source-pending-1", &mut r.cx));
    // The new row may be below the fold of the test window once the run card shows: read the model behind it.
    assert!(r.cx.update(|_, cx| {
        m.read(cx).sources.ready().is_some_and(|rows| {
            rows.iter()
                .any(|row| row.id == SourceId(2) && row.name == "Holiday 2024")
        })
    }));
    assert_eq!(r.started(), vec![vec![SourceId(2)]]);
    assert_eq!(
        label("source-add", &mut r.cx).as_deref(),
        Some("Add source…")
    );
}

#[gpui_kit::test]
fn scenario_scan_progress_status_bar_follows_a_scan_started_by_add_source(cx: &mut TestAppContext) {
    let mut r = rig(cx, one_small_source());
    r.show(ViewId::Library);
    let calls_before = r
        .shell_backend
        .activity_calls
        .load(std::sync::atomic::Ordering::SeqCst);
    r.add_from_shell();
    r.pick(holiday());
    assert_eq!(
        r.started(),
        vec![vec![SourceId(2)]],
        "the add started its scan"
    );
    let status = label("status-activity", &mut r.cx).unwrap();
    assert!(
        status.starts_with("Walking folders") || status.starts_with("Scanning"),
        "{status}"
    );
    assert!(
        r.shell_backend
            .activity_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            > calls_before,
        "the shell samples the run's activity (the Library's live updates)"
    );
    // Later, the analysis phase reads as progress.
    r.wait("the analysis", |r| {
        label("status-activity", &mut r.cx).is_some_and(|s| s.starts_with("Scanning"))
    });
    let shell = r.shell.clone();
    assert_eq!(
        r.cx.update(|_, cx| shell.read(cx).active_view()),
        ViewId::Library,
        "the add did not switch views"
    );
}

#[gpui_kit::test]
fn scenario_scan_progress_queued_add_starts_after_the_active_run(cx: &mut TestAppContext) {
    let mut r = rig(cx, one_small_source());
    start_all(&mut r);
    click("source-add", &mut r.cx);
    r.pick(holiday());
    assert_eq!(r.started(), vec![vec![SourceId(1)]], "nothing started yet");
    let m = r.monitor();
    assert_eq!(
        r.cx.update(|_, cx| m.read(cx).queued.clone()),
        vec![SourceId(2)]
    );
    let row = label("source-row-2", &mut r.cx).unwrap();
    assert!(row.contains("Queued, scans next"), "{row}");
    r.show(ViewId::Library);
    let status = label("status-activity", &mut r.cx).unwrap();
    assert!(status.ends_with("then 1 more source"), "{status}");
    // The run completes: a run of only the new source starts by itself.
    r.wait("the queued run", |r| r.started().len() == 2);
    assert_eq!(
        r.started(),
        vec![vec![SourceId(1)], vec![SourceId(2)]],
        "the finished source is not rescanned"
    );
    assert!(r.cx.update(|_, cx| m.read(cx).queued.is_empty()));
    r.wait("the queued run to show", |r| {
        label("status-activity", &mut r.cx).is_some_and(|s| !s.contains("then"))
            && r.snap(|s| s.status) == Some(RunStatus::Running)
    });
}

#[gpui_kit::test]
fn scenario_scan_progress_cancelled_run_leaves_the_queue_visible(cx: &mut TestAppContext) {
    let mut config = one_small_source();
    config.files_per_s = 50.0;
    let mut r = rig(cx, config);
    start_all(&mut r);
    click("source-add", &mut r.cx);
    r.pick(holiday());
    click("scan-cancel", &mut r.cx);
    r.wait("the cancel", |r| {
        r.snap(|s| s.status) == Some(RunStatus::Cancelled)
    });
    r.tick(2_000);
    assert_eq!(
        r.started(),
        vec![vec![SourceId(1)]],
        "no run starts by itself"
    );
    let m = r.monitor();
    assert_eq!(
        r.cx.update(|_, cx| m.read(cx).queued.clone()),
        vec![SourceId(2)]
    );
    let row = label("source-row-2", &mut r.cx).unwrap();
    assert!(row.contains("Queued, scans next"), "{row}");
    assert_eq!(
        label("scan-new", &mut r.cx).as_deref(),
        Some("Scan “Holiday 2024”")
    );
    click("scan-new", &mut r.cx);
    r.wait("its run", |r| r.started().len() == 2);
    assert_eq!(r.started()[1], vec![SourceId(2)]);
    assert!(r.cx.update(|_, cx| m.read(cx).queued.is_empty()));
}

#[gpui_kit::test]
fn scenario_scan_progress_primary_action_scans_only_never_scanned_sources(cx: &mut TestAppContext) {
    let backend = Arc::new(DemoBackend::new(FakeScanService::new(
        FakeConfig::two_sources(500.0),
    )));
    backend.never_scanned.lock().unwrap().push(SourceId(2));
    let mut r = rig_backend(cx, backend, Arc::new(ui_gallery::source::EmptySource));
    assert_eq!(
        label("scan-new", &mut r.cx).as_deref(),
        Some("Scan “NAS · Family”")
    );
    assert_eq!(
        label("scan-start", &mut r.cx).as_deref(),
        Some("Scan 2 sources (1 new)")
    );
    let row = label("source-row-2", &mut r.cx).unwrap();
    assert!(row.contains("Not scanned yet"), "{row}");
    click("scan-new", &mut r.cx);
    r.wait("the run", |r| {
        r.snap(|s| s.status) == Some(RunStatus::Running)
    });
    assert_eq!(r.started(), vec![vec![SourceId(2)]], "only the new source");
}

fn settings_off() -> Dirs {
    let dirs = Dirs::new();
    dirs.store()
        .update(|s| s.scanning.scan_new_sources = false)
        .unwrap();
    dirs
}

#[gpui_kit::test]
fn scenario_settings_new_source_not_scanned_when_the_setting_is_off(cx: &mut TestAppContext) {
    let backend = Arc::new(DemoBackend::new(FakeScanService::new(one_small_source())));
    let mut r = rig_full(
        cx,
        backend,
        Arc::new(ui_gallery::source::EmptySource),
        FakeBackend::small_corpus(),
        Some(settings_off()),
    );
    r.show(ViewId::Sources);
    click("source-add", &mut r.cx);
    r.pick(holiday());
    r.tick(500);
    assert!(r.started().is_empty(), "no run started");
    let m = r.monitor();
    assert!(!r.cx.update(|_, cx| m.read(cx).is_active()));
    let row = label("source-row-2", &mut r.cx).unwrap();
    assert!(row.contains("Not scanned yet"), "{row}");
    assert_eq!(
        label("scan-new", &mut r.cx).as_deref(),
        Some("Scan “Holiday 2024”")
    );
    assert_eq!(
        label("scan-start", &mut r.cx).as_deref(),
        Some("Scan 1 source"),
        "the new source is the selection"
    );
    click("scan-new", &mut r.cx);
    r.wait("its run", |r| r.started().len() == 1);
    assert_eq!(r.started(), vec![vec![SourceId(2)]]);
}

#[gpui_kit::test]
fn scenario_app_shell_onboarding_add_without_scanning_leads_to_sources(cx: &mut TestAppContext) {
    let mut config = one_small_source();
    config.sources.clear();
    let backend = Arc::new(DemoBackend::new(FakeScanService::new(config)));
    let mut r = rig_full(
        cx,
        backend,
        Arc::new(ui_gallery::source::EmptySource),
        FakeBackend::default(),
        Some(settings_off()),
    );
    assert!(exists("onboarding", &mut r.cx), "first launch");
    click("onboarding-add-source", &mut r.cx);
    // The catalog the shell reads next holds the new source (the shell's fake engine is not the scan hub's).
    r.shell_backend.summary.lock().unwrap().sources = 1;
    r.pick(holiday());
    r.wait("the Sources view", |r| exists("source-row-1", &mut r.cx));
    let shell = r.shell.clone();
    assert_eq!(
        r.cx.update(|_, cx| shell.read(cx).active_view()),
        ViewId::Sources
    );
    let row = label("source-row-1", &mut r.cx).unwrap();
    assert!(row.contains("Not scanned yet"), "{row}");
    assert_eq!(
        label("scan-new", &mut r.cx).as_deref(),
        Some("Scan “Holiday 2024”")
    );
    let m = r.monitor();
    assert!(
        !r.cx.update(|_, cx| m.read(cx).is_active()),
        "no run is active"
    );
    assert!(r.started().is_empty());
}
