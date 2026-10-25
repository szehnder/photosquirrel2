//! A simulated scan engine for UI tests, screenshots and the constant-cost check (change `scan-dashboard`).
//!
//! [`FakeScanService`] produces [`ProgressSnapshot`]s for a configured run (sources, files per second, file
//! sizes, error and skip rates) without any I/O. Time is simulated: every [`FakeScanService::snapshot`]
//! advances the run clock by the configured tick (100 ms by default, the dashboard's sampling interval) while
//! the run is running, and [`FakeScanService::advance`] moves it explicitly. Everything is deterministic, so
//! tests can assert exact counts.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use ps_model::{
    ContentId, DeviceClass, DiffCounts, PhotoClass, ProgressSnapshot, RecentError, RecentPhoto,
    RelPath, RunStatus, ScanPhase, ScanRunId, SourceId, SourceProgress, Stage, StageCounts,
    ThroughputSample,
};

use super::summary::RunOverview;

/// One simulated source.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeSource {
    pub id: SourceId,
    pub name: String,
    pub device: DeviceClass,
    /// Files present in the tree.
    pub files: u64,
    /// Of those, already catalogued and unchanged (a rescan); the rest is new.
    pub unchanged: u64,
    pub changed: u64,
    pub missing: u64,
    pub online: bool,
}

impl FakeSource {
    pub fn new(id: i64, name: &str, device: DeviceClass, files: u64) -> Self {
        Self {
            id: SourceId(id),
            name: name.into(),
            device,
            files,
            unchanged: 0,
            changed: 0,
            missing: 0,
            online: true,
        }
    }

    /// A rescan diff: `new` added, `changed` changed, `missing` deleted, the rest unchanged.
    pub fn rescan(mut self, new: u64, changed: u64, missing: u64) -> Self {
        self.unchanged = self.files.saturating_sub(new + changed);
        self.changed = changed;
        self.missing = missing;
        self
    }

    fn to_process(&self) -> u64 {
        self.files - self.unchanged
    }

    fn new_files(&self) -> u64 {
        self.files - self.unchanged - self.changed
    }
}

/// The simulated run.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeConfig {
    pub sources: Vec<FakeSource>,
    /// Files finished per second of run time (all sources together).
    pub files_per_s: f64,
    pub bytes_per_file: u64,
    /// Run time spent walking (diff known at its end, no analysis before).
    pub walk_ms: u64,
    /// Simulated time per `snapshot()` while running.
    pub tick_ms: u64,
    /// Every n-th processed file fails ("Permission denied"); 0 = never.
    pub error_every: u64,
    /// Percent of processed files skipped as not photos.
    pub skip_percent: u64,
    /// Simulated time from a cancel request until the run has stopped (0: at once). Until then snapshots still
    /// read running; [`FakeScanService::wait_stopped`] waits it out.
    pub cancel_ms: u64,
}

impl FakeConfig {
    /// Two sources (an SSD and a NAS) at `files_per_s`.
    pub fn two_sources(files_per_s: f64) -> Self {
        Self {
            sources: vec![
                FakeSource::new(
                    1,
                    "Macintosh HD · Pictures",
                    DeviceClass::SolidState,
                    60_000,
                ),
                FakeSource::new(2, "NAS · Family", DeviceClass::Network, 40_000),
            ],
            files_per_s,
            bytes_per_file: 4_000_000,
            walk_ms: 500,
            tick_ms: 100,
            error_every: 0,
            skip_percent: 15,
            cancel_ms: 0,
        }
    }
}

#[derive(Debug)]
struct State {
    config: FakeConfig,
    run: Option<ScanRunId>,
    status: RunStatus,
    /// Run time of the current run (only advances while running).
    elapsed_ms: u64,
    /// Sources of the current run.
    selected: Vec<SourceId>,
    next_run: i64,
    started_at_ms: i64,
    /// A cancel is under way: the run time at which the run stops.
    stop_at_ms: Option<u64>,
    /// Files waiting for a decoder, reported by the summary.
    undecoded: Vec<ps_model::Undecoded>,
}

/// A deterministic, I/O-free scan engine.
#[derive(Debug)]
pub struct FakeScanService {
    state: Mutex<State>,
    snapshots: AtomicU64,
}

/// What a fake control call can refuse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FakeError {
    #[error("a scan is already running")]
    Busy,
    #[error("no scan is running")]
    Idle,
    #[error("no such run")]
    NoSuchRun,
}

fn rel(s: String) -> RelPath {
    RelPath::from_bytes(s.into_bytes()).expect("a valid relative path")
}

fn digest_of(n: u64) -> [u8; 32] {
    let mut d = [0u8; 32];
    d[..8].copy_from_slice(&n.to_le_bytes());
    d[8] = 0xfa;
    d
}

impl FakeScanService {
    pub fn new(config: FakeConfig) -> Self {
        Self {
            state: Mutex::new(State {
                config,
                run: None,
                status: RunStatus::Completed,
                elapsed_ms: 0,
                selected: Vec::new(),
                next_run: 1,
                started_at_ms: 0,
                stop_at_ms: None,
                undecoded: Vec::new(),
            }),
            snapshots: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn config(&self) -> FakeConfig {
        self.lock().config.clone()
    }

    /// Change the rate (or anything else) of the simulation.
    pub fn reconfigure(&self, f: impl FnOnce(&mut FakeConfig)) {
        f(&mut self.lock().config);
    }

    /// How many snapshots have been taken.
    pub fn snapshots_taken(&self) -> u64 {
        self.snapshots.load(Ordering::SeqCst)
    }

    pub fn sources(&self) -> Vec<FakeSource> {
        self.lock().config.sources.clone()
    }

    /// The active (running or paused) run.
    pub fn active(&self) -> Option<ScanRunId> {
        let s = self.lock();
        s.run
            .filter(|_| matches!(s.status, RunStatus::Running | RunStatus::Paused))
    }

    /// Start a run over `sources` (every online source when empty).
    pub fn start(&self, sources: &[SourceId]) -> Result<ScanRunId, FakeError> {
        let mut s = self.lock();
        if s.run.is_some() && matches!(s.status, RunStatus::Running | RunStatus::Paused) {
            return Err(FakeError::Busy);
        }
        let id = ScanRunId(s.next_run);
        s.next_run += 1;
        s.run = Some(id);
        s.status = RunStatus::Running;
        s.elapsed_ms = 0;
        s.started_at_ms = 1_700_000_000_000;
        s.stop_at_ms = None;
        s.selected = if sources.is_empty() {
            s.config
                .sources
                .iter()
                .filter(|x| x.online)
                .map(|x| x.id)
                .collect()
        } else {
            sources.to_vec()
        };
        Ok(id)
    }

    /// A run directly in `status` at `elapsed_ms` (screenshots and tests).
    pub fn start_at(&self, sources: &[SourceId], elapsed_ms: u64, status: RunStatus) -> ScanRunId {
        let id = self.start(sources).unwrap_or(ScanRunId(0));
        let mut s = self.lock();
        s.elapsed_ms = elapsed_ms;
        s.status = status;
        id
    }

    pub fn pause(&self) -> Result<ScanRunId, FakeError> {
        let mut s = self.lock();
        match (s.run, s.status) {
            (Some(r), RunStatus::Running) => {
                s.status = RunStatus::Paused;
                Ok(r)
            }
            _ => Err(FakeError::Idle),
        }
    }

    pub fn resume(&self) -> Result<ScanRunId, FakeError> {
        let mut s = self.lock();
        match (s.run, s.status) {
            (Some(r), RunStatus::Paused) => {
                s.status = RunStatus::Running;
                Ok(r)
            }
            _ => Err(FakeError::Idle),
        }
    }

    pub fn cancel(&self) -> Result<ScanRunId, FakeError> {
        let mut s = self.lock();
        match (s.run, s.status) {
            (Some(r), RunStatus::Running | RunStatus::Paused) => {
                if s.config.cancel_ms == 0 {
                    s.status = RunStatus::Cancelled;
                } else if s.stop_at_ms.is_none() {
                    s.stop_at_ms = Some(s.elapsed_ms + s.config.cancel_ms);
                }
                Ok(r)
            }
            _ => Err(FakeError::Idle),
        }
    }

    /// Wait for a cancel to take effect, at most `limit_ms` of simulated time; whether the run has stopped.
    pub fn wait_stopped(&self, limit_ms: u64) -> bool {
        let mut s = self.lock();
        let Some(at) = s.stop_at_ms else {
            return true;
        };
        if at <= s.elapsed_ms + limit_ms {
            s.elapsed_ms = s.elapsed_ms.max(at);
            Self::stop_if_due(&mut s);
            true
        } else {
            s.elapsed_ms += limit_ms;
            false
        }
    }

    /// A cancel under way takes effect once the run clock reaches its stop time.
    fn stop_if_due(s: &mut State) {
        if s.stop_at_ms.is_some_and(|at| at <= s.elapsed_ms) {
            s.stop_at_ms = None;
            s.status = RunStatus::Cancelled;
        }
    }

    /// Continue a cancelled, paused or interrupted run where it stopped (finished files are not redone).
    pub fn resume_run(&self, run: ScanRunId) -> Result<ScanRunId, FakeError> {
        let mut s = self.lock();
        if s.run != Some(run) {
            return Err(FakeError::NoSuchRun);
        }
        match s.status {
            RunStatus::Cancelled | RunStatus::Interrupted | RunStatus::Paused => {
                s.status = RunStatus::Running;
                Ok(run)
            }
            RunStatus::Running => Ok(run),
            RunStatus::Completed => Err(FakeError::Idle),
        }
    }

    /// Move the run clock (only while running).
    pub fn advance(&self, ms: u64) {
        let mut s = self.lock();
        if s.status == RunStatus::Running {
            s.elapsed_ms += ms;
            Self::settle(&mut s);
        }
        Self::stop_if_due(&mut s);
    }

    fn total_to_process(s: &State) -> u64 {
        s.config
            .sources
            .iter()
            .filter(|x| s.selected.contains(&x.id))
            .map(FakeSource::to_process)
            .sum()
    }

    fn processed_at(s: &State, elapsed_ms: u64) -> u64 {
        let analysis_ms = elapsed_ms.saturating_sub(s.config.walk_ms);
        let done = (analysis_ms as f64 * s.config.files_per_s / 1000.0) as u64;
        done.min(Self::total_to_process(s))
    }

    fn settle(s: &mut State) {
        if s.status == RunStatus::Running
            && s.elapsed_ms >= s.config.walk_ms
            && Self::processed_at(s, s.elapsed_ms) >= Self::total_to_process(s)
        {
            s.status = RunStatus::Completed;
        }
    }

    /// The current snapshot (advances the clock by one tick while running). `None` before any run.
    pub fn snapshot(&self) -> Option<ProgressSnapshot> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        let mut s = self.lock();
        let run = s.run?;
        // A cancel takes effect at this sample if its stop time has passed (the sample right after the request
        // still reads running); then the clock moves on by one tick.
        Self::stop_if_due(&mut s);
        if s.status == RunStatus::Running {
            s.elapsed_ms += s.config.tick_ms;
            Self::settle(&mut s);
        }
        Some(Self::build(&s, run))
    }

    /// The snapshot without advancing time or counting.
    pub fn peek(&self) -> Option<ProgressSnapshot> {
        let s = self.lock();
        let run = s.run?;
        Some(Self::build(&s, run))
    }

    fn build(s: &State, run: ScanRunId) -> ProgressSnapshot {
        let c = &s.config;
        let walking = s.elapsed_ms < c.walk_ms && s.status != RunStatus::Completed;
        let processed = if walking {
            0
        } else {
            Self::processed_at(s, s.elapsed_ms)
        };
        let total = Self::total_to_process(s);
        let selected: Vec<&FakeSource> = c
            .sources
            .iter()
            .filter(|x| s.selected.contains(&x.id))
            .collect();
        let running = s.status == RunStatus::Running;
        // Split `processed` across sources in proportion to their work.
        let mut left = processed;
        let mut sources = Vec::with_capacity(selected.len());
        let mut stages = StageCounts::default();
        let mut diff = DiffCounts::default();
        for (i, src) in selected.iter().enumerate() {
            let share = if i + 1 == selected.len() {
                left.min(src.to_process())
            } else if total == 0 {
                0
            } else {
                ((processed as u128 * src.to_process() as u128) / total as u128) as u64
            };
            left -= share.min(left);
            let failed = share.checked_div(c.error_every).unwrap_or(0);
            let skipped = share * c.skip_percent / 100;
            let remaining = src.to_process() - share;
            let readers = src.device.initial_readers() as u32;
            let analyzed = if running && !walking {
                remaining.min(u64::from(readers) * 2)
            } else {
                0
            };
            let probed = if running && !walking {
                (remaining - analyzed).min(64)
            } else {
                0
            };
            let st = if walking {
                let seen = src.files * s.elapsed_ms / c.walk_ms.max(1);
                StageCounts {
                    discovered: seen,
                    ..StageCounts::default()
                }
            } else {
                StageCounts {
                    discovered: remaining - analyzed - probed,
                    probed,
                    analyzed,
                    thumbed: src.unchanged + share - failed - skipped,
                    skipped,
                    failed,
                }
            };
            let d = if walking {
                DiffCounts::default()
            } else {
                DiffCounts {
                    new: src.new_files(),
                    changed: src.changed,
                    unchanged: src.unchanged,
                    missing: src.missing,
                    returning: 0,
                    moved: 0,
                    reclassified: 0,
                    quarantined: 0,
                    redated: 0,
                    needs_refresh: 0,
                    refreshed: 0,
                    reanalyzed: 0,
                    ..DiffCounts::default()
                }
            };
            stages.add(&st);
            diff.add(&d);
            let current_dirs = if running {
                (0..readers.min(4))
                    .map(|r| {
                        format!(
                            "{}/{:04}/{:02}-{}",
                            if src.device == DeviceClass::Network {
                                "Family/Photos"
                            } else {
                                "Pictures/Imports"
                            },
                            2009 + (share / 5_000 + u64::from(r)) % 16,
                            1 + (share / 400 + u64::from(r) * 3) % 12,
                            ["Holiday", "Birthday", "Garden", "Beach"][r as usize % 4]
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            sources.push(SourceProgress {
                source: src.id,
                device: src.device,
                files: src.files,
                to_process: src.to_process(),
                processed: share,
                bytes_read: share * c.bytes_per_file,
                stages: st,
                diff: d,
                readers,
                current_dirs,
                incomplete: !src.online,
            });
        }
        let photo_count = stages.thumbed.saturating_sub(
            selected
                .iter()
                .map(|x| x.unchanged)
                .sum::<u64>()
                .min(stages.thumbed),
        );
        let first = processed.saturating_sub(photo_count.min(64));
        let recent_photos = (first..first + photo_count.min(64))
            .map(|n| RecentPhoto {
                source: selected.first().map_or(SourceId(1), |x| x.id),
                path: rel(format!("Pictures/2019/IMG_{n:05}.JPG")),
                class: PhotoClass::Photo,
                content: ContentId::full(digest_of(n)),
            })
            .collect();
        let errors = stages.failed;
        let recent_errors = (errors.saturating_sub(32)..errors)
            .map(|n| RecentError {
                source: selected.first().map_or(SourceId(1), |x| x.id),
                path: rel(format!("Private/locked/scan_{n:04}.jpg")),
                stage: Stage::Probed,
                message: "Permission denied (os error 13)".into(),
            })
            .collect();
        let analysis_s = s.elapsed_ms.saturating_sub(c.walk_ms) / 1000;
        let throughput = (analysis_s.saturating_sub(60)..analysis_s)
            .map(|i| {
                // A gentle deterministic wobble so the sparkline reads as live data.
                let wobble =
                    1.0 + 0.12 * ((i as f64) * 0.7).sin() + 0.05 * ((i as f64) * 2.3).cos();
                let fps = c.files_per_s * wobble;
                ThroughputSample {
                    at_ms: c.walk_ms + (i + 1) * 1000,
                    files_per_s: fps,
                    bytes_per_s: fps * c.bytes_per_file as f64,
                }
            })
            .collect();
        ProgressSnapshot {
            run,
            status: s.status,
            phase: match (s.status, walking) {
                (RunStatus::Running | RunStatus::Paused, true) => ScanPhase::Walking,
                (RunStatus::Running | RunStatus::Paused, false) => ScanPhase::Analyzing,
                _ => ScanPhase::Done,
            },
            elapsed_ms: s.elapsed_ms,
            stages,
            diff,
            sources,
            to_process: total,
            processed,
            bytes_to_read: total * c.bytes_per_file,
            bytes_read: processed * c.bytes_per_file,
            files_per_s: if running { c.files_per_s } else { 0.0 },
            bytes_per_s: if running {
                c.files_per_s * c.bytes_per_file as f64
            } else {
                0.0
            },
            eta_ms: None,
            recent_photos,
            recent_errors,
            errors,
            throughput,
            active_workers: if running { 12 } else { 0 },
            buffered_bytes: if running { 48 << 20 } else { 0 },
        }
    }

    /// The summary of the current run, once it has ended.
    pub fn overview(&self) -> Option<RunOverview> {
        let s = self.lock();
        let run = s.run?;
        if matches!(s.status, RunStatus::Running | RunStatus::Paused) {
            return None;
        }
        let snap = Self::build(&s, run);
        Some(RunOverview {
            run,
            status: s.status,
            sources: s.selected.clone(),
            started_at_ms: s.started_at_ms,
            ended_at_ms: Some(s.started_at_ms + s.elapsed_ms as i64),
            elapsed_ms: s.elapsed_ms,
            processed: snap.processed,
            bytes_read: snap.bytes_read,
            photos: snap.stages.thumbed * 9 / 10,
            videos: snap.stages.thumbed / 10,
            duplicate_groups: snap.stages.thumbed / 7,
            skipped: snap.stages.skipped,
            failed: snap.stages.failed,
            errors: snap.errors,
            new: snap.diff.new,
            changed: snap.diff.changed,
            unchanged: snap.diff.unchanged,
            missing: snap.diff.missing,
            redated: snap.diff.redated,
            needs_refresh: snap.diff.still_needs_refresh(),
            refreshed: snap.diff.refreshed,
            incomplete_sources: snap
                .sources
                .iter()
                .filter(|x| x.incomplete)
                .map(|x| x.source)
                .collect(),
            redecoded: 0,
            undecoded: s.undecoded.clone(),
        })
    }

    /// Files waiting for a decoder that the simulated run's summary reports (scripted screenshots).
    pub fn set_undecoded(&self, u: Vec<ps_model::Undecoded>) {
        self.lock().undecoded = u;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_snapshots_at_the_configured_rate() {
        let mut cfg = FakeConfig::two_sources(1_000.0);
        cfg.walk_ms = 0;
        let fake = FakeScanService::new(cfg);
        assert!(fake.snapshot().is_none(), "no run yet");
        fake.start(&[]).ok();
        // 10 samples of 100 ms = 1 s at 1,000 files/s.
        let mut last = None;
        for _ in 0..10 {
            last = fake.snapshot();
        }
        let snap = last.unwrap_or_else(|| panic!("a snapshot"));
        assert_eq!(snap.processed, 1_000);
        assert_eq!(snap.elapsed_ms, 1_000);
        assert_eq!(fake.snapshots_taken(), 11);
        // Doubling the rate doubles progress per sample, not the snapshot size.
        fake.reconfigure(|c| c.files_per_s = 2_000.0);
        let before = fake.peek().map(|s| s.processed).unwrap_or_default();
        let after = fake.snapshot().map(|s| s.processed).unwrap_or_default();
        assert_eq!(after, 2_200);
        assert!(after > before);
        assert_eq!(snap.recent_photos.len(), 64);
    }

    #[test]
    fn pause_stops_counts_and_resume_continues() {
        let fake = FakeScanService::new(FakeConfig::two_sources(500.0));
        let run = fake.start(&[]).unwrap_or(ScanRunId(0));
        fake.advance(2_500);
        fake.pause().ok();
        let a = fake.snapshot().map(|s| s.processed);
        let b = fake.snapshot().map(|s| s.processed);
        assert_eq!(a, b);
        fake.cancel().ok();
        assert_eq!(fake.peek().map(|s| s.status), Some(RunStatus::Cancelled));
        fake.resume_run(run).ok();
        let c = fake.snapshot().map(|s| s.processed).unwrap_or_default();
        assert!(c > b.unwrap_or_default(), "resumed where it stopped");
    }

    #[test]
    fn rescan_diff_is_known_after_the_walk_before_analysis() {
        let mut cfg = FakeConfig::two_sources(100.0);
        cfg.sources = vec![FakeSource::new(1, "a", DeviceClass::Rotational, 100).rescan(3, 0, 1)];
        let fake = FakeScanService::new(cfg);
        fake.start(&[]).ok();
        fake.advance(500);
        let s = fake.peek().unwrap_or_else(|| panic!("snapshot"));
        assert_eq!((s.diff.new, s.diff.missing, s.processed), (3, 1, 0));
    }
}
