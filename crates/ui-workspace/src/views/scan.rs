//! The live scan state shared by the Sources view (scan dashboard) and the status bar (change
//! `scan-dashboard`).
//!
//! [`ScanMonitor`] owns the app's single sampling loop: while a run is active it takes one
//! [`ProgressSnapshot`] every [`SAMPLE_INTERVAL`] on a background task and stores it; views render from the
//! stored snapshot only (ADR-0019), so their cost does not depend on the scan rate. When the run ends the
//! loop stops and the run summary is fetched. [`ScanHub`] is the global that holds the monitor.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;
use ps_model::{ProgressSnapshot, RunStatus, ScanPhase, ScanRunId, SourceId};
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

/// A folder being added (responsive-add-source, D4): shown as an "Adding…" row from the frame the picker returns
/// until the add answers; a failed add keeps its row with the error until dismissed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAdd {
    pub key: u64,
    pub path: PathBuf,
    /// The folder's name ("Pictures").
    pub name: String,
    pub error: Option<String>,
}

/// What happened to the sources an add registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddStart {
    /// Nothing was added.
    Nothing,
    /// Their scan started.
    Started,
    /// A run was active: they scan next, after it (the owner's choice, D6).
    Queued,
    /// "Scan new sources right away" is off, or the start failed: "Not scanned yet".
    NotScanned,
}

/// The result of [`ScanMonitor::add_paths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOutcome {
    pub added: Vec<SourceId>,
    /// Names of the added folders.
    pub names: Vec<String>,
    /// Folders not added: name and error.
    pub failed: Vec<(String, String)>,
    pub start: AddStart,
    /// Why the added sources were not scanned when the start was refused (an export or a cleanup holds the
    /// operations lock): shown on the dashboard and in the add's toast.
    pub note: Option<String>,
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
    /// The run summary's "How to enable HEIC previews" disclosure is open.
    pub decoder_fix_open: bool,
    /// A folder picker for Add source is open (Add is unavailable meanwhile).
    pub picking: bool,
    /// Folders being added, or whose add failed (D4).
    pub adding: Vec<PendingAdd>,
    next_add: u64,
    /// Sources added while a run was active, each once, in order: they scan next (D6).
    pub queued: Vec<SourceId>,
    /// Sources an add did not scan, for the Sources view to select once (generation, ids; D7).
    pub selection_hint: Option<(u64, Vec<SourceId>)>,
    /// The queue's start in flight: the sources are listed again first, so only present, online ones start.
    queue_check: Option<Task<()>>,
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
            // Dev-only: scripted screenshots of the open "How to enable HEIC previews".
            decoder_fix_open: std::env::var("PHOTOSQUIRREL_DECODER_FIX").is_ok_and(|v| v == "open"),
            picking: false,
            adding: Vec::new(),
            next_add: 0,
            queued: Vec::new(),
            selection_hint: None,
            queue_check: None,
        };
        this.refresh_sources(cx);
        this.refresh_last_run(cx);
        this.ensure_sampling(cx);
        this
    }

    /// Open or close the run summary's "How to enable HEIC previews".
    pub fn toggle_decoder_fix(&mut self, cx: &mut Context<'_, Self>) {
        self.decoder_fix_open = !self.decoder_fix_open;
        cx.notify();
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

    /// The status bar's line while a folder is being added ("Adding Pictures…") or a run is active ("Scanning 41 of
    /// 100 files · 41% · about 4 min left", "Walking folders"), with the queued sources that follow it ("then 1 more
    /// source").
    pub fn status_line(&self) -> Option<String> {
        let adding: Vec<&PendingAdd> = self.adding.iter().filter(|a| a.error.is_none()).collect();
        match adding.as_slice() {
            [] => {}
            [one] => return Some(format!("Adding {}…", one.name)),
            many => return Some(format!("Adding {} folders…", many.len())),
        }
        let s = self.snapshot.as_ref()?;
        let pct = self
            .percent()
            .map(|p| format!(" · {p}%"))
            .unwrap_or_default();
        let more = match self.queued_now().len() {
            0 => String::new(),
            1 => " · then 1 more source".to_owned(),
            n => format!(" · then {n} more sources"),
        };
        match (self.pending, s.status) {
            (Some(c), _) => Some(c.label().to_owned()),
            (None, RunStatus::Paused) => Some(format!("Scan paused{pct}{more}")),
            (None, RunStatus::Running)
                if !s.fill && matches!(s.phase, ScanPhase::Starting | ScanPhase::Walking) =>
            {
                Some(format!("Walking folders{more}"))
            }
            (None, RunStatus::Running) => {
                let eta = match self.eta() {
                    Eta::Remaining(d) => {
                        format!(" · {} left", ps_service::scans::eta::format_remaining(d))
                    }
                    Eta::Estimating => " · estimating".into(),
                    Eta::Done => String::new(),
                };
                Some(format!(
                    "Scanning {} of {} files{pct}{eta}{more}",
                    format_count(s.processed),
                    format_count(s.to_process)
                ))
            }
            _ => None,
        }
    }

    /// Store a fresh source list. The queue keeps only sources still present and online: a removed or folded source
    /// leaves it, and one that went offline is dropped, so its row reads "Not scanned yet" and it never starts by
    /// surprise when it comes back.
    pub fn set_sources(&mut self, rows: Vec<SourceRow>) {
        self.queued
            .retain(|id| rows.iter().any(|r| r.id == *id && r.online));
        self.sources = Loadable::Ready(rows);
    }

    /// The queued sources the shown list confirms present and online; none while the list is not loaded.
    pub fn queued_now(&self) -> Vec<SourceId> {
        let Some(rows) = self.sources.ready() else {
            return Vec::new();
        };
        self.queued
            .iter()
            .copied()
            .filter(|id| rows.iter().any(|r| r.id == *id && r.online))
            .collect()
    }

    // ---- adding sources (responsive-add-source) ----------------------------------------------------------

    /// The picker is open or a folder is being added: Add is unavailable.
    pub fn add_busy(&self) -> bool {
        self.picking || self.adding.iter().any(|a| a.error.is_none())
    }

    /// Open the folder picker and add what it returns ([`ScanMonitor::add_paths`]); `None` when the picker was
    /// cancelled or Add is busy.
    pub fn pick_and_add(
        &mut self,
        scan_now: bool,
        cx: &mut Context<'_, Self>,
    ) -> Task<Option<AddOutcome>> {
        if self.add_busy() {
            return Task::ready(None);
        }
        self.picking = true;
        cx.notify();
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add source".into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = match picked.await {
                Ok(Ok(Some(p))) if !p.is_empty() => Some(p),
                _ => None,
            };
            let task = this
                .update(cx, |m, cx| {
                    m.picking = false;
                    cx.notify();
                    paths.map(|p| m.add_paths(p, scan_now, cx))
                })
                .ok()
                .flatten()?;
            Some(task.await)
        })
    }

    /// Add `paths` as sources (D4-D7). Each shows as an "Adding…" row from this update on, so the next frame draws
    /// it; one background task registers them (constant cost, ADR-0121), lists the sources and, with `scan_now`,
    /// starts their scan, or queues them when a run is active. The pending rows give way to the real rows in the same
    /// update; a failed folder keeps its row with the error.
    pub fn add_paths(
        &mut self,
        paths: Vec<PathBuf>,
        scan_now: bool,
        cx: &mut Context<'_, Self>,
    ) -> Task<AddOutcome> {
        let mut keys = Vec::new();
        for path in &paths {
            self.next_add += 1;
            keys.push(self.next_add);
            self.adding.push(PendingAdd {
                key: self.next_add,
                path: path.clone(),
                name: crate::scan_backend::source_name(path),
                error: None,
            });
        }
        self.error = None;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let work = cx.background_spawn(async move {
                let results: Vec<Result<SourceId, String>> = paths
                    .iter()
                    .map(|p| backend.add_source(p).map_err(|e| format!("{e:#}")))
                    .collect();
                let added: Vec<SourceId> = results.iter().filter_map(|r| r.clone().ok()).collect();
                let active = |b: &crate::scan_backend::SharedScanBackend| {
                    b.snapshot()
                        .is_some_and(|s| matches!(s.status, RunStatus::Running | RunStatus::Paused))
                };
                let mut start_error: Option<(Option<String>, String)> = None;
                let (start, snap) = if added.is_empty() {
                    (AddStart::Nothing, None)
                } else if !scan_now {
                    (AddStart::NotScanned, None)
                } else if active(&backend) {
                    (AddStart::Queued, None)
                } else {
                    match backend.start(&added) {
                        Ok(_) => (AddStart::Started, backend.snapshot()),
                        // A run started meanwhile: they scan after it. Never "not added".
                        Err(_) if active(&backend) => (AddStart::Queued, None),
                        Err(e) => {
                            // An export or a cleanup holds the operations lock: say so (task 6.7, L8).
                            let busy = match e.downcast_ref::<ps_service::scan::ScanError>() {
                                Some(ps_service::scan::ScanError::Busy(op)) => Some(op.clone()),
                                _ => None,
                            };
                            start_error = Some((busy, format!("{e:#}")));
                            (AddStart::NotScanned, None)
                        }
                    }
                };
                let rows = backend.sources();
                (results, added, start, snap, rows, start_error)
            });
            let (results, added, start, snap, rows, start_error) = work.await;
            this.update(cx, |m, cx| {
                let mut outcome = AddOutcome {
                    added: added.clone(),
                    names: Vec::new(),
                    failed: Vec::new(),
                    start,
                    note: None,
                };
                for (key, r) in keys.iter().zip(&results) {
                    let Some(i) = m.adding.iter().position(|a| a.key == *key) else {
                        continue;
                    };
                    match r {
                        Ok(_) => outcome.names.push(m.adding.remove(i).name),
                        Err(e) => {
                            outcome.failed.push((m.adding[i].name.clone(), e.clone()));
                            m.adding[i].error = Some(e.clone());
                        }
                    }
                }
                match rows {
                    Ok(rows) => m.set_sources(rows),
                    Err(_) => m.refresh_sources(cx),
                }
                match start {
                    AddStart::Started => {
                        m.apply(snap, cx);
                        m.overview = None;
                        m.ensure_sampling(cx);
                    }
                    AddStart::Queued => {
                        for id in &added {
                            if !m.queued.contains(id) {
                                m.queued.push(*id);
                            }
                        }
                        // The run may have ended meanwhile: sampling sees it and starts the queue (`run_ended`).
                        m.ensure_sampling(cx);
                    }
                    AddStart::NotScanned => {
                        let generation = m.selection_hint.as_ref().map_or(1, |(g, _)| g + 1);
                        m.selection_hint = Some((generation, added.clone()));
                    }
                    AddStart::Nothing => {}
                }
                if let Some((busy, e)) = start_error {
                    let named = match outcome.names.as_slice() {
                        [one] => format!("“{one}”"),
                        many => format!("{} folders", many.len()),
                    };
                    let it = if outcome.names.len() == 1 { "it" } else { "them" };
                    let message = match busy {
                        Some(op) => format!(
                            "Added {named}, not scanned: {op} is running. Scan {it} when it finishes."
                        ),
                        None => format!("Added {named}, not scanned: could not start the scan: {e}"),
                    };
                    outcome.note = Some(message.clone());
                    m.error = Some(message);
                }
                cx.notify();
                outcome
            })
            .unwrap_or(AddOutcome {
                added,
                names: Vec::new(),
                failed: Vec::new(),
                start,
                note: None,
            })
        })
    }

    /// Forget a failed add's row.
    pub fn dismiss_add(&mut self, key: u64, cx: &mut Context<'_, Self>) {
        self.adding.retain(|a| a.key != key);
        cx.notify();
    }

    /// Start a run of the queued sources once the run before them completed (D6). The sources are listed again first
    /// ([`ScanMonitor::set_sources`]): only those still present and online start, and nothing starts when the list
    /// cannot be read.
    fn start_queued(&mut self, cx: &mut Context<'_, Self>) {
        if self.queued.is_empty()
            || self.is_active()
            || self.pending.is_some()
            || self.queue_check.is_some()
        {
            return;
        }
        let backend = self.backend.clone();
        self.queue_check = Some(cx.spawn(async move |this, cx| {
            let rows = cx.background_spawn(async move { backend.sources() }).await;
            let _ = this.update(cx, |m, cx| {
                m.queue_check = None;
                match rows {
                    Ok(rows) => m.set_sources(rows),
                    Err(e) => {
                        m.sources = Loadable::Failed(format!("{e:#}"));
                        cx.notify();
                        return;
                    }
                }
                if !m.queued.is_empty() && !m.is_active() && m.pending.is_none() {
                    let ids = m.queued.clone();
                    m.start(ids, cx);
                }
                cx.notify();
            });
        }));
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
        let Some((run, status)) = self.snapshot.as_ref().map(|s| (s.run, s.status)) else {
            // No run at all: a queue (from a race with a run's end) can start now.
            self.start_queued(cx);
            return;
        };
        // The queued sources scan next after a completed run; a cancelled run leaves them queued until the user acts.
        if status == RunStatus::Completed {
            self.start_queued(cx);
        }
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
        match rows {
            Ok(r) => self.set_sources(r),
            Err(e) => self.sources = Loadable::Failed(format!("{e:#}")),
        }
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
                match rows {
                    Ok(r) => m.set_sources(r),
                    Err(e) => m.sources = Loadable::Failed(format!("{e:#}")),
                }
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
        // Started now: no longer waiting.
        self.queued.retain(|id| !sources.contains(id));
        self.control(
            Control::Starting,
            move |b| b.start(&sources).map(|_| ()),
            cx,
        );
    }

    /// Rescan `sources` refreshing the analysis of their files analysed by an older version (the summary's
    /// "Refresh analysis", ADR-0071).
    pub fn refresh_analysis(&mut self, sources: Vec<SourceId>, cx: &mut Context<'_, Self>) {
        if self.is_active() {
            return;
        }
        self.control(
            Control::Starting,
            move |b| b.start_refreshing_analysis(&sources).map(|_| ()),
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
            m.set_sources(rows);
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
