//! `scan [start|resume|pause|cancel|status]` and `status`.
//!
//! A scan runs in the foreground of the command that started it: a progress line at most 10 times per second
//! (redrawn on a terminal, every 5 s otherwise), NDJSON snapshot lines with `--json` (at least 100 ms apart)
//! and a final summary line. Ctrl-C cancels cooperatively (exit 130, the run stays resumable with
//! `scan --resume`). Other terminals pause, resume or cancel it through the data folder ([`crate::control`]).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use ps_model::{ProgressSnapshot, RunStatus, ScanPhase, ScanRunId, SourceId};
use ps_service::groups::GroupTotals;
use ps_service::scan::{RunSummary, ScanService};

use super::Ctx;
use crate::control::{self, Interrupted, Job};
use crate::out::{bytes, duration_ms, emit, line};

const TICK: Duration = Duration::from_millis(100);
const JSON_EVERY: Duration = Duration::from_millis(200);
const PLAIN_EVERY: Duration = Duration::from_secs(5);
const BEAT_EVERY: Duration = Duration::from_millis(250);

fn phase(p: ScanPhase) -> &'static str {
    match p {
        ScanPhase::Starting => "starting",
        ScanPhase::Walking => "walking",
        ScanPhase::Analyzing => "analyzing",
        ScanPhase::Finishing => "finishing",
        ScanPhase::Done => "done",
    }
}

/// One compact progress line.
pub fn progress_line(s: &ProgressSnapshot) -> String {
    let eta = s
        .eta_ms
        .map_or_else(String::new, |e| format!("  ETA {}", duration_ms(e)));
    format!(
        "run {} [{}{}] {}/{} files  {}/s  {:.0} files/s  {} read{}  errors {}",
        s.run,
        phase(s.phase),
        if s.status == RunStatus::Paused {
            ", paused"
        } else {
            ""
        },
        s.processed,
        s.to_process,
        bytes(s.bytes_per_s.max(0.0) as u64),
        s.files_per_s.max(0.0) + 0.0,
        bytes(s.bytes_read),
        eta,
        s.errors
    )
}

fn latest_resumable(svc: &ScanService) -> Result<ScanRunId> {
    svc.runs()?
        .into_iter()
        .rev()
        .find(|r| {
            matches!(
                r.status,
                RunStatus::Paused | RunStatus::Cancelled | RunStatus::Interrupted
            ) || (r.status == RunStatus::Completed
                && RunSummary::from_json(&r.stats)
                    .is_some_and(|s| !s.incomplete_sources.is_empty()))
        })
        .map(|r| r.id)
        .context("no cancelled, paused or interrupted run to resume")
}

/// `scan` / `scan start` / `scan --resume`.
pub fn start(ctx: &Ctx, sources: &[i64], resume: bool, run: Option<i64>) -> Result<()> {
    let data = ctx.data_dir()?;
    if control::is_live(&data, Job::Scan) {
        bail!(
            "a scan is already running in another terminal (see `photosquirrel-cli scan status`)"
        );
    }
    let svc = ctx.service()?;
    let ids: Vec<SourceId> = sources.iter().map(|s| SourceId(*s)).collect();
    let run = if resume {
        let r = match run {
            Some(r) => ScanRunId(r),
            None => latest_resumable(&svc)?,
        };
        svc.resume_run(r)?
    } else {
        svc.start(&ids)?
    };
    foreground(ctx, &svc, run)
}

/// `scan resume [RUN]`: un-pause the live scan, or continue a run in the foreground.
pub fn resume(ctx: &Ctx, run: Option<i64>) -> Result<()> {
    if control::is_live(&ctx.data_dir()?, Job::Scan) {
        return control_live(ctx, "resume");
    }
    start(ctx, &[], true, run)
}

fn foreground(ctx: &Ctx, svc: &ScanService, run: ScanRunId) -> Result<()> {
    let data = ctx.data_dir()?;
    let tty = std::io::stderr().is_terminal();
    let mut last_out: Option<Instant> = None;
    let mut last_beat: Option<Instant> = None;
    let mut cancelled_by_interrupt = false;
    let due = |at: Option<Instant>, every: Duration| at.is_none_or(|t| t.elapsed() >= every);
    loop {
        if control::interrupted() && !cancelled_by_interrupt {
            cancelled_by_interrupt = true;
            let _ = svc.cancel();
        }
        if let Some(req) = control::take_request(&data, Job::Scan) {
            let r = match req.as_str() {
                "pause" => svc.pause().map(|_| ()),
                "resume" => svc.resume().map(|_| ()),
                "cancel" => svc.cancel().map(|_| ()),
                other => {
                    log::warn!("ignoring unknown scan request {other:?}");
                    Ok(())
                }
            };
            if let Err(e) = r {
                log::warn!("scan request {req}: {e}");
            }
        }
        let finished = svc.active().is_none();
        if let Some(snap) = svc.snapshot() {
            if due(last_beat, BEAT_EVERY) && !finished {
                control::beat(&data, Job::Scan, &snap);
                last_beat = Some(Instant::now());
            }
            if !finished {
                if ctx.json {
                    if due(last_out, JSON_EVERY) {
                        line("scan", "snapshot", &snap)?;
                        last_out = Some(Instant::now());
                    }
                } else if tty {
                    eprint!("\r\x1b[2K{}", progress_line(&snap));
                    std::io::stderr().flush()?;
                } else if due(last_out, PLAIN_EVERY) {
                    eprintln!("{}", progress_line(&snap));
                    last_out = Some(Instant::now());
                }
            }
        }
        if finished {
            break;
        }
        std::thread::sleep(TICK);
    }
    let summary = svc.wait();
    control::end(&data, Job::Scan);
    if tty && !ctx.json {
        eprintln!();
    }
    let summary = summary.context("the scan ended without a summary")?;
    if ctx.json {
        line("scan", "summary", &summary)?;
    } else {
        print_summary(run, &summary);
    }
    if cancelled_by_interrupt {
        return Err(Interrupted.into());
    }
    Ok(())
}

fn print_summary(run: ScanRunId, s: &RunSummary) {
    println!(
        "run {run} {}: {} files processed ({} read) in {}; {} new, {} changed, {} unchanged, {} missing, {} errors",
        s.status.as_str(),
        s.processed,
        bytes(s.bytes_read),
        duration_ms(s.elapsed_ms),
        s.diff.new,
        s.diff.changed,
        s.diff.unchanged,
        s.diff.missing,
        s.errors
    );
    if let Some(g) = &s.grouping {
        println!(
            "duplicate groups: {} groups with {} files, {} similar-shot suggestions ({} regrouped in {} ms)",
            g.groups, g.grouped_files, g.suggestions, g.changed, g.elapsed_ms
        );
    }
    if !s.incomplete_sources.is_empty() {
        println!(
            "incomplete (offline) sources: {:?}; run `photosquirrel-cli scan --resume` when they are back",
            s.incomplete_sources
        );
    }
    if s.status == RunStatus::Cancelled {
        println!("resume with: photosquirrel-cli scan --resume");
    }
}

#[derive(Serialize)]
struct ControlOut {
    requested: String,
    status: Option<RunStatus>,
    run: Option<ScanRunId>,
}

fn control_live(ctx: &Ctx, what: &str) -> Result<()> {
    let data = ctx.data_dir()?;
    if !control::is_live(&data, Job::Scan) {
        bail!("no scan is running in another terminal");
    }
    control::request(&data, Job::Scan, what)?;
    let want = match what {
        "pause" => Some(RunStatus::Paused),
        "resume" => Some(RunStatus::Running),
        _ => None,
    };
    let t = Instant::now();
    let mut last: Option<ProgressSnapshot> = None;
    while t.elapsed() < Duration::from_secs(5) {
        let snap: Option<ProgressSnapshot> = control::live(&data, Job::Scan);
        match (&snap, want) {
            (None, None) => break,
            (Some(s), Some(w)) if s.status == w => {
                last = snap;
                break;
            }
            (Some(s), None) if s.status == RunStatus::Cancelled => {
                last = snap;
                break;
            }
            _ => {}
        }
        last = snap;
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = ControlOut {
        requested: what.into(),
        status: last.as_ref().map(|s| s.status).or(match what {
            "cancel" => Some(RunStatus::Cancelled),
            _ => None,
        }),
        run: last.as_ref().map(|s| s.run),
    };
    emit(ctx.json, &format!("scan {what}"), &out, |o| {
        println!(
            "requested {}; scan is {}",
            o.requested,
            o.status.map_or("unknown", RunStatus::as_str)
        );
    })
}

/// `scan pause` / `scan cancel`.
pub fn control(ctx: &Ctx, what: &str) -> Result<()> {
    control_live(ctx, what)
}

#[derive(Serialize)]
pub struct RunRow {
    pub id: ScanRunId,
    pub status: RunStatus,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub sources: Vec<SourceId>,
    pub processed: Option<u64>,
    pub errors: Option<u64>,
    pub elapsed_ms: Option<u64>,
}

#[derive(Serialize)]
struct StatusOut {
    data_dir: PathBuf,
    /// The live scan's latest snapshot (a scan running in another terminal), if any.
    live: Option<ProgressSnapshot>,
    runs: Vec<RunRow>,
    sources: usize,
    groups: GroupTotals,
}

/// `status` / `scan status`.
pub fn status(ctx: &Ctx) -> Result<()> {
    let data = ctx.data_dir()?;
    let svc = ctx.reader()?;
    let runs: Vec<RunRow> = svc
        .runs()?
        .into_iter()
        .map(|r| {
            let s = RunSummary::from_json(&r.stats);
            RunRow {
                id: r.id,
                status: r.status,
                started_at_ms: r.started_at_ms,
                ended_at_ms: r.ended_at_ms,
                sources: r.sources,
                processed: s.as_ref().map(|s| s.processed),
                errors: s.as_ref().map(|s| s.errors),
                elapsed_ms: s.as_ref().map(|s| s.elapsed_ms),
            }
        })
        .collect();
    let out = StatusOut {
        live: control::live(&data, Job::Scan),
        sources: svc.sources()?.len(),
        groups: svc.groups().totals()?,
        runs,
        data_dir: data,
    };
    emit(ctx.json, "status", &out, |o| {
        println!("data folder: {}", o.data_dir.display());
        println!("sources: {}", o.sources);
        match &o.live {
            Some(s) => println!("live: {}", progress_line(s)),
            None => println!("live: no scan running"),
        }
        println!(
            "duplicate groups: {} ({} files, {} reclaimable, {} suggestions)",
            o.groups.groups,
            o.groups.grouped_files,
            bytes(o.groups.reclaimable_bytes),
            o.groups.suggestions
        );
        if o.runs.is_empty() {
            println!("no scan runs yet");
        }
        for r in o.runs.iter().rev().take(10) {
            println!(
                "run {:>4} {:<11} {} files processed, {} errors{}",
                r.id,
                r.status.as_str(),
                r.processed.unwrap_or(0),
                r.errors.unwrap_or(0),
                r.elapsed_ms
                    .map_or_else(String::new, |e| format!(", {}", duration_ms(e)))
            );
        }
    })
}
