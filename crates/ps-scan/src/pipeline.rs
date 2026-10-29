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
use std::io::{self, Read, Seek, SeekFrom};
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
use crate::fs::SourceFile;
use crate::lane::{LaneController, MAX_READERS, Window, p90};
use crate::memory::{BufferPermit, ByteBudget};
use crate::options::ScanOptions;
use crate::progress::Progress;
use crate::report::{RunSummary, SourceSummary};
use crate::tiff_pass::OnePass;
use crate::walk::{WalkError, WalkOptions, walk};

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
    pub hidden_dirs_skipped: AtomicU64,
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
    /// Estimated working memory of HEIC and AVIF decodes in flight (ADR-0095). Separate from `budget`: a
    /// decode waits only for other decodes, which never wait for anything, so it cannot deadlock with the
    /// readers' file buffers.
    pub decode_budget: Arc<crate::memory::DecodeBudget>,
    pub sources: Vec<RunSource>,
    pub changed_during_scan: AtomicU64,
    pub retries_exhausted: AtomicU64,
    /// The post-scan regroup engine (shared by every run of the scanner).
    pub groups: Arc<crate::post::GroupEngine>,
    /// `opts.classifier.stable_hash()`, stored with every class this run decides (ADR-0051).
    pub classifier_hash: u64,
    /// The run's local zone (ADR-0070): `opts.zone`, else the machine's, read once per run.
    pub zone: Arc<dyn ps_media::LocalZone>,
    /// Refresh the analysis of unchanged files without stored class facts (ADR-0071).
    pub refresh_analysis: bool,
    /// A Fill in previews job of this scope (ADR-0109): no walk; the work list is the catalog's waiting rows.
    pub fill: Option<ps_model::FillScope>,
    /// Fill in previews counters: previews made, files decoded again without one, waiting files of offline sources.
    pub filled: AtomicU64,
    pub fill_failed: AtomicU64,
    pub skipped_offline: AtomicU64,
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

    // 1. Walk and diff every source in parallel (listing + stat only); a Fill in previews job lists nothing and
    // takes its work from the catalog (ADR-0109).
    let diffs: Vec<Option<DiffOutput>> = if let Some(scope) = ctx.fill.clone() {
        ctx.progress.set_fill();
        fill_work(&ctx, &scope)
    } else {
        walk_and_diff_all(&ctx)
    };

    // 2. The staged pipeline over the files to process.
    if !ctx.ctl.is_cancelled() {
        ctx.progress.set_phase(ScanPhase::Analyzing);
        run_pipeline(&ctx, diffs);
    }
    finish(&ctx, started)
}

fn walk_and_diff_all(ctx: &Arc<RunCtx>) -> Vec<Option<DiffOutput>> {
    std::thread::scope(|sc| {
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
    })
}

/// The waiting rule of this run's decoders, generations and poster-frame tool (ADR-0109).
pub(crate) fn waiting_rule(opts: &ScanOptions) -> ps_catalog::WaitingRule {
    ps_catalog::WaitingRule {
        decodable: ps_model::DecoderKey::ALL
            .into_iter()
            .filter(|k| opts.decoders.can_decode(*k))
            .collect(),
        generations: opts.generations,
        poster_tool: opts.poster_tool.is_some(),
    }
}

/// The work of a Fill in previews job (ADR-0109): the run's sources' placeholders that can be filled in now, in
/// path order per source, read from the catalog (no folder is listed). Rows of offline sources are counted and
/// left waiting. A row the re-decode can fill is queued as a re-decode (its identity checked against the bytes
/// read); one without dimensions, larger than the analysis buffer or a video is analysed again from
/// `discovered`, as a rescan does.
fn fill_work(ctx: &RunCtx, scope: &ps_model::FillScope) -> Vec<Option<DiffOutput>> {
    let ids: Vec<SourceId> = ctx.sources.iter().map(|s| s.id).collect();
    let rows = match ctx
        .catalog
        .read()
        .fillable_files(&waiting_rule(&ctx.opts), scope, &ids)
    {
        Ok(r) => r,
        Err(e) => {
            log::error!("fill run {}: waiting rows: {e}", ctx.run);
            return Vec::new();
        }
    };
    let mut outs: Vec<DiffOutput> = ctx.sources.iter().map(|_| DiffOutput::default()).collect();
    for r in rows {
        let Some(idx) = ctx.sources.iter().position(|s| s.id == r.key.source) else {
            continue;
        };
        let s = &ctx.sources[idx];
        if s.offline {
            ctx.skipped_offline.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let out = &mut outs[idx];
        let Some(identity) = r.identity else { continue };
        let full = crate::diff::needs_full_analysis(&r);
        let order = out.todo.len() as u32;
        let mut todo = Todo {
            path: r.key.path.clone(),
            stat: r.stat,
            order,
            thumbs_db_sibling: false,
            stage: r.stage,
            is_new: false,
            refresh: false,
            redecode: Some(identity),
            prior_failure: None,
            meta_reread: false,
            structure_only: false,
            read_path: None,
        };
        out.counts.unchanged += 1;
        if full {
            // Classified again: the folder's Thumbs.db hint, from two stats (no listing).
            let dir = r.key.path.under(&s.root);
            let dir = dir.parent().unwrap_or(&s.root);
            todo.thumbs_db_sibling = ["Thumbs.db", "thumbs.db"].iter().any(|n| {
                ctx.opts
                    .fs
                    .metadata(&dir.join(n))
                    .is_ok_and(|m| m.is_file())
            });
            todo.redecode = None;
            todo.stage = Stage::Discovered;
            out.counts.outdated_previews += 1;
            out.upserts
                .push(ps_catalog::FileRow::discovered(r.key.clone(), r.stat));
            *out.stages.get_mut(Stage::Discovered) += 1;
        } else {
            out.counts.redecode += 1;
            *out.stages.get_mut(r.stage) += 1;
        }
        if r.decoder_missing.is_none() && !full {
            out.counts.outdated_previews += 1;
        }
        out.bytes_to_process += r.stat.size;
        out.present += 1;
        out.todo.push(todo);
    }
    outs.into_iter()
        .enumerate()
        .map(|(idx, mut d)| {
            if ctx.sources[idx].offline {
                ctx.progress.set_incomplete(idx);
                return None;
            }
            for chunk in std::mem::take(&mut d.upserts).chunks(UPSERT_CHUNK) {
                ctx.submit(Command::UpsertFiles(chunk.to_vec()));
            }
            ctx.progress.set_diff(
                idx,
                &d.counts,
                d.present,
                &d.stages,
                d.todo.len() as u64,
                d.bytes_to_process,
            );
            Some(d)
        })
        .collect()
}

/// Flush, regroup and record the outcome of a run.
fn finish(ctx: &Arc<RunCtx>, started: Instant) -> RunSummary {
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
    // After the flush and the regroup: the waiting counts are the catalog's.
    let mut summary = summarize(ctx, status, started.elapsed());
    summary.grouping = grouping;
    if let Some(scope) = &ctx.fill {
        let online: Vec<SourceId> = ctx
            .sources
            .iter()
            .filter(|s| !s.offline && !s.lost.load(Ordering::Acquire))
            .map(|s| s.id)
            .collect();
        let still_waiting = if online.is_empty() {
            0
        } else {
            ctx.catalog
                .read()
                .fillable_files(&waiting_rule(&ctx.opts), scope, &online)
                .map_or(0, |r| r.len() as u64)
        };
        summary.fill = Some(ps_model::FillSummary {
            filled: ctx.filled.load(Ordering::Relaxed),
            skipped_offline: ctx.skipped_offline.load(Ordering::Relaxed),
            failed: ctx.fill_failed.load(Ordering::Relaxed),
            still_waiting,
        });
    }
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
            hidden_dirs_skipped: s.hidden_dirs_skipped.load(Ordering::Relaxed),
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
        undecoded: undecoded(ctx),
        damaged: damaged(ctx),
        fill: None,
    }
}

/// The present files of the run's sources catalogued as damaged at the end of the run (ADR-0115): one aggregate.
fn damaged(ctx: &RunCtx) -> u64 {
    let ids: Vec<SourceId> = ctx.sources.iter().map(|s| s.id).collect();
    ctx.catalog.read().damaged_count(&ids).unwrap_or_else(|e| {
        log::error!("scan run {}: damaged count: {e}", ctx.run);
        0
    })
}

/// The files of the run's sources still waiting for a decoder (ADR-0091), per format and decoder, with the
/// run's decoder report.
fn undecoded(ctx: &RunCtx) -> Vec<ps_model::Undecoded> {
    let ids: Vec<SourceId> = ctx.sources.iter().map(|s| s.id).collect();
    let rows = match ctx.catalog.read().undecoded_counts(&ids) {
        Ok(r) => r,
        Err(e) => {
            log::error!("scan run {}: undecoded counts: {e}", ctx.run);
            return Vec::new();
        }
    };
    undecoded_from(&rows, ctx.opts.decoders.as_ref())
}

/// Aggregate rows (source, format, decoder, count) into one entry per format and decoder.
pub fn undecoded_from(
    rows: &[(SourceId, String, ps_model::DecoderKey, u64)],
    decoders: &ps_media::Decoders,
) -> Vec<ps_model::Undecoded> {
    let mut out: Vec<ps_model::Undecoded> = Vec::new();
    for (source, format, key, count) in rows {
        match out
            .iter_mut()
            .find(|u| u.format == *format && u.key == *key)
        {
            Some(u) => {
                u.count += count;
                if !u.sources.contains(source) {
                    u.sources.push(*source);
                }
            }
            None => {
                let entry = decoders.report().for_key(*key);
                out.push(ps_model::Undecoded {
                    format: format.clone(),
                    key: *key,
                    count: *count,
                    available: decoders.can_decode(*key),
                    reason: entry.and_then(|e| e.reason.clone()),
                    fix: entry.and_then(|e| e.fix.clone()),
                    sources: vec![*source],
                });
            }
        }
    }
    out
}

fn walk_and_diff(ctx: &RunCtx, idx: usize) -> Option<DiffOutput> {
    let s = &ctx.sources[idx];
    if s.offline {
        return None;
    }
    let options = WalkOptions {
        include_hidden: ctx.opts.include_hidden,
    };
    let w = match walk(&ctx.opts.fs, &s.root, options, &ctx.ctl, |d| {
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
    s.hidden_dirs_skipped
        .store(w.hidden_dirs_skipped, Ordering::Relaxed);
    if w.hidden_dirs_skipped > 0 {
        log::info!(
            "source {}: {} hidden folders not scanned (scanning.include_hidden is off)",
            s.id,
            w.hidden_dirs_skipped
        );
    }
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
    let mut d = match diff(
        s.id,
        w.entries,
        ctx.catalog.read(),
        &ctx.opts.classifier,
        ctx.classifier_hash,
        ctx.refresh_analysis,
        &ctx.opts.decoders,
        &ctx.opts.generations,
        ctx.opts.poster_tool.is_some(),
    ) {
        Ok(d) => d,
        Err(e) => {
            log::error!("scan run {}: diff of source {}: {e}", ctx.run, s.id);
            return None;
        }
    };
    // Stored stats of unchanged files brought up to the walk's first (ADR-0112: a stale inode, a FAT hour shift), so
    // the upserts below compare against them.
    for chunk in std::mem::take(&mut d.stat_refresh).chunks(UPSERT_CHUNK) {
        ctx.submit(Command::RefreshStats(chunk.to_vec()));
    }
    for chunk in std::mem::take(&mut d.upserts).chunks(UPSERT_CHUNK) {
        ctx.submit(Command::UpsertFiles(chunk.to_vec()));
    }
    for chunk in std::mem::take(&mut d.reclassified).chunks(UPSERT_CHUNK) {
        ctx.submit(Command::Reclassify(chunk.to_vec()));
    }
    // Rows dated by older date rules: dated again from stored facts, no file read (ADR-0071).
    let stale = std::mem::take(&mut d.stale_dates);
    if !stale.is_empty() {
        match crate::redate::redate(
            s.id,
            &stale,
            ctx.catalog.read(),
            &*ctx.zone,
            &ctx.ctl,
            crate::redate::REDATE_CHUNK,
            |cmd| ctx.submit(cmd),
        ) {
            Ok(r) => {
                d.counts.redated = r.changed;
                log::info!(
                    "source {}: {} rows re-dated by the current date rules, {} dates changed{}",
                    s.id,
                    r.written,
                    r.changed,
                    if r.cancelled { " (cancelled)" } else { "" }
                );
            }
            Err(e) => log::error!("scan run {}: re-dating source {}: {e}", ctx.run, s.id),
        }
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
        /// A plain TIFF's metadata directories as a compact TIFF (ADR-0084), read instead of the prefix.
        metadata_block: Option<Vec<u8>>,
        /// A plain TIFF decoded during the same read (ADR-0086).
        decoded: Option<Result<ps_media::Decoded, DecodeError>>,
        /// The first image's dimensions as that decode read them (even when it failed later), for a TIFF
        /// whose probe found none.
        tiff_dims: Option<(u32, u32)>,
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
    /// A re-decode of an unchanged file that waited for a decoder (ADR-0091); its identity was checked.
    redecode: bool,
    /// A re-decode after a failed decode (ADR-0098): that failure.
    prior_failure: Option<ps_model::DecodeFailure>,
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
    /// A re-decode (ADR-0091); it never fails the file.
    Redecoded {
        idx: usize,
        path: RelPath,
        identity: ContentId,
        outcome: Redecode,
    },
}

/// How a re-decode ended.
enum Redecode {
    /// Preview and hashes filled in.
    Filled {
        hashes: Option<ps_model::PerceptualHash>,
        thumb: ThumbnailRecord,
    },
    /// The decoder is missing again: the file keeps waiting.
    StillMissing,
    /// The decoder rejected the data or did not complete: the file keeps its exact-copy analysis, with the failure
    /// recorded for the next rescan's retry rule (ADR-0098), and a new placeholder saying why, stamped with this
    /// run's generation (ADR-0109; none when the decode did not complete for a reason that says nothing about the
    /// file).
    ///
    /// The flag says whether to report it as a run error: always, except a RAW that is not damaged (ADR-0115: an
    /// unknown camera or a RAW without a preview read again after the generation bump stays a variant not
    /// supported, as in the analysis); HEIC and AVIF re-decodes are always reported (ADR-0095, ADR-0098).
    Undecodable(
        String,
        Option<ps_model::DecodeFailure>,
        Option<(ThumbnailRecord, ps_model::PreviewStamp)>,
        bool,
    ),
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

/// Read the rest of a file of `size` bytes into `buf` (which holds its head), then check that it ends there.
fn read_rest(r: &mut Controlled<'_>, buf: &mut Vec<u8>, size: u64) -> io::Result<()> {
    buf.reserve_exact((size as usize).saturating_sub(buf.len()));
    read_into(r, buf, size as usize)?;
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
}

/// The probe of a plain TIFF whose directories lie beyond `prefix`, with bounded targeted reads through the
/// lane's reader (ADR-0084); the reader is back at the end of the prefix afterwards.
fn probe_with_targeted_reads(
    r: &mut Controlled<'_>,
    prefix: &[u8],
    size: u64,
    rel: &Path,
) -> io::Result<(ProbeRecord, Vec<ps_media::tiff::Segment>)> {
    let mut fetch = |off: u64, len: usize| -> io::Result<Vec<u8>> {
        r.seek(SeekFrom::Start(off))?;
        let mut v = Vec::with_capacity(len.min(CHUNK));
        (&mut *r).take(len as u64).read_to_end(&mut v)?;
        Ok(v)
    };
    let (probe, loaded) =
        ps_media::probe_with_directory_reads(prefix, size, Some(rel), &mut fetch)?;
    r.seek(SeekFrom::Start(prefix.len() as u64))?;
    Ok((probe, loaded.segments))
}

/// Hash and decode a plain TIFF above 64 MB from one sequential read (ADR-0086). The decode's own failure is
/// part of the result, with the first image's dimensions when the decoder read them; an I/O error or a cancel
/// (during the decode or after it) is the `Err`. Pause and cancel are checked before every strip or tile, also
/// those served from memory without a read (ADR-0088).
#[allow(clippy::type_complexity)]
fn one_pass_analysis(
    r: &mut Controlled<'_>,
    size: u64,
    prefix: &[u8],
    segments: &[ps_media::tiff::Segment],
    probe: &ProbeRecord,
) -> io::Result<(
    ContentId,
    Result<ps_media::Decoded, DecodeError>,
    Option<(u32, u32)>,
)> {
    let busy = r.busy;
    let stop = || busy.checkpoint().is_err();
    let mut pass = OnePass::new(r, size, prefix, segments)?;
    let (decoded, stats) = ps_media::decode::decode_tiff_reader_with(
        &mut pass,
        probe,
        ps_media::decode::ANALYSIS_MIN_LONG_EDGE,
        &stop,
    );
    if let Some(e) = pass.take_error() {
        return Err(e);
    }
    if matches!(decoded, Err(DecodeError::Cancelled)) {
        return Err(io::Error::other(Cancelled));
    }
    let identity = pass.finish()?;
    let dims = Some(stats.image_dims).filter(|&(w, h)| w > 0 && h > 0);
    Ok((identity, decoded, dims))
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
    let abs = t.read_path.as_ref().unwrap_or(&t.path).under(&src.root);
    let mut file = match ctx.opts.fs.open(&abs) {
        Ok(f) => f,
        Err(e) => {
            if !ctx.root_available(idx) {
                ctx.lose(idx);
            } else if e.kind() == io::ErrorKind::NotFound && t.read_path.is_none() {
                // Deleted between the walk and the read: it is missing now.
                ctx.submit(Command::MarkMissing {
                    files: vec![FileKey::new(src.id, t.path.clone())],
                    at_ms: now_ms(),
                });
                ctx.progress.vanished(idx, t.stage);
                ctx.progress.file_done(idx);
            } else if t.refresh {
                refresh_unreadable(ctx, idx, &t, &e);
            } else if t.meta_reread && e.kind() == io::ErrorKind::NotFound && t.read_path.is_some()
            {
                // The file a cleanup run moved into the quarantine folder is gone from there (removed by hand): there
                // is nothing left to read, now or later. Marked current without new facts, quietly (ADR-0108).
                reread_gone(ctx, idx, &t);
            } else if t.meta_reread {
                reread_unreadable(ctx, idx, &t, &e);
            } else if t.redecode.is_some() {
                redecode_unreadable(ctx, idx, &t, &io_message(&e));
            } else {
                ctx.fail(idx, &t.path, t.stage, Stage::Probed, io_message(&e));
            }
            return;
        }
    };
    let meta = file.metadata().ok();
    if meta
        .as_ref()
        .is_some_and(|m| !ctx.opts.fs.fingerprint(m).unchanged(&t.stat))
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

    // A header re-read for frame identity (ADR-0105): the probe window, read-only, plus bounded targeted reads of
    // directories and frame values beyond it (ADR-0108); the facts are merged into the stored metadata. A RAW with
    // preview pixels is also checked for a cut over the same reads (ADR-0117); a row whose frame identity is current
    // reads only the structure check's head and its directories. No decode, no hashing.
    if t.meta_reread {
        let want = if t.structure_only {
            ps_media::raw_check::STRUCTURE_HEAD
        } else {
            ps_media::PROBE_LIMIT
        };
        let mut head = Vec::with_capacity((size as usize).min(want));
        let read = read_into(&mut r, &mut head, want).and_then(|()| {
            let probe = ps_media::probe_bytes(&head, size, Some(rel.as_path()));
            let mut fetch = |off: u64, len: usize| -> io::Result<Vec<u8>> {
                r.seek(SeekFrom::Start(off))?;
                let mut v = Vec::with_capacity(len.min(CHUNK));
                (&mut r).take(len as u64).read_to_end(&mut v)?;
                Ok(v)
            };
            if t.structure_only {
                ps_media::raw_check::check_by_reads(
                    &head,
                    size,
                    &probe,
                    &mut fetch,
                    ANALYSIS_BUFFER_MAX_BYTES,
                )
                .map(|structure| (None, structure, probe))
            } else {
                ps_media::reread_header(
                    &head,
                    size,
                    &probe,
                    &mut fetch,
                    true,
                    ANALYSIS_BUFFER_MAX_BYTES,
                )
                .map(|h| (Some(h.facts), h.structure, probe))
            }
        });
        let (facts, structure, probe) = match read {
            Ok(found) => found,
            Err(e) => {
                if !is_cancel(&e) {
                    if ctx.root_available(idx) {
                        reread_unreadable(ctx, idx, &t, &e);
                    } else {
                        ctx.lose(idx);
                    }
                }
                return;
            }
        };
        drop(file);
        let key = FileKey::new(src.id, t.path.clone());
        // `Err(())`: found cut, but the placeholder could not be stored; the structure check stays due.
        let damaged = match &structure {
            ps_media::raw_check::Structure::Cut(cut) => cut_found(ctx, idx, &t, &key, &probe, cut)
                .map(Some)
                .ok_or(()),
            ps_media::raw_check::Structure::Unknown => {
                log::info!(
                    "{}: its directories lie beyond the structure check's bounds and it is too large to read whole; \
                     not judged",
                    t.path
                );
                Ok(None)
            }
            ps_media::raw_check::Structure::Complete => Ok(None),
        };
        let reread = |frame_id, maker_notes, camera_original, version, merge_facts, damaged| {
            Command::MetaReread(vec![ps_catalog::MetaReread {
                key: key.clone(),
                frame_id,
                maker_notes,
                camera_original,
                version,
                merge_facts,
                damaged,
            }])
        };
        match (facts, damaged) {
            // A frame value beyond the bounds: not stamped, so the row is not taken for "no frame identity"; a later
            // full analysis (a change or a refresh) reads it, and cleanup waits for it (ADR-0108).
            (Some(None), Ok(None) | Err(())) => {
                log::info!(
                    "{}: frame values lie beyond the re-read's bounds; left for a full analysis",
                    t.path
                );
            }
            // Cut short: it has no hashes, so it joins no burst by look and its frame identity cannot matter; stamped
            // current, so it is not re-checked (and reported) on every rescan.
            (Some(None), Ok(Some(stamp))) => {
                ctx.submit(reread(
                    None,
                    false,
                    false,
                    ps_model::META_LOGIC_VERSION,
                    false,
                    Some(stamp),
                ));
                ctx.progress.frame_reread(idx);
            }
            // The frame facts are current; the structure check stays due when its placeholder was not stored.
            (Some(Some(facts)), damaged) => {
                let version = if damaged.is_ok() {
                    ps_model::META_LOGIC_VERSION
                } else {
                    ps_model::FRAME_META_VERSION
                };
                ctx.submit(reread(
                    facts.frame_id,
                    facts.maker_notes,
                    facts.camera_original,
                    version,
                    true,
                    damaged.ok().flatten(),
                ));
                ctx.progress.frame_reread(idx);
            }
            (None, Ok(damaged)) => {
                ctx.submit(reread(
                    None,
                    false,
                    false,
                    ps_model::META_LOGIC_VERSION,
                    false,
                    damaged,
                ));
                ctx.progress.frame_reread(idx);
            }
            // Found cut, its placeholder not stored: the row keeps its version, so the next rescan checks it again.
            (None, Err(())) => {}
        }
        ctx.progress.file_done(idx);
        ctx.progress.set_dir(idx, reader, None);
        return;
    }

    // A re-decode (ADR-0091): the single full read, then the identity check. The same content goes on as a
    // re-decode; other content under an unchanged fingerprint is analysed like a changed file.
    let mut redecode_permit = None;
    let mut buf = Vec::new();
    if let Some(stored) = t.redecode {
        if size > ANALYSIS_BUFFER_MAX_BYTES {
            redecode_unreadable(ctx, idx, &t, "it is too large to decode");
            return;
        }
        let permit = match ctx
            .budget
            .acquire(size, (idx, reader), &|| busy.checkpoint())
        {
            Ok(p) => p,
            Err(Cancelled) => return,
        };
        buf.reserve(size as usize);
        if let Err(e) = read_rest(&mut r, &mut buf, size) {
            if !is_cancel(&e) {
                if ctx.root_available(idx) {
                    redecode_unreadable(ctx, idx, &t, &io_message(&e));
                } else {
                    ctx.lose(idx);
                }
            }
            return;
        }
        if ps_hash::identity_from_buffer(&buf) == stored {
            let probe = ps_media::probe_buffer(&buf, Some(rel.as_path()));
            drop(file);
            let job = CpuJob {
                idx,
                path: t.path,
                abs,
                is_new: false,
                class: PhotoClass::Photo,
                probe: Box::new(probe),
                content: Content::Buffer(buf),
                mtime,
                redecode: true,
                prior_failure: t.prior_failure.clone(),
                _permit: permit,
            };
            ctx.progress.set_dir(idx, reader, None);
            send_job(&busy, cpu_tx, job);
            return;
        }
        log::info!(
            "{}: content changed under an unchanged fingerprint; analysed again",
            t.path
        );
        redecode_permit = Some(permit);
    }

    // Probe: at most the first 256 KB (ADR-0010).
    let reread = redecode_permit.is_none();
    if reread {
        buf.reserve((size as usize).min(ps_media::PROBE_LIMIT));
    }
    if reread && let Err(e) = read_into(&mut r, &mut buf, ps_media::PROBE_LIMIT) {
        if !is_cancel(&e) {
            if t.refresh && ctx.root_available(idx) {
                refresh_unreadable(ctx, idx, &t, &e);
            } else {
                read_failed(ctx, idx, &t, Stage::Probed, &e);
            }
        }
        return;
    }
    let mut probe = if reread {
        ps_media::probe_bytes(&buf, size, Some(rel.as_path()))
    } else {
        ps_media::probe_buffer(&buf, Some(rel.as_path()))
    };
    let buffered = size <= ANALYSIS_BUFFER_MAX_BYTES;
    // A plain TIFF whose directories lie beyond the prefix (ADR-0084): read in full now when it is buffered
    // anyway, else fetch the directories with bounded targeted reads. Not for an analysis refresh, which reads
    // only the prefix (ADR-0071).
    let mut early_permit = redecode_permit;
    let mut segments: Vec<ps_media::tiff::Segment> = Vec::new();
    if !t.refresh && reread && ps_media::needs_directory_reads(&buf, size) {
        if buffered {
            let permit = match ctx
                .budget
                .acquire(size, (idx, reader), &|| busy.checkpoint())
            {
                Ok(p) => p,
                Err(Cancelled) => return,
            };
            if let Err(e) = read_rest(&mut r, &mut buf, size) {
                if !is_cancel(&e) {
                    read_failed(ctx, idx, &t, Stage::Probed, &e);
                }
                return;
            }
            probe = ps_media::probe_buffer(&buf, Some(rel.as_path()));
            early_permit = Some(permit);
        } else {
            match probe_with_targeted_reads(&mut r, &buf, size, rel.as_path()) {
                Ok((p, loaded)) => {
                    probe = p;
                    segments = loaded;
                }
                Err(e) => {
                    if !is_cancel(&e) {
                        read_failed(ctx, idx, &t, Stage::Probed, &e);
                    }
                    return;
                }
            }
        }
    }
    let facts = ps_media::class_facts(&probe);
    let (class, reasons) = ps_media::classify_facts(
        &facts,
        &rel,
        &ctx.opts.classifier,
        FolderHints {
            has_thumbs_db: t.thumbs_db_sibling,
        },
    );
    let skip = !class.is_candidate();
    if t.refresh && (t.stage != Stage::Skipped || skip) {
        // Analysis refresh (ADR-0071): store the facts and the class; nothing else changes. A skipped file that
        // became a candidate is processed further below like any probed file.
        ctx.submit(Command::Reclassify(vec![ps_catalog::Reclassification {
            key: FileKey::new(src.id, t.path.clone()),
            class,
            reasons: reasons.iter().map(ToString::to_string).collect(),
            classifier_hash: ctx.classifier_hash,
            stage: None,
            facts: Some(facts),
        }]));
        ctx.progress.refreshed(idx);
        ctx.progress.file_done(idx);
        return;
    }
    let pr = ProbeResult {
        format: probe.format,
        class,
        reasons: reasons.iter().map(ToString::to_string).collect(),
        width: probe.stored.map(|d| d.width),
        height: probe.stored.map(|d| d.height),
        skip,
        facts: Some(facts),
        classifier_hash: Some(ctx.classifier_hash),
    };
    ctx.submit(Command::Probed(vec![(
        FileKey::new(src.id, t.path.clone()),
        pr,
    )]));
    if t.refresh {
        // A skipped file the refresh made a candidate: refreshed now, analysed like any probed file.
        ctx.progress.refreshed(idx);
    }
    if skip {
        ctx.progress.transition(idx, t.stage, Stage::Skipped);
        ctx.progress.file_done(idx);
        return;
    }
    ctx.progress.transition(idx, t.stage, Stage::Probed);

    // The single full read (ADR-0010): the probe bytes are the head of the buffer. A plain TIFF above 64 MB is
    // hashed and decoded in one sequential pass (ADR-0086), counted as one analysis buffer.
    let one_pass = !buffered && probe.format == Some(ps_model::Format::Tiff);
    let want = if buffered || one_pass {
        size.min(ANALYSIS_BUFFER_MAX_BYTES)
    } else {
        CHUNK as u64
    };
    let early = early_permit.is_some();
    let permit = match early_permit {
        Some(p) => p,
        None => match ctx
            .budget
            .acquire(want, (idx, reader), &|| busy.checkpoint())
        {
            Ok(p) => p,
            Err(Cancelled) => return,
        },
    };
    let content = if buffered {
        if !early && let Err(e) = read_rest(&mut r, &mut buf, size) {
            if !is_cancel(&e) {
                read_failed(ctx, idx, &t, Stage::Analyzed, &e);
            }
            return;
        }
        Content::Buffer(buf)
    } else if one_pass {
        match one_pass_analysis(&mut r, size, &buf, &segments, &probe) {
            Ok((identity, decoded, tiff_dims)) => Content::Streamed {
                identity,
                metadata_block: ps_media::tiff_metadata_block(&buf, &segments),
                prefix: buf,
                decoded: Some(decoded),
                tiff_dims,
            },
            Err(e) => {
                if !is_cancel(&e) {
                    read_failed(ctx, idx, &t, Stage::Analyzed, &e);
                }
                return;
            }
        }
    } else {
        match ps_hash::identity_from_reader(&mut r, size) {
            Ok(identity) => Content::Streamed {
                identity,
                prefix: buf,
                metadata_block: None,
                decoded: None,
                tiff_dims: None,
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
    let job = CpuJob {
        idx,
        path: t.path,
        abs,
        is_new: t.is_new,
        class,
        probe: Box::new(probe),
        content,
        mtime,
        redecode: false,
        prior_failure: None,
        _permit: permit,
    };
    ctx.progress.set_dir(idx, reader, None);
    send_job(&busy, cpu_tx, job);
}

/// Hand a job to the CPU workers, giving up on cancel.
fn send_job(busy: &Busy<'_>, cpu_tx: &Sender<CpuJob>, mut job: CpuJob) {
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

/// A file waiting for a decoder could not be read to fill in its preview (ADR-0091). Its catalogued analysis,
/// stage, group and marker stay as they were; the error is reported and the next rescan tries again.
fn redecode_unreadable(ctx: &RunCtx, idx: usize, t: &Todo, why: &str) {
    ctx.progress.error(RecentError {
        source: ctx.sources[idx].id,
        path: t.path.clone(),
        stage: t.stage,
        message: format!("could not be read to fill in its preview ({why}); it keeps its analysis"),
    });
    ctx.progress.file_done(idx);
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

/// An unchanged, analysed file could not be read for the analysis refresh (ADR-0071). Its catalogued analysis
/// stays exactly as it was, stage included, so it keeps its duplicate group and its exported copy; the error is
/// reported and the next refresh tries again.
fn refresh_unreadable(ctx: &RunCtx, idx: usize, t: &Todo, e: &io::Error) {
    ctx.progress.error(RecentError {
        source: ctx.sources[idx].id,
        path: t.path.clone(),
        stage: t.stage,
        message: format!(
            "could not be read for the analysis refresh ({}); it keeps its analysis",
            io_message(e)
        ),
    });
    ctx.progress.file_done(idx);
}

/// A quarantined file gone from the quarantine folder (ADR-0108): marked current with no new facts, no error.
fn reread_gone(ctx: &RunCtx, idx: usize, t: &Todo) {
    ctx.submit(Command::MetaReread(vec![ps_catalog::MetaReread {
        key: FileKey::new(ctx.sources[idx].id, t.path.clone()),
        frame_id: None,
        maker_notes: false,
        camera_original: false,
        version: ps_model::META_LOGIC_VERSION,
        merge_facts: false,
        damaged: None,
    }]));
    ctx.progress.file_done(idx);
}

/// A RAW the one-time structure check found cut short (ADR-0117): its content's thumbnail becomes the "could not
/// be read" placeholder, and it is reported like the analysis reports a cut RAW (ADR-0115). Returns the stamp the
/// catalog stores with the hashes dropped; `None` when the thumbnail could not be stored (the row keeps its
/// version, so the next rescan checks it again).
fn cut_found(
    ctx: &RunCtx,
    idx: usize,
    t: &Todo,
    key: &FileKey,
    probe: &ProbeRecord,
    cut: &ps_media::raw_check::Truncation,
) -> Option<ps_model::PreviewStamp> {
    let identity = ctx.catalog.read().file(key).ok().flatten()?.identity?;
    let e = DecodeError::Failed(cut.message());
    let mut thumb = placeholder_of(probe, None, &e);
    let stamp = stamp(ctx, &mut thumb, probe.format)?;
    if let Err(err) = ctx.thumbs.put_many([(&identity, &thumb)]) {
        log::warn!("{}: could not store its placeholder ({err})", t.path);
        return None;
    }
    ctx.progress.error(RecentError {
        source: ctx.sources[idx].id,
        path: t.path.clone(),
        stage: t.stage,
        message: format!(
            "could not be read ({e}); it is catalogued with a placeholder and exported as is"
        ),
    });
    Some(stamp)
}

fn reread_unreadable(ctx: &RunCtx, idx: usize, t: &Todo, e: &io::Error) {
    ctx.progress.error(RecentError {
        source: ctx.sources[idx].id,
        path: t.path.clone(),
        stage: t.stage,
        message: format!(
            "could not be read for its one-time header check (burst frame number, RAW file cut short) ({}); it \
             keeps its analysis and is checked again by the next rescan",
            io_message(e)
        ),
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
        // `None`: cancelled during the decode; the file stays `probed`, like a queued job.
        let Some(out) = analyze(ctx, job, &busy) else {
            return;
        };
        drop(busy);
        if tx.send(out).is_err() {
            return;
        }
    }
}

/// Everything from the one buffer (ADR-0010): identity, metadata, analysis decode, hashes, thumbnail. `None`
/// when a cancel stopped the decode (pause parks it inside the decode, through `busy`).
fn analyze(ctx: &RunCtx, job: CpuJob, busy: &Busy<'_>) -> Option<Persist> {
    let CpuJob {
        idx,
        path,
        abs,
        is_new,
        class,
        probe,
        content,
        mtime,
        redecode,
        prior_failure,
        _permit,
    } = job;
    if redecode {
        return analyze_redecode(
            ctx,
            idx,
            path,
            &probe,
            content,
            prior_failure.as_ref(),
            busy,
        );
    }
    let (identity, bytes, full, metadata_block, decoded, tiff_dims) = match content {
        Content::Buffer(b) => (ps_hash::identity_from_buffer(&b), b, true, None, None, None),
        Content::Streamed {
            identity,
            prefix,
            metadata_block,
            decoded,
            tiff_dims,
        } => (identity, prefix, false, metadata_block, decoded, tiff_dims),
    };
    let mut metadata = ps_media::extract_metadata(
        metadata_block.as_deref().unwrap_or(&bytes),
        &probe,
        mtime,
        &ps_media::DateContext {
            rel_path: Some(&path),
            zone: &*ctx.zone,
        },
    );
    // A plain TIFF whose probe found no dimensions (an ImageWidth the probe cannot read): the decoder's, so the
    // row gets them and a rescan does not analyse it again (ADR-0088).
    let is_tiff = probe.format == Some(ps_model::Format::Tiff);
    if is_tiff && metadata.fields.width.is_none() {
        let dims = tiff_dims.or_else(|| {
            full.then(|| ps_media::tiff_decode::dimensions(std::io::Cursor::new(&bytes)))
                .flatten()
        });
        if let Some((w, h)) = dims {
            metadata.fields.width = Some(w);
            metadata.fields.height = Some(h);
        }
    }
    let capture = metadata.capture_date.map(|d| d.value);
    let stop = || busy.checkpoint().is_err();
    // The decoder this format needs and this machine lacks (ADR-0091), or how its decode failed (ADR-0098).
    let mut decoder_missing = None;
    let mut decode_failure = None;
    let (hashes, mut thumb) = if probe.kind() == Some(MediaKind::Video) {
        (
            None,
            ps_media::thumb::video_thumbnail(
                &abs,
                &probe,
                capture,
                ctx.opts.poster_tool.as_deref(),
            ),
        )
    } else if let Some(decoded) = decoded {
        // Decoded in one pass by the reader (ADR-0086); a decode error keeps the identity, with a placeholder.
        match decoded.and_then(|d| {
            let thumb =
                ps_media::thumb::thumbnail_from_image(&d.image, ps_model::ThumbKind::Image)?;
            Ok((d, thumb))
        }) {
            Ok((d, thumb)) => (still_hashes(&d.image), thumb),
            // Neither the probe nor the decoder could read its dimensions: not a picture, and a thumbed row
            // without dimensions would be re-read by every rescan.
            Err(e) if is_tiff && metadata.fields.width.is_none() => {
                return Some(Persist::Failed {
                    idx,
                    path,
                    message: format!("analysis failed: {e}"),
                });
            }
            Err(e) => (None, placeholder_of(&probe, capture, &e)),
        }
    } else if !full {
        (
            None,
            ps_media::thumb::placeholder_for(
                &probe,
                capture,
                "preview unavailable (file too large)",
                Some(ps_model::PlaceholderReason::UnsupportedVariant {
                    detail: format!(
                        "{} files over {} MB",
                        probe.format.map_or("such", |f| f.label()),
                        ANALYSIS_BUFFER_MAX_BYTES >> 20
                    ),
                }),
            ),
        )
    } else {
        let Ok(permit) = decode_permit(ctx, &probe, busy) else {
            return None;
        };
        match ps_media::analyze_still_held(&bytes, &probe, &ctx.opts.decoders, &stop, hold(permit))
        {
            Ok((decoded, thumb, _)) => (still_hashes(&decoded.image), thumb),
            Err(DecodeError::Cancelled) => return None,
            Err(e @ (DecodeError::DecoderMissing(_) | DecodeError::Sequence)) => {
                if let DecodeError::DecoderMissing(k) = e {
                    decoder_missing = Some(k);
                }
                match ps_media::thumb::placeholder_for_error(&probe, capture, &e) {
                    Some(p) => (None, p),
                    None => unreachable!("a placeholder for every missing decoder"),
                }
            }
            // A RAW decode error of any kind keeps the file, with or without probe dimensions (ADR-0115): its
            // identity, metadata and a placeholder that says why, never hashed (so a cut RAW cannot join or
            // outrank its camera JPEG), exported as is. Only a damaged file (`Failed`: cut short, or a damaged
            // preview) is reported; an unknown camera or a RAW without a preview is a variant not supported.
            Err(e) if probe.format.is_some_and(ps_model::Format::is_raw) => {
                if matches!(e, DecodeError::Failed(_)) {
                    ctx.progress.error(RecentError {
                        source: ctx.sources[idx].id,
                        path: path.clone(),
                        stage: Stage::Thumbed,
                        message: format!(
                            "could not be read ({e}); it is catalogued with a placeholder and exported as is"
                        ),
                    });
                }
                (None, placeholder_of(&probe, capture, &e))
            }
            // A HEIC or AVIF the decoder rejects (truncated, corrupt, or larger than the decode cap) keeps its
            // identity and metadata with a placeholder, as without a decoder: catalogued, de-duplicated by exact
            // content and exported (ADR-0095, as a re-decode does, ADR-0091). Reported, never `unreadable`.
            Err(e) if is_system_or_av1_still(&probe) => {
                decode_failure = failure_record(ctx, &probe, &e, None);
                ctx.progress.error(RecentError {
                    source: ctx.sources[idx].id,
                    path: path.clone(),
                    stage: Stage::Thumbed,
                    message: format!(
                        "could not be decoded ({e}); it is catalogued with a placeholder and exported as is"
                    ),
                });
                let reason = ps_media::thumb::reason_for_error(&e).unwrap_or(
                    ps_model::PlaceholderReason::Unreadable {
                        message: e.to_string(),
                    },
                );
                (
                    None,
                    ps_media::thumb::placeholder_for(
                        &probe,
                        capture,
                        &format!("preview unavailable (could not be decoded: {e})"),
                        Some(reason),
                    ),
                )
            }
            // A recognised still with dimensions whose decode failed (ADR-0109, amending ADR-0055): catalogued with
            // its identity, metadata and a placeholder that says why, never hashed. A variant this version does not
            // decode is not an error; data the decoder rejected is reported.
            Err(e) if probe.display.is_some() || metadata.fields.width.is_some() => {
                if !matches!(e, DecodeError::Unsupported(_) | DecodeError::TooLarge(_)) {
                    ctx.progress.error(RecentError {
                        source: ctx.sources[idx].id,
                        path: path.clone(),
                        stage: Stage::Thumbed,
                        message: format!(
                            "could not be read ({e}); it is catalogued with a placeholder and exported as is"
                        ),
                    });
                }
                (None, placeholder_of(&probe, capture, &e))
            }
            Err(e) => {
                return Some(Persist::Failed {
                    idx,
                    path,
                    message: format!("analysis failed: {e}"),
                });
            }
        }
    };
    drop(bytes);
    // Placeholders carry this run's decoder generation of their format (ADR-0109).
    let preview = stamp(ctx, &mut thumb, probe.format);
    Some(Persist::Done {
        idx,
        path,
        is_new,
        class,
        analysis: Box::new(AnalysisResult {
            identity,
            decoder_missing,
            decode_failure,
            hashes,
            metadata,
            preview,
        }),
        thumb,
    })
}

/// The placeholder of a failed decode, with its reason ([`ps_media::thumb::placeholder_for_error`]); a decode that
/// did not complete for a reason that says nothing about the file counts as could not be read.
fn placeholder_of(
    probe: &ProbeRecord,
    capture: Option<ps_model::CaptureTime>,
    e: &DecodeError,
) -> ThumbnailRecord {
    ps_media::thumb::placeholder_for_error(probe, capture, e).unwrap_or_else(|| {
        ps_media::thumb::placeholder_for(
            probe,
            capture,
            &format!("preview unavailable ({e})"),
            Some(ps_model::PlaceholderReason::Unreadable {
                message: e.to_string(),
            }),
        )
    })
}

/// Stamp a placeholder with this run's decoder generation of `format` (a test's override included) and return
/// the row's `preview_gen` and `preview_reason`; `None` for real pixels (ADR-0109).
fn stamp(
    ctx: &RunCtx,
    thumb: &mut ThumbnailRecord,
    format: Option<ps_model::Format>,
) -> Option<ps_model::PreviewStamp> {
    if let Some(p) = thumb.placeholder.as_mut() {
        p.generation = ctx.opts.generations.of_format(format);
    }
    ps_model::PreviewStamp::of(thumb)
}

/// Reserve the estimated working memory of a HEIC or AVIF decode in the run's decode budget (ADR-0095); `None`
/// for other formats. Waits while other decodes hold the budget (a decode larger than the cap runs alone);
/// `Err` when the run is cancelled meanwhile.
fn decode_permit(
    ctx: &RunCtx,
    probe: &ProbeRecord,
    busy: &Busy<'_>,
) -> Result<Option<crate::memory::DecodePermit>, Cancelled> {
    let Some(bytes) = ps_media::decode::decode_working_bytes(probe) else {
        return Ok(None);
    };
    ctx.decode_budget
        .acquire(bytes, &|| busy.checkpoint())
        .map(Some)
}

/// A decode permit as the hold of an AVIF decode thread, which keeps it until it really ends (ADR-0097); for
/// other formats the analysis drops it when it returns.
fn hold(permit: Option<crate::memory::DecodePermit>) -> Option<Arc<dyn ps_media::av1::DecodeHold>> {
    permit.map(|p| Arc::new(p) as Arc<dyn ps_media::av1::DecodeHold>)
}

/// The failure record of a HEIC or AVIF decode that failed with its decoder present (ADR-0098): transient for
/// [`DecodeError::Interrupted`], attempts counted on from `prior`. `None` when no backend decodes the format.
fn failure_record(
    ctx: &RunCtx,
    probe: &ProbeRecord,
    e: &DecodeError,
    prior: Option<&ps_model::DecodeFailure>,
) -> Option<ps_model::DecodeFailure> {
    let key = match probe.format? {
        ps_model::Format::Heic => ps_model::DecoderKey::Heif,
        ps_model::Format::Avif => ps_model::DecoderKey::Avif,
        _ => return None,
    };
    let backend = ctx.opts.decoders.backend(key)?;
    if matches!(e, DecodeError::Refused(_)) {
        // Not started: no attempt used (ADR-0100).
        return Some(ps_model::DecodeFailure::refused(prior, key, &backend));
    }
    Some(ps_model::DecodeFailure::next(
        prior,
        key,
        &backend,
        matches!(e, DecodeError::Interrupted(_)),
    ))
}

/// HEIC and AVIF: formats whose decode failure keeps a placeholder instead of failing the file (ADR-0095).
/// Other stills keep failing as before: their decoders are bundled and a failure means unreadable data.
fn is_system_or_av1_still(probe: &ProbeRecord) -> bool {
    matches!(
        probe.format,
        Some(ps_model::Format::Heic | ps_model::Format::Avif)
    )
}

/// A re-decode (ADR-0091): decode the whole buffer with this run's decoders, hash it and make its thumbnail.
/// Never a failure of the file. `None` when a cancel stopped the decode.
fn analyze_redecode(
    ctx: &RunCtx,
    idx: usize,
    path: RelPath,
    probe: &ProbeRecord,
    content: Content,
    prior: Option<&ps_model::DecodeFailure>,
    busy: &Busy<'_>,
) -> Option<Persist> {
    let Content::Buffer(bytes) = content else {
        unreachable!("re-decodes read the whole file");
    };
    let identity = ps_hash::identity_from_buffer(&bytes);
    let stop = || busy.checkpoint().is_err();
    let Ok(permit) = decode_permit(ctx, probe, busy) else {
        return None;
    };
    let outcome = match ps_media::analyze_still_held(
        &bytes,
        probe,
        &ctx.opts.decoders,
        &stop,
        hold(permit),
    ) {
        Ok((decoded, thumb, _)) => Redecode::Filled {
            hashes: still_hashes(&decoded.image),
            thumb,
        },
        Err(DecodeError::Cancelled) => return None,
        Err(DecodeError::DecoderMissing(_)) => Redecode::StillMissing,
        Err(e) => {
            let placeholder =
                ps_media::thumb::placeholder_for_error(probe, None, &e).and_then(|mut t| {
                    let p = stamp(ctx, &mut t, probe.format)?;
                    Some((t, p))
                });
            let report = !probe.format.is_some_and(ps_model::Format::is_raw)
                || matches!(e, DecodeError::Failed(_));
            Redecode::Undecodable(
                e.to_string(),
                failure_record(ctx, probe, &e, prior),
                placeholder,
                report,
            )
        }
    };
    Some(Persist::Redecoded {
        idx,
        path,
        identity,
        outcome,
    })
}

/// Perceptual hashes of an analysis decode.
fn still_hashes(img: &ps_media::RgbImage) -> Option<ps_model::PerceptualHash> {
    ps_hash::ImageView::new(img.width, img.height, ps_hash::PixelLayout::Rgb8, &img.data)
        .ok()
        .map(|v| ps_hash::perceptual_hash(&v))
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
                if ctx.fill.is_some() {
                    ctx.fill_failed.fetch_add(1, Ordering::Relaxed);
                }
                ctx.fail(idx, &path, Stage::Probed, Stage::Analyzed, message);
            }
            Persist::Redecoded {
                idx,
                path,
                identity,
                outcome,
            } => persist_redecode(ctx, idx, path, identity, outcome),
            Persist::Done {
                idx,
                path,
                is_new,
                class,
                analysis,
                thumb,
            } => {
                if ctx.fill.is_some() {
                    let counter = if analysis.preview.is_some() {
                        &ctx.fill_failed
                    } else {
                        &ctx.filled
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                done.push((idx, path, is_new, class, analysis, thumb));
            }
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
        // A stored placeholder is replaced (a preview now, or a placeholder that says why at this generation,
        // ADR-0109); stored pixels are kept.
        .filter(|d| {
            matches!(
                ctx.thumbs.stored_kind(&d.4.identity),
                Ok(None | Some(ps_model::ThumbKind::Placeholder))
            )
        })
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

/// Write a re-decode (ADR-0091): the thumbnail replaces the placeholder for every exact copy (upsert), then
/// the hashes and the cleared marker; a decode error clears the marker only and is reported.
fn persist_redecode(
    ctx: &RunCtx,
    idx: usize,
    path: RelPath,
    identity: ContentId,
    outcome: Redecode,
) {
    let src = &ctx.sources[idx];
    let key = FileKey::new(src.id, path.clone());
    match outcome {
        Redecode::Filled { hashes, thumb } => {
            if let Err(e) = ctx.thumbs.put_many([(&identity, &thumb)]) {
                ctx.progress.error(RecentError {
                    source: src.id,
                    path,
                    stage: Stage::Thumbed,
                    message: format!(
                        "could not store its new preview ({e}); it keeps its analysis and waits"
                    ),
                });
                ctx.progress.file_done(idx);
                return;
            }
            match hashes {
                Some(h) => ctx.submit(Command::Redecoded(vec![(key, h)])),
                None => {
                    ctx.submit(Command::ClearDecoderMissing(vec![key.clone()]));
                    ctx.submit(Command::SetPreview(vec![(key, None)]));
                }
            }
            ctx.progress.redecoded(idx);
            ctx.filled.fetch_add(1, Ordering::Relaxed);
        }
        Redecode::StillMissing => {}
        Redecode::Undecodable(why, failure, placeholder, report) => {
            ctx.fill_failed.fetch_add(1, Ordering::Relaxed);

            // The placeholder now says why, at this run's generation: not read again until the file or the
            // generation changes (ADR-0109). Stored pixels (another copy of the same content decoded) are never
            // replaced by a placeholder, as in `persist_batch`: the row then shows them.
            if let Some((t, p)) = placeholder {
                match ctx.thumbs.stored_kind(&identity) {
                    Ok(None | Some(ps_model::ThumbKind::Placeholder)) => {
                        if ctx.thumbs.put_many([(&identity, &t)]).is_ok() {
                            ctx.submit(Command::SetPreview(vec![(key.clone(), Some(p))]));
                        }
                    }
                    Ok(Some(_)) => ctx.submit(Command::SetPreview(vec![(key.clone(), None)])),
                    Err(_) => {}
                }
            }
            ctx.submit(match failure {
                Some(f) => Command::DecodeFailed(vec![(key, f)]),
                None => Command::ClearDecoderMissing(vec![key]),
            });
            if report {
                ctx.progress.error(RecentError {
                    source: src.id,
                    path,
                    stage: Stage::Thumbed,
                    message: format!(
                        "could not be decoded ({why}); it keeps its exact-copy analysis"
                    ),
                });
            }
        }
    }
    ctx.progress.file_done(idx);
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
