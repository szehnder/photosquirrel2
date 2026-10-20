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
    fn excluded(&self) -> anyhow::Result<Vec<ui_gallery::source::ExcludedRow>> {
        Ok(Vec::new())
    }
}

struct Rig {
    shell: Entity<Shell>,
    cx: VisualTestContext,
    backend: Arc<DemoBackend>,
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
    init_app(cx, Arc::new(FakeBackend::small_corpus()), None);
    let b = backend.clone();
    cx.update(move |cx| ui_workspace::views::scan::install(b, gallery, cx));
    let (shell, mut vcx) = open(cx);
    wait_loaded(&shell, &mut vcx);
    let mut rig = Rig {
        shell,
        cx: vcx,
        backend,
    };
    rig.show(ViewId::Sources);
    rig
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
        ("stage-discovered", "Discovered 1000"),
        ("stage-probed", "Probed 600"),
        ("stage-analyzed", "Analyzed 400"),
        ("stage-thumbed", "Thumbnailed 300"),
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

/// Render the dashboard `frames` times for a pinned snapshot of a run at `files_per_s`; returns the frame times.
fn frame_times(r: &mut Rig, files_per_s: f64, frames: usize) -> Vec<Duration> {
    let fake = FakeScanService::new(FakeConfig::two_sources(files_per_s));
    fake.start_at(&[], 4_000, RunStatus::Running);
    let snap = fake.snapshot().unwrap();
    *r.backend.pinned.lock().unwrap() = Some(snap.clone());
    let m = r.monitor();
    r.cx.update(|_, cx| m.update(cx, |m, cx| m.apply(Some(snap), cx)));
    let mut times = Vec::with_capacity(frames);
    for _ in 0..frames {
        let m = m.clone();
        let t = r.cx.update(|window, cx| {
            m.update(cx, |_, cx| cx.notify());
            let t0 = Instant::now();
            window.refresh();
            window.render_frame(cx);
            t0.elapsed()
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
fn scenario_scan_progress_ui_cost_independent_of_rate(cx: &mut TestAppContext) {
    let mut r = rig(cx, FakeConfig::two_sources(100.0));
    // Warm up layout and text caches.
    frame_times(&mut r, 100.0, 20);
    frame_times(&mut r, 20_000.0, 20);
    // Interleave short rounds so machine noise (other tests running in parallel) hits both rates alike.
    let (mut slow, mut fast) = (Vec::new(), Vec::new());
    for _ in 0..8 {
        slow.extend(frame_times(&mut r, 100.0, 15));
        fast.extend(frame_times(&mut r, 20_000.0, 15));
    }
    let (slow, fast) = (p95(slow), p95(fast));
    let diff = slow.abs_diff(fast);
    eprintln!("dashboard frame p95: {slow:?} at 100 files/s, {fast:?} at 20,000 files/s");
    assert!(diff < Duration::from_millis(1), "p95 differs by {diff:?}");
    assert!(
        slow.max(fast) <= Duration::from_micros(8_300),
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
