//! Progress as coalesced snapshots (ADR-0019): atomic counters per stage and per source, plus a small
//! mutex-guarded "recent" ring (last 64 photos, last 32 errors, the folder each reader is in, 1 s throughput
//! samples). [`Progress::snapshot`] copies them; its cost does not depend on how many files were processed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use ps_model::{
    DeviceClass, DiffCounts, ProgressSnapshot, RecentError, RecentPhoto, RunStatus, ScanPhase,
    ScanRunId, SourceId, SourceProgress, Stage, StageCounts, ThroughputSample,
};

/// Throughput samples kept (one per second: the last minute).
const SAMPLES: usize = 60;
/// Samples averaged for the current rate.
const RATE_SAMPLES: usize = 5;

#[derive(Debug, Default)]
struct Stages([AtomicU64; 6]);

impl Stages {
    fn idx(s: Stage) -> usize {
        match s {
            Stage::Discovered => 0,
            Stage::Probed => 1,
            Stage::Analyzed => 2,
            Stage::Thumbed => 3,
            Stage::Skipped => 4,
            Stage::Failed => 5,
        }
    }

    fn add(&self, s: Stage, n: i64) {
        let a = &self.0[Self::idx(s)];
        if n >= 0 {
            a.fetch_add(n as u64, Ordering::Relaxed);
        } else {
            let _ = a.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(n.unsigned_abs()))
            });
        }
    }

    fn load(&self) -> StageCounts {
        let mut c = StageCounts::default();
        for s in Stage::ALL {
            *c.get_mut(s) = self.0[Self::idx(s)].load(Ordering::Relaxed);
        }
        c
    }

    fn set(&self, c: &StageCounts) {
        for s in Stage::ALL {
            self.0[Self::idx(s)].store(c.get(s), Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Default)]
struct Diff {
    new: AtomicU64,
    changed: AtomicU64,
    unchanged: AtomicU64,
    missing: AtomicU64,
    returning: AtomicU64,
    moved: AtomicU64,
    reclassified: AtomicU64,
    quarantined: AtomicU64,
}

impl Diff {
    fn load(&self) -> DiffCounts {
        DiffCounts {
            new: self.new.load(Ordering::Relaxed),
            changed: self.changed.load(Ordering::Relaxed),
            unchanged: self.unchanged.load(Ordering::Relaxed),
            missing: self.missing.load(Ordering::Relaxed),
            returning: self.returning.load(Ordering::Relaxed),
            moved: self.moved.load(Ordering::Relaxed),
            reclassified: self.reclassified.load(Ordering::Relaxed),
            quarantined: self.quarantined.load(Ordering::Relaxed),
        }
    }

    fn store(&self, d: &DiffCounts) {
        self.new.store(d.new, Ordering::Relaxed);
        self.changed.store(d.changed, Ordering::Relaxed);
        self.unchanged.store(d.unchanged, Ordering::Relaxed);
        self.missing.store(d.missing, Ordering::Relaxed);
        self.returning.store(d.returning, Ordering::Relaxed);
        self.moved.store(d.moved, Ordering::Relaxed);
        self.reclassified.store(d.reclassified, Ordering::Relaxed);
        self.quarantined.store(d.quarantined, Ordering::Relaxed);
    }
}

/// Counters of one source.
#[derive(Debug)]
pub struct SourceCounters {
    pub source: SourceId,
    device: AtomicU8,
    files: AtomicU64,
    to_process: AtomicU64,
    processed: AtomicU64,
    bytes_read: AtomicU64,
    bytes_to_read: AtomicU64,
    stages: Stages,
    diff: Diff,
    readers: AtomicU32,
    incomplete: AtomicBool,
}

impl SourceCounters {
    fn new(source: SourceId) -> Self {
        SourceCounters {
            source,
            device: AtomicU8::new(0),
            files: AtomicU64::new(0),
            to_process: AtomicU64::new(0),
            processed: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            bytes_to_read: AtomicU64::new(0),
            stages: Stages::default(),
            diff: Diff::default(),
            readers: AtomicU32::new(0),
            incomplete: AtomicBool::new(false),
        }
    }
}

fn device_code(d: DeviceClass) -> u8 {
    match d {
        DeviceClass::SolidState => 1,
        DeviceClass::Rotational => 2,
        DeviceClass::Network => 3,
        DeviceClass::Unknown => 0,
    }
}

fn device_from(c: u8) -> DeviceClass {
    match c {
        1 => DeviceClass::SolidState,
        2 => DeviceClass::Rotational,
        3 => DeviceClass::Network,
        _ => DeviceClass::Unknown,
    }
}

fn status_code(s: RunStatus) -> u8 {
    match s {
        RunStatus::Running => 0,
        RunStatus::Paused => 1,
        RunStatus::Cancelled => 2,
        RunStatus::Completed => 3,
        RunStatus::Interrupted => 4,
    }
}

fn phase_code(p: ScanPhase) -> u8 {
    match p {
        ScanPhase::Starting => 0,
        ScanPhase::Walking => 1,
        ScanPhase::Analyzing => 2,
        ScanPhase::Finishing => 3,
        ScanPhase::Done => 4,
    }
}

#[derive(Debug)]
struct Recent {
    photos: VecDeque<RecentPhoto>,
    errors: VecDeque<RecentError>,
    /// Per source, per reader: the folder being read.
    dirs: Vec<Vec<Option<String>>>,
    samples: VecDeque<ThroughputSample>,
    last_sample: (Instant, u64, u64),
}

/// Live progress of one run.
#[derive(Debug)]
pub struct Progress {
    run: ScanRunId,
    start: Instant,
    status: AtomicU8,
    phase: AtomicU8,
    sources: Vec<SourceCounters>,
    files_done: AtomicU64,
    errors: AtomicU64,
    recent: Mutex<Recent>,
}

impl Progress {
    pub fn new(run: ScanRunId, sources: &[SourceId]) -> Progress {
        let start = Instant::now();
        Progress {
            run,
            start,
            status: AtomicU8::new(status_code(RunStatus::Running)),
            phase: AtomicU8::new(phase_code(ScanPhase::Starting)),
            sources: sources.iter().map(|s| SourceCounters::new(*s)).collect(),
            files_done: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            recent: Mutex::new(Recent {
                photos: VecDeque::with_capacity(ProgressSnapshot::RECENT_PHOTOS),
                errors: VecDeque::with_capacity(ProgressSnapshot::RECENT_ERRORS),
                dirs: vec![Vec::new(); sources.len()],
                samples: VecDeque::with_capacity(SAMPLES),
                last_sample: (start, 0, 0),
            }),
        }
    }

    pub fn run(&self) -> ScanRunId {
        self.run
    }

    pub fn source(&self, idx: usize) -> &SourceCounters {
        &self.sources[idx]
    }

    pub fn set_status(&self, s: RunStatus) {
        self.status.store(status_code(s), Ordering::Release);
    }

    pub fn status(&self) -> RunStatus {
        let c = self.status.load(Ordering::Acquire);
        RunStatus::ALL
            .into_iter()
            .find(|s| status_code(*s) == c)
            .unwrap_or(RunStatus::Running)
    }

    pub fn set_phase(&self, p: ScanPhase) {
        self.phase.store(phase_code(p), Ordering::Release);
    }

    pub fn phase(&self) -> ScanPhase {
        let c = self.phase.load(Ordering::Acquire);
        [
            ScanPhase::Starting,
            ScanPhase::Walking,
            ScanPhase::Analyzing,
            ScanPhase::Finishing,
            ScanPhase::Done,
        ]
        .into_iter()
        .find(|p| phase_code(*p) == c)
        .unwrap_or(ScanPhase::Starting)
    }

    pub fn set_device(&self, idx: usize, d: DeviceClass) {
        self.sources[idx]
            .device
            .store(device_code(d), Ordering::Relaxed);
    }

    /// Record a source's diff: counts, present files, stage distribution and the work to do.
    pub fn set_diff(
        &self,
        idx: usize,
        diff: &DiffCounts,
        present: u64,
        stages: &StageCounts,
        to_process: u64,
        bytes: u64,
    ) {
        let s = &self.sources[idx];
        s.diff.store(diff);
        s.files.store(present, Ordering::Relaxed);
        s.stages.set(stages);
        s.to_process.store(to_process, Ordering::Relaxed);
        s.bytes_to_read.store(bytes, Ordering::Relaxed);
    }

    /// A file of source `idx` moved from stage `from` to `to`.
    pub fn transition(&self, idx: usize, from: Stage, to: Stage) {
        if from == to {
            return;
        }
        let s = &self.sources[idx];
        s.stages.add(from, -1);
        s.stages.add(to, 1);
    }

    /// A present file of source `idx` disappeared during the run.
    pub fn vanished(&self, idx: usize, stage: Stage) {
        let s = &self.sources[idx];
        s.stages.add(stage, -1);
        s.files.fetch_sub(1, Ordering::Relaxed);
        s.diff.missing.fetch_add(1, Ordering::Relaxed);
    }

    /// A new file was re-identified as a moved one (it is no longer counted missing).
    pub fn moved(&self, idx: usize) {
        let d = &self.sources[idx].diff;
        d.moved.fetch_add(1, Ordering::Relaxed);
        let _ = d
            .missing
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }

    /// A file of source `idx` needs no more work this run.
    pub fn file_done(&self, idx: usize) {
        self.sources[idx].processed.fetch_add(1, Ordering::Relaxed);
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn bytes_read(&self, idx: usize, n: u64) {
        self.sources[idx].bytes_read.fetch_add(n, Ordering::Relaxed);
    }

    pub fn set_readers(&self, idx: usize, n: usize) {
        self.sources[idx].readers.store(n as u32, Ordering::Relaxed);
    }

    pub fn readers(&self, idx: usize) -> usize {
        self.sources[idx].readers.load(Ordering::Relaxed) as usize
    }

    pub fn set_incomplete(&self, idx: usize) {
        self.sources[idx].incomplete.store(true, Ordering::Relaxed);
    }

    pub fn is_incomplete(&self, idx: usize) -> bool {
        self.sources[idx].incomplete.load(Ordering::Relaxed)
    }

    /// Set the folder reader `reader` of source `idx` is in (`None`: idle).
    pub fn set_dir(&self, idx: usize, reader: usize, dir: Option<String>) {
        let mut r = self.recent.lock();
        let v = &mut r.dirs[idx];
        if v.len() <= reader {
            v.resize(reader + 1, None);
        }
        v[reader] = dir;
    }

    pub fn photo(&self, p: RecentPhoto) {
        let mut r = self.recent.lock();
        if r.photos.len() == ProgressSnapshot::RECENT_PHOTOS {
            r.photos.pop_front();
        }
        r.photos.push_back(p);
    }

    pub fn error(&self, e: RecentError) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        let mut r = self.recent.lock();
        if r.errors.len() == ProgressSnapshot::RECENT_ERRORS {
            r.errors.pop_front();
        }
        r.errors.push_back(e);
    }

    fn totals(&self) -> (u64, u64) {
        let files = self.files_done.load(Ordering::Relaxed);
        let bytes = self
            .sources
            .iter()
            .map(|s| s.bytes_read.load(Ordering::Relaxed))
            .sum();
        (files, bytes)
    }

    /// Take a throughput sample when at least a second passed since the last one.
    pub fn sample(&self) {
        let now = Instant::now();
        let (files, bytes) = self.totals();
        let mut r = self.recent.lock();
        let (t0, f0, b0) = r.last_sample;
        let dt = now.duration_since(t0);
        if dt < Duration::from_secs(1) {
            return;
        }
        let s = dt.as_secs_f64();
        if r.samples.len() == SAMPLES {
            r.samples.pop_front();
        }
        r.samples.push_back(ThroughputSample {
            at_ms: now.duration_since(self.start).as_millis() as u64,
            files_per_s: files.saturating_sub(f0) as f64 / s,
            bytes_per_s: bytes.saturating_sub(b0) as f64 / s,
        });
        r.last_sample = (now, files, bytes);
    }

    /// Copy the counters (ADR-0019). `active_workers` and `buffered_bytes` come from the run's control and
    /// byte budget.
    pub fn snapshot(&self, active_workers: u32, buffered_bytes: u64) -> ProgressSnapshot {
        let mut stages = StageCounts::default();
        let mut diff = DiffCounts::default();
        let (mut to_process, mut bytes_to_read) = (0, 0);
        let r = self.recent.lock();
        let sources: Vec<SourceProgress> = self
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let st = s.stages.load();
                let d = s.diff.load();
                stages.add(&st);
                diff.add(&d);
                to_process += s.to_process.load(Ordering::Relaxed);
                bytes_to_read += s.bytes_to_read.load(Ordering::Relaxed);
                SourceProgress {
                    source: s.source,
                    device: device_from(s.device.load(Ordering::Relaxed)),
                    files: s.files.load(Ordering::Relaxed),
                    to_process: s.to_process.load(Ordering::Relaxed),
                    processed: s.processed.load(Ordering::Relaxed),
                    bytes_read: s.bytes_read.load(Ordering::Relaxed),
                    stages: st,
                    diff: d,
                    readers: s.readers.load(Ordering::Relaxed),
                    current_dirs: r.dirs[i].iter().flatten().cloned().collect(),
                    incomplete: s.incomplete.load(Ordering::Relaxed),
                }
            })
            .collect();
        let (processed, bytes_read) = self.totals();
        let recent: Vec<&ThroughputSample> = r.samples.iter().rev().take(RATE_SAMPLES).collect();
        let n = recent.len().max(1) as f64;
        let files_per_s = recent.iter().map(|s| s.files_per_s).sum::<f64>() / n;
        let bytes_per_s = recent.iter().map(|s| s.bytes_per_s).sum::<f64>() / n;
        let eta_ms = if bytes_per_s > 0.0 && bytes_to_read > bytes_read {
            Some(((bytes_to_read - bytes_read) as f64 / bytes_per_s * 1000.0) as u64)
        } else if files_per_s > 0.0 && to_process > processed {
            Some(((to_process - processed) as f64 / files_per_s * 1000.0) as u64)
        } else if to_process <= processed && !recent.is_empty() {
            Some(0)
        } else {
            None
        };
        ProgressSnapshot {
            run: self.run,
            status: self.status(),
            phase: self.phase(),
            elapsed_ms: self.start.elapsed().as_millis() as u64,
            stages,
            diff,
            sources,
            to_process,
            processed,
            bytes_to_read,
            bytes_read,
            files_per_s,
            bytes_per_s,
            eta_ms,
            recent_photos: r.photos.iter().cloned().collect(),
            recent_errors: r.errors.iter().cloned().collect(),
            errors: self.errors.load(Ordering::Relaxed),
            throughput: r.samples.iter().copied().collect(),
            active_workers,
            buffered_bytes,
        }
    }
}
