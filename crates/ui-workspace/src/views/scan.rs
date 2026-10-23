//! The live scan state shared by the Sources view (scan dashboard) and the status bar (change
//! `scan-dashboard`).
//!
//! [`ScanMonitor`] owns the app's single sampling loop: while a run is active it takes one
//! [`ProgressSnapshot`] every [`SAMPLE_INTERVAL`] on a background task and stores it; views render from the
//! stored snapshot only (ADR-0019), so their cost does not depend on the scan rate. When the run ends the
//! loop stops and the run summary is fetched. [`ScanHub`] is the global that holds the monitor.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use gpui_kit::*;
use ps_model::{ProgressSnapshot, RunStatus, ScanRunId, SourceId};
use ps_service::scans::eta::{RATE_WINDOW_MS, remaining_bytes, window_rates};
use ps_service::scans::{Eta, EtaEstimator, RunError, RunOverview};
use ui_gallery::source::{SharedSource, ThumbKey};
use ui_gallery::strip::ThumbStrip;

use crate::inspector::format_count;
use crate::scan_backend::{SharedScanBackend, SourceRow};
use crate::service::Loadable;

/// The dashboard samples at most this often (ADR-0019).
pub const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
/// How long a cancel waits for the engine to stop before the dashboard samples anyway.
pub const CANCEL_WAIT: Duration = Duration::from_secs(2);
/// Points per sparkline (one per second of run time).
pub const SPARK_POINTS: usize = 60;

/// A control the user just clicked, shown at once until the engine confirms it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Starting,
    Pausing,
    Resuming,
    Cancelling,
}

impl Control {
    pub fn label(self) -> &'static str {
        match self {
            Control::Starting => "Starting…",
            Control::Pausing => "Pausing…",
            Control::Resuming => "Resuming…",
            Control::Cancelling => "Cancelling…",
        }
    }
}

/// Per-source throughput history (MB/s and files/s, one point per second of run time).
#[derive(Debug, Clone, Default)]
pub struct LaneHistory {
    last_ms: u64,
    last_bytes: u64,
    last_files: u64,
    pub bytes_per_s: VecDeque<f64>,
    pub files_per_s: f64,
}

impl LaneHistory {
    pub fn current_bytes_per_s(&self) -> f64 {
        self.bytes_per_s.back().copied().unwrap_or(0.0)
    }
}

/// The app's live scan state.
pub struct ScanMonitor {
    backend: SharedScanBackend,
    pub snapshot: Option<ProgressSnapshot>,
    eta: EtaEstimator,
    eta_run: Option<ScanRunId>,
    pub lanes: HashMap<SourceId, LaneHistory>,
    /// Summary of the run shown when nothing runs (the one that just ended, or the last one).
    pub overview: Option<RunOverview>,
    pub sources: Loadable<Vec<SourceRow>>,
    pub pending: Option<Control>,
    /// The last control or engine error, shown inline.
    pub error: Option<String>,
    /// Every stored error of the shown run, once the user asked for them.
    pub all_errors: Option<Loadable<Vec<RunError>>>,
    /// Snapshots taken so far (tests check the 10 Hz cap).
    pub samples: u64,
    pub strip: Entity<ThumbStrip>,
    sampling: Option<Task<()>>,
    /// The run whose end was last handled (summary fetched), so repeated wake-ups do no work.
    ended: Option<ScanRunId>,
    /// Why the run is paused for power, or that mains power is back (`general.pause_on_battery`).
    pub power_note: Option<String>,
}

impl ScanMonitor {
    pub fn new(
        backend: SharedScanBackend,
        gallery: SharedSource,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let strip = cx.new(|_| ThumbStrip::new(gallery));
        cx.observe(&strip, |_, _, cx| cx.notify()).detach();
        let mut this = Self {
            backend,
            snapshot: None,
            eta: EtaEstimator::new(),
            eta_run: None,
            lanes: HashMap::new(),
            overview: None,
            sources: Loadable::Loading,
            pending: None,
            error: None,
            all_errors: None,
            samples: 0,
            strip,
            sampling: None,
            ended: None,
            power_note: None,
        };
        this.refresh_sources(cx);
        this.refresh_last_run(cx);
        this.ensure_sampling(cx);
        this
    }

    pub fn backend(&self) -> &SharedScanBackend {
        &self.backend
    }

    /// A run is running or paused.
    pub fn is_active(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| matches!(s.status, RunStatus::Running | RunStatus::Paused))
    }

    pub fn is_sampling(&self) -> bool {
        self.sampling.is_some()
    }

    pub fn eta(&self) -> Eta {
        match &self.snapshot {
            Some(s) => self.eta.eta(remaining_bytes(s)),
            None => Eta::Estimating,
        }
    }

    /// Overall files/s and bytes/s over the last 10 s.
    pub fn rates(&self) -> (f64, f64) {
        self.snapshot
            .as_ref()
            .map_or((0.0, 0.0), |s| window_rates(s, RATE_WINDOW_MS))
    }

    /// Percent of the run's work done.
    pub fn percent(&self) -> Option<u32> {
        let s = self.snapshot.as_ref()?;
        (s.to_process > 0).then(|| ((s.processed.min(s.to_process) * 100) / s.to_process) as u32)
    }

    /// The status bar's line while a run is active ("Scanning · 41% · about 4 min left").
    pub fn status_line(&self) -> Option<String> {
        let s = self.snapshot.as_ref()?;
        let pct = self
            .percent()
            .map(|p| format!(" · {p}%"))
            .unwrap_or_default();
        match (self.pending, s.status) {
            (Some(c), _) => Some(c.label().to_owned()),
            (None, RunStatus::Paused) => Some(format!("Scan paused{pct}")),
            (None, RunStatus::Running) => {
                let eta = match self.eta() {
                    Eta::Remaining(d) => {
                        format!(" · {} left", ps_service::scans::eta::format_remaining(d))
                    }
                    Eta::Estimating => " · estimating".into(),
                    Eta::Done => String::new(),
                };
                Some(format!(
                    "Scanning {} of {} files{pct}{eta}",
                    format_count(s.processed),
                    format_count(s.to_process)
                ))
            }
            _ => None,
        }
    }

    // ---- sampling -------------------------------------------------------------------------------------

    /// Start the 10 Hz loop if it is not running (it stops by itself when no run is active).
    pub fn ensure_sampling(&mut self, cx: &mut Context<'_, Self>) {
        if self.sampling.is_some() {
            return;
        }
        let backend = self.backend.clone();
        self.sampling = Some(cx.spawn(async move |this, cx| {
            loop {
                let b = backend.clone();
                let (snap, note) = cx
                    .background_spawn(async move { (b.snapshot(), b.power_note()) })
                    .await;
                let Ok(active) = this.update(cx, |m, cx| {
                    m.set_power_note(note, cx);
                    m.apply(snap, cx)
                }) else {
                    return;
                };
                if !active {
                    break;
                }
                cx.background_executor().timer(SAMPLE_INTERVAL).await;
            }
            let _ = this.update(cx, |m, cx| {
                m.sampling = None;
                m.run_ended(cx);
            });
        }));
    }

    /// Store the power note (re-renders only on change).
    pub fn set_power_note(&mut self, note: Option<String>, cx: &mut Context<'_, Self>) {
        if self.power_note != note {
            self.power_note = note;
            cx.notify();
        }
    }

    /// Store a snapshot; returns whether the run is still active.
    pub fn apply(&mut self, snap: Option<ProgressSnapshot>, cx: &mut Context<'_, Self>) -> bool {
        self.samples += 1;
        let Some(snap) = snap else {
            // Only a change re-renders: the status bar wakes the monitor on every render.
            if self.snapshot.take().is_some() {
                cx.notify();
            }
            return false;
        };
        if self.eta_run != Some(snap.run) {
            self.eta = EtaEstimator::new();
            self.eta_run = Some(snap.run);
            self.lanes.clear();
            self.all_errors = None;
        }
        self.eta.observe(&snap.throughput);
        for src in &snap.sources {
            let lane = self.lanes.entry(src.source).or_default();
            if lane.last_ms == 0 {
                lane.last_ms = snap.elapsed_ms.max(1);
                lane.last_bytes = src.bytes_read;
                lane.last_files = src.processed;
                continue;
            }
            let dt = snap.elapsed_ms.saturating_sub(lane.last_ms);
            if dt >= 1000 {
                let secs = dt as f64 / 1000.0;
                lane.bytes_per_s
                    .push_back(src.bytes_read.saturating_sub(lane.last_bytes) as f64 / secs);
                lane.files_per_s = src.processed.saturating_sub(lane.last_files) as f64 / secs;
                while lane.bytes_per_s.len() > SPARK_POINTS {
                    lane.bytes_per_s.pop_front();
                }
                lane.last_ms = snap.elapsed_ms;
                lane.last_bytes = src.bytes_read;
                lane.last_files = src.processed;
            }
        }
        let keys: Vec<ThumbKey> = snap
            .recent_photos
            .iter()
            .map(|p| ThumbKey(p.content.digest))
            .collect();
        self.strip.update(cx, |s, cx| s.set_keys(keys, cx));
        let active = matches!(snap.status, RunStatus::Running | RunStatus::Paused);
        if active {
            self.ended = None;
        }
        self.pending = match (self.pending, snap.status) {
            (Some(Control::Pausing), RunStatus::Paused)
            | (Some(Control::Resuming | Control::Starting), RunStatus::Running)
            | (Some(Control::Cancelling), RunStatus::Cancelled | RunStatus::Completed) => None,
            (p, _) => p,
        };
        if self.snapshot.as_ref() != Some(&snap) {
            self.snapshot = Some(snap);
            cx.notify();
        }
        active
    }

    fn run_ended(&mut self, cx: &mut Context<'_, Self>) {
        self.pending = None;
        let Some(run) = self.snapshot.as_ref().map(|s| s.run) else {
            return;
        };
        if self.ended == Some(run) {
            return;
        }
        self.ended = Some(run);
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let o = cx
                .background_spawn(async move { backend.overview(run) })
                .await;
            let _ = this.update(cx, |m, cx| {
                match o {
                    Ok(Some(o)) => m.overview = Some(o),
                    Ok(None) => {}
                    Err(e) => m.error = Some(format!("{e:#}")),
                }
                m.refresh_sources(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Shown at start-up (`--command shell::ShowSources`): load the sources and the last run within the start-up
    /// grace, so frame one is the list rather than "Reading sources…" (docs/gpui-notes.md). `false` when the
    /// grace is over or the load did not finish in it; the caller then refreshes asynchronously.
    pub fn prime_within_startup_grace(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(deadline) = crate::startup::StartupGrace::deadline(cx) else {
            return false;
        };
        let backend = self.backend.clone();
        let loaded = crate::startup::StartupGrace::within(deadline, move || {
            (backend.sources(), backend.last_run())
        });
        let Some((rows, last)) = loaded else {
            return false;
        };
        self.sources = match rows {
            Ok(r) => Loadable::Ready(r),
            Err(e) => Loadable::Failed(format!("{e:#}")),
        };
        if self.overview.is_none()
            && let Ok(Some(o)) = last
        {
            self.overview = Some(o);
        }
        true
    }

    pub fn refresh_sources(&mut self, cx: &mut Context<'_, Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let rows = cx.background_spawn(async move { backend.sources() }).await;
            let _ = this.update(cx, |m, cx| {
                m.sources = match rows {
                    Ok(r) => Loadable::Ready(r),
                    Err(e) => Loadable::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_last_run(&mut self, cx: &mut Context<'_, Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let o = cx.background_spawn(async move { backend.last_run() }).await;
            let _ = this.update(cx, |m, cx| {
                if m.overview.is_none()
                    && let Ok(Some(o)) = o
                {
                    m.overview = Some(o);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- controls -------------------------------------------------------------------------------------

    /// Run `f` on a background task with `control` shown at once, then sample immediately so the new status
    /// shows without waiting for the next tick.
    fn control(
        &mut self,
        control: Control,
        f: impl FnOnce(&SharedScanBackend) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<'_, Self>,
    ) {
        if self.pending.is_some() {
            return;
        }
        self.pending = Some(control);
        self.error = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let b = backend.clone();
            let result = cx
                .background_spawn(async move {
                    let r = f(&b);
                    (r, b.snapshot())
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let (r, snap) = result;
                if let Err(e) = r {
                    m.pending = None;
                    m.error = Some(format!("{e:#}"));
                }
                m.apply(snap, cx);
                m.pending = None;
                if control == Control::Starting || control == Control::Resuming {
                    m.overview = None;
                }
                m.ensure_sampling(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn start(&mut self, sources: Vec<SourceId>, cx: &mut Context<'_, Self>) {
        if self.is_active() {
            return;
        }
        self.control(
            Control::Starting,
            move |b| b.start(&sources).map(|_| ()),
            cx,
        );
    }

    pub fn pause(&mut self, cx: &mut Context<'_, Self>) {
        self.control(Control::Pausing, |b| b.pause(), cx);
    }

    pub fn resume(&mut self, cx: &mut Context<'_, Self>) {
        let run = self.snapshot.as_ref().map(|s| (s.run, s.status));
        match run {
            Some((_, RunStatus::Paused)) => self.control(Control::Resuming, |b| b.resume(), cx),
            Some((run, _)) => self.control(Control::Resuming, move |b| b.resume_run(run), cx),
            None => {}
        }
    }

    /// Resume a specific run (the interrupted-run banner, the summary's Resume).
    pub fn resume_run(&mut self, run: ScanRunId, cx: &mut Context<'_, Self>) {
        self.control(Control::Resuming, move |b| b.resume_run(run), cx);
    }

    /// Cancel the run and wait (on the background task, at most [`CANCEL_WAIT`]) until the engine has stopped
    /// it, so the snapshot `control` takes right after already reads cancelled instead of the next periodic
    /// sample (scan-progress "Pause, resume and cancel").
    pub fn cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.control(
            Control::Cancelling,
            |b| {
                b.cancel()?;
                b.wait_stopped(CANCEL_WAIT);
                Ok(())
            },
            cx,
        );
    }

    pub fn show_all_errors(&mut self, cx: &mut Context<'_, Self>) {
        let Some(run) = self.snapshot.as_ref().map(|s| s.run) else {
            return;
        };
        self.all_errors = Some(Loadable::Loading);
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move { backend.run_errors(run, 10_000) })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.all_errors = Some(match rows {
                    Ok(r) => Loadable::Ready(r),
                    Err(e) => Loadable::Failed(format!("{e:#}")),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

/// The global that holds the monitor (set once by [`install`]).
pub struct ScanHub {
    pub monitor: Entity<ScanMonitor>,
    /// Views load their first data synchronously (the instant simulated engine of scripted screenshots,
    /// [`install_primed`]); never with a real engine.
    pub synchronous: bool,
}

impl Global for ScanHub {}

impl ScanHub {
    pub fn monitor(cx: &App) -> Option<Entity<ScanMonitor>> {
        Some(cx.try_global::<ScanHub>()?.monitor.clone())
    }

    /// Ask the monitor to look for a run started elsewhere (the shell's Rescan, Resume or Add Source).
    pub fn wake(cx: &mut App) {
        if let Some(m) = Self::monitor(cx) {
            m.update(cx, |m, cx| m.ensure_sampling(cx));
        }
    }

    /// Whether views may load their first data on the UI thread (see [`ScanHub::synchronous`]).
    pub fn synchronous(cx: &App) -> bool {
        cx.try_global::<ScanHub>().is_some_and(|h| h.synchronous)
    }

    /// The status bar's line while a run is active.
    pub fn status_line(cx: &App) -> Option<String> {
        Self::monitor(cx)?.read(cx).status_line()
    }
}

/// Connect the scan views to an engine. Without it they explain that scanning is unavailable.
pub fn install(backend: SharedScanBackend, gallery: SharedSource, cx: &mut App) {
    let monitor = cx.new(|cx| ScanMonitor::new(backend, gallery, cx));
    cx.set_global(ScanHub {
        monitor,
        synchronous: false,
    });
}

/// Like [`install`], for an instant simulated engine (`--demo-scan` screenshots): the sources, the first
/// snapshot and the recent-finds thumbnails are loaded before the first frame, which is all a scripted
/// capture gets on some compositors (docs/gpui-notes.md).
pub fn install_primed(backend: SharedScanBackend, gallery: SharedSource, cx: &mut App) {
    let monitor = cx.new(|cx| {
        let mut m = ScanMonitor::new(backend.clone(), gallery, cx);
        if let Ok(rows) = backend.sources() {
            m.sources = Loadable::Ready(rows);
        }
        let snap = backend.snapshot();
        m.apply(snap, cx);
        if m.snapshot.is_none()
            && let Ok(Some(o)) = backend.last_run()
        {
            m.overview = Some(o);
        }
        m.strip.update(cx, |s, _| s.prime_blocking());
        m
    });
    cx.set_global(ScanHub {
        monitor,
        synchronous: true,
    });
}

/// Register the Sources, Export and Reports views.
pub fn register(cx: &mut App) {
    crate::views::ViewRegistry::register(cx, crate::views::ViewId::Sources, |_, _, cx| {
        cx.new(super::sources::SourcesView::new).into()
    });
    crate::views::ViewRegistry::register(cx, crate::views::ViewId::Export, |_, _, cx| {
        cx.new(super::export::ExportView::new).into()
    });
    crate::views::ViewRegistry::register(cx, crate::views::ViewId::Reports, |_, _, cx| {
        cx.new(super::reports::ReportsView::new).into()
    });
}
