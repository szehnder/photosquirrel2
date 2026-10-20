//! One scan run: walk + diff, then the staged pipeline (ADR-0008, ADR-0009, ADR-0010).
//!
//! ```text
//!  per source:  walk thread ──diff──▶ catalog (discovered rows, missing marks)
//!  per source:  lane dispatcher ──bounded(2×16)──▶ 1..16 reader threads
//!                 readers: open (read-only) → 256 KB probe → classify → `probed` / `skipped`
//!                          candidates: byte permit → rest of the file into the same buffer
//!  shared:      ──bounded(2×cpu)──▶ rayon pool (cores − 1): identity, metadata, analysis decode,
//!                          perceptual hashes, thumbnail, all from that one buffer
//!               ──bounded(256)──▶ persist thread: thumbs.db commit, then `analyzed` + `thumbed`
//!                          (and move re-identification) through the catalog writer
//! ```
//!
//! Every stage reaches the catalog together with its results through the single writer, so a crash or a
//! cancel at any instant leaves a consistent checkpoint; resume simply walks again.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, RecvTimeoutError, SendTimeoutError, Sender, bounded};
use parking_lot::Mutex;

use ps_catalog::{AnalysisResult, Catalog, Command, FileFailure, FileKey, ProbeResult, ThumbStore};
use ps_media::FolderHints;
use ps_media::decode::DecodeError;
use ps_model::limits::ANALYSIS_BUFFER_MAX_BYTES;
use ps_model::{
    ContentId, DeviceClass, MediaKind, PhotoClass, ProbeRecord, RecentError, RecentPhoto, RelPath,
    RunStatus, ScanPhase, ScanRunId, SourceId, SourceStatus, Stage, ThumbnailRecord,
};

use crate::control::{Busy, Cancelled, Control};
use crate::diff::{DiffOutput, Todo, diff};
use crate::fs::{SourceFile, fingerprint};
use crate::lane::{LaneController, MAX_READERS, Window, p90};
use crate::memory::{BufferPermit, ByteBudget};
use crate::options::ScanOptions;
use crate::progress::Progress;
use crate::report::{RunSummary, SourceSummary};
use crate::walk::{WalkError, walk};

/// Read chunk between cancel checks (1 MB).
pub const CHUNK: usize = 1024 * 1024;
/// Upsert rows per writer command during the diff.
const UPSERT_CHUNK: usize = 2_000;
/// Persist batch: items or delay, whichever first.
const PERSIST_BATCH: usize = 64;
const PERSIST_DELAY: Duration = Duration::from_millis(50);
/// Poll interval of blocking queue operations (bounds pause/cancel reaction time).
const POLL: Duration = Duration::from_millis(20);

/// A source taking part in a run.
#[derive(Debug)]
pub(crate) struct RunSource {
    pub id: SourceId,
    pub root: PathBuf,
    pub device: DeviceClass,
    /// Offline before the run started.
    pub offline: bool,
    /// The root disappeared during the run.
    pub lost: AtomicBool,
    pub symlinks_skipped: AtomicU64,
    pub walk_errors: AtomicU64,
}

/// Everything the threads of one run share.
pub(crate) struct RunCtx {
    pub run: ScanRunId,
    pub catalog: Arc<Catalog>,
    pub thumbs: Arc<ThumbStore>,
    pub opts: ScanOptions,
    pub ctl: Arc<Control>,
    pub progress: Arc<Progress>,
    pub budget: Arc<ByteBudget>,
    pub sources: Vec<RunSource>,
    pub changed_during_scan: AtomicU64,
    pub retries_exhausted: AtomicU64,
    /// The post-scan regroup engine (shared by every run of the scanner).
    pub groups: Arc<crate::post::GroupEngine>,
}

impl RunCtx {
    fn submit(&self, cmd: Command) {
        if let Err(e) = self.catalog.writer().submit(cmd) {
            log::error!("scan run {}: catalog writer: {e}", self.run);
        }
    }

    fn fail(&self, idx: usize, path: &RelPath, from: Stage, attempted: Stage, message: String) {
        let key = FileKey::new(self.sources[idx].id, path.clone());
        self.submit(Command::Failed(vec![FileFailure {
            key,
            stage: attempted,
            message: message.clone(),
            run: Some(self.run),
        }]));
        self.progress.transition(idx, from, Stage::Failed);
        self.progress.error(RecentError {
            source: self.sources[idx].id,
            path: path.clone(),
            stage: attempted,
            message,
        });
        self.progress.file_done(idx);
    }

    /// The root of source `idx` is gone: stop its lane, mark it offline (once).
    fn lose(&self, idx: usize) {
        let s = &self.sources[idx];
        if !s.lost.swap(true, Ordering::AcqRel) {
            log::warn!(
                "scan run {}: source {} ({}) went offline",
                self.run,
                s.id,
                s.root.display()
            );
            self.progress.set_incomplete(idx);
            self.submit(Command::SetSourceStatus {
                source: s.id,
                status: SourceStatus::Offline,
            });
        }
    }

    fn root_available(&self, idx: usize) -> bool {
        self.opts
            .fs
            .metadata(&self.sources[idx].root)
            .is_ok_and(|m| m.is_dir())
    }
}

/// Execute a run to its end (completed or cancelled) and record its summary.
pub(crate) fn execute(ctx: Arc<RunCtx>) -> RunSummary {
    let started = Instant::now();
    ctx.progress.set_phase(ScanPhase::Walking);
    for (i, s) in ctx.sources.iter().enumerate() {
        ctx.progress.set_device(i, s.device);
        if s.offline {
            ctx.progress.set_incomplete(i);
        }
    }

    // 1. Walk and diff every source in parallel (listing + stat only).
    let diffs: Vec<Option<DiffOutput>> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..ctx.sources.len())
            .map(|i| {
                let ctx = &ctx;
                std::thread::Builder::new()
                    .name(format!("ps-scan-walk-{i}"))
                    .spawn_scoped(sc, move || walk_and_diff(ctx, i))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.ok().and_then(|h| h.join().ok().flatten()))
            .collect()
    });

    // 2. The staged pipeline over the files to process.
    if !ctx.ctl.is_cancelled() {
        ctx.progress.set_phase(ScanPhase::Analyzing);
        run_pipeline(&ctx, diffs);
    }

    // 3. Flush and record the outcome.
    ctx.progress.set_phase(ScanPhase::Finishing);
    if let Err(e) = ctx.catalog.writer().flush() {
        log::error!("scan run {}: flush: {e}", ctx.run);
    }
    let status = if ctx.ctl.is_cancelled() {
        RunStatus::Cancelled
    } else {
        RunStatus::Completed
    };
    // 4. Post stage: regroup the new and changed files (a cancelled run regroups when resumed).
    let grouping = if status == RunStatus::Completed {
        match ctx.groups.refresh(&ctx.catalog) {
            Ok(r) => Some(r),
            Err(e) => {
                log::error!("scan run {}: regroup: {e}", ctx.run);
                None
            }
        }
    } else {
        None
    };
    let mut summary = summarize(&ctx, status, started.elapsed());
    summary.grouping = grouping;
    ctx.submit(Command::SetRunStatus {
        run: ctx.run,
        status,
        stats: Some(summary.to_json()),
    });
    if let Err(e) = ctx.catalog.writer().flush() {
        log::error!("scan run {}: final flush: {e}", ctx.run);
    }
    ctx.progress.set_status(status);
    ctx.progress.set_phase(ScanPhase::Done);
    summary
}

fn summarize(ctx: &RunCtx, status: RunStatus, elapsed: Duration) -> RunSummary {
    let snap = ctx.progress.snapshot(0, 0);
    let sources: Vec<SourceSummary> = snap
        .sources
        .iter()
        .zip(&ctx.sources)
        .map(|(p, s)| SourceSummary {
            source: p.source,
            device: p.device,
            files: p.files,
            to_process: p.to_process,
            processed: p.processed,
            bytes_read: p.bytes_read,
            stages: p.stages,
            diff: p.diff,
            incomplete: p.incomplete,
            symlinks_skipped: s.symlinks_skipped.load(Ordering::Relaxed),
            walk_errors: s.walk_errors.load(Ordering::Relaxed),
        })
        .collect();
    RunSummary {
        run: ctx.run,
        status,
        elapsed_ms: elapsed.as_millis() as u64,
        stages: snap.stages,
        diff: snap.diff,
        incomplete_sources: sources
            .iter()
            .filter(|s| s.incomplete)
            .map(|s| s.source)
            .collect(),
        sources,
        processed: snap.processed,
        bytes_read: snap.bytes_read,
        errors: snap.errors,
        changed_during_scan: ctx.changed_during_scan.load(Ordering::Relaxed),
        retries_exhausted: ctx.retries_exhausted.load(Ordering::Relaxed),
        grouping: None,
    }
}

fn walk_and_diff(ctx: &RunCtx, idx: usize) -> Option<DiffOutput> {
    let s = &ctx.sources[idx];
    if s.offline {
        return None;
    }
    let w = match walk(&ctx.opts.fs, &s.root, &ctx.ctl, |d| {
        ctx.progress.set_dir(idx, 0, Some(d.to_owned()));
    }) {
        Ok(w) => w,
        Err(WalkError::Cancelled) => return None,
        Err(WalkError::RootUnavailable(e)) => {
            log::warn!("source {} unavailable: {e}", s.id);
            ctx.lose(idx);
            return None;
        }
    };
    ctx.progress.set_dir(idx, 0, None);
    s.symlinks_skipped
        .store(w.symlinks_skipped, Ordering::Relaxed);
    s.walk_errors
        .store(w.errors.len() as u64, Ordering::Relaxed);
    for (path, err) in &w.errors {
        if let Some(p) = RelPath::from_bytes(path.as_bytes().to_vec()) {
            ctx.progress.error(RecentError {
                source: s.id,
                path: p,
                stage: Stage::Discovered,
                message: err.clone(),
            });
        }
    }
    let mut d = match diff(s.id, w.entries, ctx.catalog.read()) {
        Ok(d) => d,
        Err(e) => {
            log::error!("scan run {}: diff of source {}: {e}", ctx.run, s.id);
            return None;
        }
    };
    for chunk in std::mem::take(&mut d.upserts).chunks(UPSERT_CHUNK) {
        ctx.submit(Command::UpsertFiles(chunk.to_vec()));
    }
    if !d.newly_missing.is_empty() {
        ctx.submit(Command::MarkMissing {
            files: std::mem::take(&mut d.newly_missing),
            at_ms: now_ms(),
        });
    }
    ctx.retries_exhausted
        .fetch_add(d.retries_exhausted, Ordering::Relaxed);
    ctx.progress.set_diff(
        idx,
        &d.counts,
        d.present,
        &d.stages,
        d.todo.len() as u64,
        d.bytes_to_process,
    );
    Some(d)
}

/// Bytes of one candidate, as handed from a reader to a CPU worker.
enum Content {
    /// The whole file (<= 64 MB), the probe prefix being its head.
    Buffer(Vec<u8>),
    /// Larger files: identity streamed by the reader; only the probe prefix is kept (metadata).
    Streamed {
        identity: ContentId,
        prefix: Vec<u8>,
    },
}

struct CpuJob {
    idx: usize,
    path: RelPath,
    abs: PathBuf,
    is_new: bool,
    class: PhotoClass,
    probe: Box<ProbeRecord>,
    content: Content,
    mtime: Option<SystemTime>,
    _permit: BufferPermit,
}

enum Persist {
    Done {
        idx: usize,
        path: RelPath,
        is_new: bool,
        class: PhotoClass,
        analysis: Box<AnalysisResult>,
        thumb: ThumbnailRecord,
    },
    Failed {
        idx: usize,
        path: RelPath,
        message: String,
    },
}

fn run_pipeline(ctx: &Arc<RunCtx>, diffs: Vec<Option<DiffOutput>>) {
    let cpu = ctx.opts.cpu_threads.max(1);
    let (cpu_tx, cpu_rx) = bounded::<CpuJob>(cpu * 2);
    let (out_tx, out_rx) = bounded::<Persist>(256);
    let mut moves: Vec<HashMap<ContentId, Vec<RelPath>>> = Vec::new();
    let mut todos: Vec<Vec<Todo>> = Vec::new();
    for d in diffs
        .into_iter()
        .chain(std::iter::repeat_with(|| None))
        .take(ctx.sources.len())
    {
        let d = d.unwrap_or_default();
        moves.push(d.move_candidates);
        todos.push(d.todo);
    }

    std::thread::scope(|sc| {
        // Persist thread.
        let persist = std::thread::Builder::new()
            .name("ps-scan-persist".into())
            .spawn_scoped(sc, {
                let ctx = ctx.clone();
                move || persist_loop(&ctx, out_rx, moves)
            });

        // CPU pool: cores - 1 workers pulling from the bounded queue.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(cpu)
            .thread_name(|i| format!("ps-scan-cpu-{i}"))
            .build();
        let workers = std::thread::Builder::new()
            .name("ps-scan-cpu".into())
            .spawn_scoped(sc, {
                let ctx = ctx.clone();
                move || match pool {
                    Ok(pool) => pool.scope(|s| {
                        for _ in 0..cpu {
                            let (ctx, rx, tx) = (ctx.clone(), cpu_rx.clone(), out_tx.clone());
                            s.spawn(move |_| worker_loop(&ctx, &rx, &tx));
                        }
                    }),
                    Err(e) => {
                        log::error!("cannot build the CPU pool ({e}); analyzing on one thread");
                        worker_loop(&ctx, &cpu_rx, &out_tx);
                    }
                }
            });

        // One lane per source.
        let lanes: Vec<_> = todos
            .into_iter()
            .enumerate()
            .filter(|(_, t)| !t.is_empty())
            .map(|(idx, todo)| {
                let (ctx, tx) = (ctx.clone(), cpu_tx.clone());
                std::thread::Builder::new()
                    .name(format!("ps-scan-lane-{idx}"))
                    .spawn_scoped(sc, move || run_lane(&ctx, idx, todo, tx))
            })
            .collect();
        drop(cpu_tx);

        // Sample throughput while the lanes run.
        loop {
            let running = lanes
                .iter()
                .any(|l| l.as_ref().is_ok_and(|h| !h.is_finished()));
            ctx.progress.sample();
            if !running {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for l in lanes.into_iter().flatten() {
            let _ = l.join();
        }
        if let Ok(w) = workers {
            let _ = w.join();
        }
        if let Ok(p) = persist {
            let _ = p.join();
        }
        ctx.progress.sample();
    });
}

/// Shared state of one lane's readers.
struct LaneShared {
    target: AtomicUsize,
    dispatch_done: AtomicBool,
    /// (bytes, chunk latencies) of the current window.
    window: Mutex<(u64, Vec<Duration>)>,
}

fn run_lane(ctx: &Arc<RunCtx>, idx: usize, todo: Vec<Todo>, cpu_tx: Sender<CpuJob>) {
    let s = &ctx.sources[idx];
    let max = ctx.opts.max_readers.clamp(1, MAX_READERS);
    let mut controller = LaneController::new(s.device.initial_readers().min(max));
    let shared = Arc::new(LaneShared {
        target: AtomicUsize::new(controller.readers()),
        dispatch_done: AtomicBool::new(false),
        window: Mutex::new((0, Vec::new())),
    });
    ctx.progress.set_readers(idx, controller.readers());
    let (tx, rx) = bounded::<Todo>(2 * MAX_READERS);
    std::thread::scope(|sc| {
        let mut readers = Vec::new();
        let spawn = |i: usize, readers: &mut Vec<_>| {
            let (ctx, shared, rx, cpu_tx) =
                (ctx.clone(), shared.clone(), rx.clone(), cpu_tx.clone());
            if let Ok(h) = std::thread::Builder::new()
                .name(format!("ps-scan-read-{idx}-{i}"))
                .spawn_scoped(sc, move || reader_loop(&ctx, idx, i, &shared, &rx, &cpu_tx))
            {
                readers.push(h);
            }
        };
        for i in 0..controller.readers() {
            spawn(i, &mut readers);
        }
        let mut window_start = Instant::now();
        let mut evaluate = |readers: &mut Vec<_>| {
            if window_start.elapsed() < ctx.opts.lane_window {
                return;
            }
            let (bytes, mut lat) = std::mem::take(&mut *shared.window.lock());
            let secs = window_start.elapsed().as_secs_f64();
            window_start = Instant::now();
            if lat.len() < 8 {
                return; // too few reads to judge
            }
            let w = Window {
                bytes_per_s: bytes as f64 / secs,
                p90_latency: p90(&mut lat),
            };
            let n = controller.observe(w).min(max);
            shared.target.store(n, Ordering::Release);
            ctx.progress.set_readers(idx, n);
            while readers.len() < n {
                let i = readers.len();
                spawn(i, readers);
            }
        };
        'files: for t in todo {
            let mut item = t;
            loop {
                if ctx.ctl.checkpoint().is_err() || s.lost.load(Ordering::Acquire) {
                    break 'files;
                }
                evaluate(&mut readers);
                match tx.send_timeout(item, POLL) {
                    Ok(()) => break,
                    Err(SendTimeoutError::Timeout(back)) => item = back,
                    Err(SendTimeoutError::Disconnected(_)) => break 'files,
                }
            }
        }
        shared.dispatch_done.store(true, Ordering::Release);
        drop(tx);
        for r in readers {
            let _ = r.join();
        }
    });
    ctx.progress.set_readers(idx, 0);
}

fn reader_loop(
    ctx: &RunCtx,
    idx: usize,
    reader: usize,
    shared: &LaneShared,
    rx: &Receiver<Todo>,
    cpu_tx: &Sender<CpuJob>,
) {
    loop {
        if ctx.ctl.is_cancelled() || ctx.sources[idx].lost.load(Ordering::Acquire) {
            break;
        }
        if reader >= shared.target.load(Ordering::Acquire) {
            // Parked by the lane controller.
            if shared.dispatch_done.load(Ordering::Acquire) && rx.is_empty() {
                break;
            }
            std::thread::sleep(POLL);
            continue;
        }
        match rx.recv_timeout(POLL) {
            Ok(t) => process_file(ctx, idx, reader, shared, t, cpu_tx),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    ctx.progress.set_dir(idx, reader, None);
}

/// Human wording of an I/O error kind for failure records.
fn io_message(e: &io::Error) -> String {
    let kind = match e.kind() {
        io::ErrorKind::PermissionDenied => "permission denied",
        io::ErrorKind::NotFound => "not found",
        io::ErrorKind::UnexpectedEof => "truncated while reading",
        io::ErrorKind::InvalidData => "changed while reading",
        _ => "read error",
    };
    format!("{kind}: {e}")
}

/// A reader over a source file that checks the control between chunks of at most 1 MB and records
/// chunk latencies for the lane controller.
struct Controlled<'a> {
    file: &'a mut SourceFile,
    busy: &'a Busy<'a>,
    ctl: &'a Control,
    shared: &'a LaneShared,
    progress: &'a Progress,
    idx: usize,
    delay: Option<Duration>,
}

impl Read for Controlled<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.busy.checkpoint().is_err() {
            return Err(io::Error::other(Cancelled));
        }
        let n = buf.len().min(CHUNK);
        let t = Instant::now();
        if let Some(d) = self.delay
            && self.ctl.sleep(d).is_err()
        {
            return Err(io::Error::other(Cancelled));
        }
        let got = self.file.read(&mut buf[..n])?;
        let dt = t.elapsed();
        {
            let mut w = self.shared.window.lock();
            w.0 += got as u64;
            w.1.push(dt);
        }
        self.progress.bytes_read(self.idx, got as u64);
        Ok(got)
    }
}

impl io::Seek for Controlled<'_> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

fn is_cancel(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|i| i.is::<Cancelled>())
}

/// Append up to `limit` bytes in total to `buf` (stops early at end of file).
fn read_into(r: &mut Controlled<'_>, buf: &mut Vec<u8>, limit: usize) -> io::Result<()> {
    while buf.len() < limit {
        let start = buf.len();
        let want = (limit - start).min(CHUNK);
        buf.resize(start + want, 0);
        match r.read(&mut buf[start..]) {
            Ok(0) => {
                buf.truncate(start);
                return Ok(());
            }
            Ok(n) => buf.truncate(start + n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => buf.truncate(start),
            Err(e) => {
                buf.truncate(start);
                return Err(e);
            }
        }
    }
    Ok(())
}

fn process_file(
    ctx: &RunCtx,
    idx: usize,
    reader: usize,
    shared: &LaneShared,
    t: Todo,
    cpu_tx: &Sender<CpuJob>,
) {
    let busy = ctx.ctl.busy();
    if busy.checkpoint().is_err() {
        return;
    }
    let src = &ctx.sources[idx];
    if src.lost.load(Ordering::Acquire) {
        return;
    }
    ctx.progress.set_dir(
        idx,
        reader,
        Some(
            t.path
                .parent()
                .map(|p| p.display().into_owned())
                .unwrap_or_default(),
        ),
    );
    if let Some(d) = ctx.opts.read_delay
        && ctx.ctl.sleep(d).is_err()
    {
        return;
    }
    let rel = t.path.to_path_buf();
    let abs = t.path.under(&src.root);
    let mut file = match ctx.opts.fs.open(&abs) {
        Ok(f) => f,
        Err(e) => {
            if !ctx.root_available(idx) {
                ctx.lose(idx);
            } else if e.kind() == io::ErrorKind::NotFound {
                // Deleted between the walk and the read: it is missing now.
                ctx.submit(Command::MarkMissing {
                    files: vec![FileKey::new(src.id, t.path.clone())],
                    at_ms: now_ms(),
                });
                ctx.progress.vanished(idx, t.stage);
                ctx.progress.file_done(idx);
            } else {
                ctx.fail(idx, &t.path, t.stage, Stage::Probed, io_message(&e));
            }
            return;
        }
    };
    let meta = file.metadata().ok();
    if meta
        .as_ref()
        .is_some_and(|m| !fingerprint(m).unchanged(&t.stat))
    {
        changed_during_scan(ctx, idx, &t);
        return;
    }
    let mtime = meta.as_ref().and_then(|m| m.modified().ok());
    let size = file.len();
    let mut r = Controlled {
        file: &mut file,
        busy: &busy,
        ctl: &ctx.ctl,
        shared,
        progress: &ctx.progress,
        idx,
        delay: ctx.opts.chunk_delay,
    };

    // Probe: at most the first 256 KB (ADR-0010).
    let mut buf = Vec::with_capacity((size as usize).min(ps_media::PROBE_LIMIT));
    if let Err(e) = read_into(&mut r, &mut buf, ps_media::PROBE_LIMIT) {
        if !is_cancel(&e) {
            read_failed(ctx, idx, &t, Stage::Probed, &e);
        }
        return;
    }
    let probe = ps_media::probe_bytes(&buf, size, Some(rel.as_path()));
    let (class, reasons) = ps_media::classify_with(
        &probe,
        &rel,
        &ctx.opts.classifier,
        FolderHints {
            has_thumbs_db: t.thumbs_db_sibling,
        },
    );
    let skip = !class.is_candidate();
    let pr = ProbeResult {
        format: probe.format,
        class,
        reasons: reasons.iter().map(ToString::to_string).collect(),
        width: probe.stored.map(|d| d.width),
        height: probe.stored.map(|d| d.height),
        skip,
    };
    ctx.submit(Command::Probed(vec![(
        FileKey::new(src.id, t.path.clone()),
        pr,
    )]));
    if skip {
        ctx.progress.transition(idx, t.stage, Stage::Skipped);
        ctx.progress.file_done(idx);
        return;
    }
    ctx.progress.transition(idx, t.stage, Stage::Probed);

    // The single full read (ADR-0010): the probe bytes are the head of the buffer.
    let buffered = size <= ANALYSIS_BUFFER_MAX_BYTES;
    let want = if buffered { size } else { CHUNK as u64 };
    let permit = match ctx
        .budget
        .acquire(want, (idx, reader), &|| busy.checkpoint())
    {
        Ok(p) => p,
        Err(Cancelled) => return,
    };
    let content = if buffered {
        buf.reserve_exact((size as usize).saturating_sub(buf.len()));
        let res = read_into(&mut r, &mut buf, size as usize).and_then(|()| {
            if (buf.len() as u64) < size {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "file shrank while it was being read",
                ));
            }
            let mut one = [0u8; 1];
            match r.read(&mut one)? {
                0 => Ok(()),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file grew while it was being read",
                )),
            }
        });
        if let Err(e) = res {
            if !is_cancel(&e) {
                read_failed(ctx, idx, &t, Stage::Analyzed, &e);
            }
            return;
        }
        Content::Buffer(buf)
    } else {
        match ps_hash::identity_from_reader(&mut r, size) {
            Ok(identity) => Content::Streamed {
                identity,
                prefix: buf,
            },
            Err(e) => {
                if !is_cancel(&e) {
                    read_failed(ctx, idx, &t, Stage::Analyzed, &e);
                }
                return;
            }
        }
    };
    drop(file);
    let mut job = CpuJob {
        idx,
        path: t.path,
        abs,
        is_new: t.is_new,
        class,
        probe: Box::new(probe),
        content,
        mtime,
        _permit: permit,
    };
    ctx.progress.set_dir(idx, reader, None);
    loop {
        if busy.checkpoint().is_err() {
            return;
        }
        match cpu_tx.send_timeout(job, POLL) {
            Ok(()) => return,
            Err(SendTimeoutError::Timeout(j)) => job = j,
            Err(SendTimeoutError::Disconnected(_)) => return,
        }
    }
}

fn changed_during_scan(ctx: &RunCtx, idx: usize, t: &Todo) {
    ctx.changed_during_scan.fetch_add(1, Ordering::Relaxed);
    ctx.progress.error(RecentError {
        source: ctx.sources[idx].id,
        path: t.path.clone(),
        stage: t.stage,
        message: "changed during the scan; the next rescan picks it up".into(),
    });
    ctx.progress.file_done(idx);
}

fn read_failed(ctx: &RunCtx, idx: usize, t: &Todo, attempted: Stage, e: &io::Error) {
    if !ctx.root_available(idx) {
        ctx.lose(idx);
        return;
    }
    if matches!(
        e.kind(),
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
    ) {
        // Being written to right now: leave it for the next rescan rather than burning an attempt.
        changed_during_scan(ctx, idx, t);
        return;
    }
    let from = if attempted == Stage::Analyzed {
        Stage::Probed
    } else {
        t.stage
    };
    ctx.fail(idx, &t.path, from, attempted, io_message(e));
}

fn worker_loop(ctx: &RunCtx, rx: &Receiver<CpuJob>, tx: &Sender<Persist>) {
    loop {
        if ctx.ctl.is_cancelled() {
            // Queued jobs are dropped: their files stay `probed`.
            return;
        }
        let job = match rx.recv_timeout(POLL) {
            Ok(j) => j,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let busy = ctx.ctl.busy();
        if busy.checkpoint().is_err() {
            return;
        }
        let out = analyze(ctx, job);
        drop(busy);
        if tx.send(out).is_err() {
            return;
        }
    }
}

/// Everything from the one buffer (ADR-0010): identity, metadata, analysis decode, hashes, thumbnail.
fn analyze(ctx: &RunCtx, job: CpuJob) -> Persist {
    let CpuJob {
        idx,
        path,
        abs,
        is_new,
        class,
        probe,
        content,
        mtime,
        _permit,
    } = job;
    let (identity, bytes, full) = match content {
        Content::Buffer(b) => (ps_hash::identity_from_buffer(&b), b, true),
        Content::Streamed { identity, prefix } => (identity, prefix, false),
    };
    let metadata = ps_media::extract_metadata(&bytes, &probe, mtime);
    let capture = metadata.capture_date.map(|d| d.value);
    let (hashes, thumb) = if probe.kind() == Some(MediaKind::Video) {
        (
            None,
            ps_media::thumb::video_thumbnail(
                &abs,
                &probe,
                capture,
                ctx.opts.poster_tool.as_deref(),
            ),
        )
    } else if !full {
        (
            None,
            ps_media::thumb::placeholder(&probe, capture, "preview unavailable (file too large)"),
        )
    } else {
        match ps_media::analyze_still(&bytes, &probe) {
            Ok((decoded, thumb, _)) => {
                let img = &decoded.image;
                let hashes = ps_hash::ImageView::new(
                    img.width,
                    img.height,
                    ps_hash::PixelLayout::Rgb8,
                    &img.data,
                )
                .ok()
                .map(|v| ps_hash::perceptual_hash(&v));
                (hashes, thumb)
            }
            Err(DecodeError::HeicUnavailable) => (
                None,
                ps_media::thumb::placeholder(
                    &probe,
                    capture,
                    "preview unavailable (HEIC decoder not built in)",
                ),
            ),
            Err(DecodeError::NoRawPreview) => (
                None,
                ps_media::thumb::placeholder(
                    &probe,
                    capture,
                    "preview unavailable (no embedded preview)",
                ),
            ),
            Err(e) => {
                return Persist::Failed {
                    idx,
                    path,
                    message: format!("analysis failed: {e}"),
                };
            }
        }
    };
    drop(bytes);
    Persist::Done {
        idx,
        path,
        is_new,
        class,
        analysis: Box::new(AnalysisResult {
            identity,
            hashes,
            metadata,
        }),
        thumb,
    }
}

fn persist_loop(
    ctx: &RunCtx,
    rx: Receiver<Persist>,
    mut moves: Vec<HashMap<ContentId, Vec<RelPath>>>,
) {
    let mut batch: Vec<Persist> = Vec::with_capacity(PERSIST_BATCH);
    let mut deadline = Instant::now() + PERSIST_DELAY;
    loop {
        let msg = if batch.is_empty() {
            match rx.recv() {
                Ok(m) => {
                    deadline = Instant::now() + PERSIST_DELAY;
                    Some(m)
                }
                Err(_) => break,
            }
        } else {
            match rx.recv_deadline(deadline) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        match msg {
            Some(m) => {
                batch.push(m);
                if batch.len() >= PERSIST_BATCH {
                    persist_batch(ctx, std::mem::take(&mut batch), &mut moves);
                }
            }
            None => persist_batch(ctx, std::mem::take(&mut batch), &mut moves),
        }
    }
    persist_batch(ctx, batch, &mut moves);
}

fn persist_batch(
    ctx: &RunCtx,
    batch: Vec<Persist>,
    moves: &mut [HashMap<ContentId, Vec<RelPath>>],
) {
    if batch.is_empty() {
        return;
    }
    let mut done = Vec::new();
    for item in batch {
        match item {
            Persist::Failed { idx, path, message } => {
                ctx.fail(idx, &path, Stage::Probed, Stage::Analyzed, message);
            }
            Persist::Done {
                idx,
                path,
                is_new,
                class,
                analysis,
                thumb,
            } => done.push((idx, path, is_new, class, analysis, thumb)),
        }
    }
    if done.is_empty() {
        return;
    }
    // Thumbnails first: committed to thumbs.db before `thumbed` reaches the catalog (ADR-0008). Duplicates
    // share one row (ADR-0016).
    let mut seen = std::collections::HashSet::new();
    let to_store: Vec<(&ContentId, &ThumbnailRecord)> = done
        .iter()
        .filter(|d| seen.insert(d.4.identity))
        .filter(|d| !ctx.thumbs.contains(&d.4.identity).unwrap_or(false))
        .map(|d| (&d.4.identity, &d.5))
        .collect();
    if let Err(e) = ctx.thumbs.put_many(to_store) {
        for (idx, path, ..) in done {
            ctx.fail(
                idx,
                &path,
                Stage::Probed,
                Stage::Thumbed,
                format!("thumbnail store: {e}"),
            );
        }
        return;
    }
    let mut analyzed = Vec::with_capacity(done.len());
    let mut thumbed = Vec::with_capacity(done.len());
    for (idx, path, is_new, class, analysis, _thumb) in done {
        let src = &ctx.sources[idx];
        let key = FileKey::new(src.id, path.clone());
        if is_new && let Some(from) = moves[idx].get_mut(&analysis.identity).and_then(Vec::pop) {
            // Moved within the source: the old row takes over this path (ADR-0007).
            ctx.submit(Command::MoveFile {
                from: FileKey::new(src.id, from),
                to: key.clone(),
            });
            ctx.progress.moved(idx);
        }
        ctx.progress.transition(idx, Stage::Probed, Stage::Thumbed);
        ctx.progress.file_done(idx);
        if matches!(class, PhotoClass::Photo | PhotoClass::LikelyPhoto) {
            ctx.progress.photo(RecentPhoto {
                source: src.id,
                path,
                class,
                content: analysis.identity,
            });
        }
        thumbed.push(key.clone());
        analyzed.push((key, analysis));
    }
    ctx.submit(Command::Analyzed(analyzed));
    ctx.submit(Command::Thumbed(thumbed));
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Normalised absolute form of a root (for overlap checks and storage): symlinks resolved (macOS `/var` is
/// `/private/var`), and on Windows the plain `C:\...` form rather than the `\\?\` verbatim one
/// `canonicalize` returns, so stored roots match what users and the rest of the app see.
pub(crate) fn normalize_root(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p).map(ps_model::path::simplify_canonical)
}
