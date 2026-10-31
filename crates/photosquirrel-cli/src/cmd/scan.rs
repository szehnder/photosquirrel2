//! `scan [start|resume|pause|cancel|status]` and `status`.
//!
//! A scan runs in the foreground of the command that started it: a progress line at most 10 times per second
//! (redrawn on a terminal, every 5 s otherwise), NDJSON snapshot lines with `--json` (at least 100 ms apart)
//! and a final summary line. Ctrl-C cancels cooperatively (exit 130, the run stays resumable with
//! `scan --resume`). Other terminals pause, resume or cancel it through the data folder ([`crate::control`]).
//!
//! With `general.pause_on_battery` on (and no `--ignore-battery`), the foreground loop applies the app's
//! pause-on-battery rule every second and resumes a run it paused once mains power is back (ADR-0072): one line on
//! standard error per transition, a `power` NDJSON line with `--json`, and paused snapshots at most every 5 s.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use ps_model::{DiffCounts, ProgressSnapshot, RunStatus, ScanPhase, ScanRunId, SourceId};
use ps_service::groups::GroupTotals;
use ps_service::power::{PowerEvent, PowerPause, PowerPolicy, PowerSource, probe_from_env};
use ps_service::scan::{RunSummary, ScanService};

use super::Ctx;
use crate::control::{self, Interrupted, Job};
use crate::out::{bytes, duration_ms, emit, line};

const TICK: Duration = Duration::from_millis(100);
const JSON_EVERY: Duration = Duration::from_millis(200);
const PLAIN_EVERY: Duration = Duration::from_secs(5);
const BEAT_EVERY: Duration = Duration::from_millis(250);
/// While the run is paused (for any reason), snapshot lines and non-terminal progress lines at most this often.
const PAUSED_EVERY: Duration = Duration::from_secs(5);
/// How often a foreground scan asks the power source (ADR-0072).
const POWER_EVERY: Duration = Duration::from_secs(1);

/// The heartbeat of a foreground scan: its latest snapshot (fields at the top level, where `scan pause` and
/// `scan resume` in another terminal read them) and why it is paused for power, if it is.
#[derive(Serialize, Deserialize)]
pub struct LiveScan {
    #[serde(flatten)]
    pub snapshot: ProgressSnapshot,
    /// `"on_battery"` while the run is paused because the machine is on battery power.
    #[serde(default)]
    pub power_pause: Option<String>,
}

impl LiveScan {
    fn on_battery(&self) -> bool {
        self.power_pause.as_deref() == Some(ON_BATTERY)
    }
}

const ON_BATTERY: &str = "on_battery";

/// A `power` NDJSON line: the run paused or resumed for power.
#[derive(Serialize)]
struct PowerLine {
    event: &'static str,
    run: ScanRunId,
    at_ms: u64,
}

/// Pause on battery for one foreground scan: the shared rule, the probe and when it was last asked.
struct PowerGuard {
    policy: PowerPolicy,
    source: Arc<dyn PowerSource>,
    last_poll: Option<Instant>,
}

impl PowerGuard {
    /// The guard for `general.pause_on_battery` and `--ignore-battery`; none when the rule is off. With the flag
    /// and the setting on, says once on standard error that battery power is being ignored.
    fn new(pause_on_battery: bool, ignore_battery: bool) -> Option<Self> {
        if !pause_on_battery {
            return None;
        }
        let source = probe_from_env();
        if ignore_battery {
            if source.on_battery() {
                eprintln!(
                    "on battery power: ignoring it (--ignore-battery), the scan does not pause"
                );
            }
            return None;
        }
        Some(Self {
            policy: PowerPolicy::for_cli(pause_on_battery, ignore_battery)?,
            source,
            last_poll: None,
        })
    }

    /// Ask the probe if a poll is due: on battery or not. Apply the reading with [`PowerGuard::apply`] after the
    /// pending control requests.
    fn probe(&mut self) -> Option<bool> {
        if self.last_poll.is_some_and(|t| t.elapsed() < POWER_EVERY) {
            return None;
        }
        self.last_poll = Some(Instant::now());
        Some(self.source.on_battery())
    }

    /// Apply the rule to one probe reading.
    fn apply(&mut self, svc: &ScanService, on_battery: bool) -> Option<PowerEvent> {
        self.policy.tick(svc, on_battery)
    }

    fn on_battery_pause(&self) -> bool {
        matches!(self.policy.note(), Some(PowerPause::OnBattery(_)))
    }
}

/// Say that the run paused or resumed for power: standard error always (it is for people and logs), a `power`
/// line with `--json`.
fn report_power(ctx: &Ctx, tty: bool, ev: PowerEvent) -> Result<()> {
    let (event, run, text) = match ev {
        PowerEvent::PausedOnBattery(run) => (
            "paused_on_battery",
            run,
            "paused: this computer is on battery power (general.pause_on_battery); the scan resumes when \
             mains power returns, or run with --ignore-battery",
        ),
        PowerEvent::ResumedOnMains(run) => (
            "resumed_on_mains",
            run,
            "mains power is back: resuming the scan",
        ),
        // The CLI resumes by itself (`ResumeOnMains::Auto`); an offer is the app's.
        PowerEvent::MainsBack(_) => return Ok(()),
    };
    if tty && !ctx.json {
        eprint!("\r\x1b[2K");
    }
    eprintln!("{text}");
    if ctx.json {
        line(
            "scan",
            "power",
            &PowerLine {
                event,
                run,
                at_ms: control::now_ms(),
            },
        )?;
    }
    Ok(())
}

fn phase(p: ScanPhase) -> &'static str {
    match p {
        ScanPhase::Starting => "starting",
        ScanPhase::Walking => "walking",
        ScanPhase::Analyzing => "analyzing",
        ScanPhase::Finishing => "finishing",
        ScanPhase::Done => "done",
    }
}

/// One compact progress line (`on_battery`: the run is paused for battery power).
pub fn progress_line(s: &ProgressSnapshot, on_battery: bool) -> String {
    let eta = s
        .eta_ms
        .map_or_else(String::new, |e| format!("  ETA {}", duration_ms(e)));
    format!(
        "{} {} [{}{}] {}/{} files  {}/s  {:.0} files/s  {} read{}  errors {}",
        if s.fill {
            "filling in previews, run"
        } else {
            "run"
        },
        s.run,
        phase(s.phase),
        match (s.status, on_battery) {
            (RunStatus::Paused, true) => ", paused on battery",
            (RunStatus::Paused, false) => ", paused",
            _ => "",
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
pub fn start(
    ctx: &Ctx,
    sources: &[i64],
    resume: bool,
    run: Option<i64>,
    ignore_battery: bool,
    refresh_analysis: bool,
) -> Result<()> {
    let data = ctx.data_dir()?;
    if control::is_live(&data, Job::Scan) {
        // Another process owns the catalog: exit 2 like every other refused mutation (ADR-0041).
        return Err(super::Busy(
            "a scan is already running in another terminal (see `photosquirrel-cli scan status`)"
                .into(),
        )
        .into());
    }
    let svc = ctx.service()?;
    if refresh_analysis {
        // This process's runs (a new one or a resumed one) refresh old rows' analysis (ADR-0071).
        svc.engine().update_options(|o| o.refresh_analysis = true);
    }
    let power = PowerGuard::new(ctx.settings()?.general.pause_on_battery, ignore_battery);
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
    foreground(ctx, &svc, run, power)
}

/// `scan resume [RUN]`: un-pause the live scan, or continue a run in the foreground.
pub fn resume(ctx: &Ctx, run: Option<i64>, ignore_battery: bool) -> Result<()> {
    if control::is_live(&ctx.data_dir()?, Job::Scan) {
        if ignore_battery {
            eprintln!(
                "--ignore-battery has no effect here: the scan running in another terminal keeps its own pause on \
                 battery; this only asks it to resume (on battery, it then keeps running until mains power returns)"
            );
        }
        return control_live(ctx, "resume");
    }
    start(ctx, &[], true, run, ignore_battery, false)
}

fn foreground(
    ctx: &Ctx,
    svc: &ScanService,
    run: ScanRunId,
    power: Option<PowerGuard>,
) -> Result<()> {
    foreground_as(ctx, svc, run, power, "scan")
}

/// A Fill in previews job in the foreground (`previews fill`, ADR-0109): the scan's progress lines, control
/// requests, pause on battery and Ctrl-C handling, then the job's summary.
pub fn foreground_fill(
    ctx: &Ctx,
    svc: &ScanService,
    run: ScanRunId,
    ignore_battery: bool,
) -> Result<()> {
    let power = PowerGuard::new(ctx.settings()?.general.pause_on_battery, ignore_battery);
    foreground_as(ctx, svc, run, power, "previews fill")
}

fn foreground_as(
    ctx: &Ctx,
    svc: &ScanService,
    run: ScanRunId,
    mut power: Option<PowerGuard>,
    command: &str,
) -> Result<()> {
    let data = ctx.data_dir()?;
    let tty = std::io::stderr().is_terminal();
    let mut last_out: Option<Instant> = None;
    let mut last_beat: Option<Instant> = None;
    let mut cancelled_by_interrupt = false;
    // A cancel was asked for (Ctrl-C or a `cancel` request): no power action follows it.
    let mut cancel_requested = false;
    let due = |at: Option<Instant>, every: Duration| at.is_none_or(|t| t.elapsed() >= every);
    loop {
        if control::interrupted() && !cancelled_by_interrupt {
            cancelled_by_interrupt = true;
            cancel_requested = true;
            let _ = svc.cancel();
        }
        // The probe is read before the pending requests are taken, and applied after them: a request written
        // before the probe saw mains power return (`scan pause` replies at once when the run is already paused for
        // power) is always applied first, so a user pause wins over the automatic resume.
        let reading = if cancel_requested {
            None
        } else {
            power.as_mut().and_then(PowerGuard::probe)
        };
        if let Some(req) = control::take_request(&data, Job::Scan) {
            let r = match req.as_str() {
                // The pause is the user's: mains power must not resume it.
                "pause" => svc.pause().map(|r| {
                    if let Some(g) = power.as_mut() {
                        g.policy.user_paused(r);
                    }
                }),
                // On battery after a power pause, the rule leaves a resumed run running until mains returns.
                "resume" => svc.resume().map(|_| ()),
                "cancel" => {
                    cancel_requested = true;
                    svc.cancel().map(|_| ())
                }
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
        if !finished
            && !cancel_requested
            && let Some(on_battery) = reading
            && let Some(ev) = power.as_mut().and_then(|g| g.apply(svc, on_battery))
        {
            report_power(ctx, tty, ev)?;
        }
        let on_battery = power.as_ref().is_some_and(PowerGuard::on_battery_pause);
        if let Some(snap) = svc.snapshot() {
            if due(last_beat, BEAT_EVERY) && !finished {
                let live = LiveScan {
                    snapshot: snap.clone(),
                    power_pause: on_battery.then(|| ON_BATTERY.to_owned()),
                };
                control::beat(&data, Job::Scan, &live);
                last_beat = Some(Instant::now());
            }
            if !finished {
                // A paused snapshot carries no news: at most one every 5 s.
                let paused = snap.status == RunStatus::Paused;
                if ctx.json {
                    if due(last_out, if paused { PAUSED_EVERY } else { JSON_EVERY }) {
                        line(command, "snapshot", &snap)?;
                        last_out = Some(Instant::now());
                    }
                } else if tty {
                    eprint!("\r\x1b[2K{}", progress_line(&snap, on_battery));
                    std::io::stderr().flush()?;
                } else if due(last_out, if paused { PAUSED_EVERY } else { PLAIN_EVERY }) {
                    eprintln!("{}", progress_line(&snap, on_battery));
                    last_out = Some(Instant::now());
                }
            } else if ctx.json {
                // The run's last snapshot, so its `recent_errors` are never missed between samples.
                line(command, "snapshot", &snap)?;
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
    if let Some(fill) = &summary.fill {
        if ctx.json {
            line(command, "summary", fill)?;
        } else {
            super::previews::print_fill_summary(run, &summary, fill);
        }
    } else if ctx.json {
        line(command, "summary", &summary)?;
    } else {
        // A refresh run: this process asked for one, or the resumed run recorded it (ADR-0082).
        let refresh = svc.engine().options().refresh_analysis
            || svc
                .runs()
                .ok()
                .and_then(|rs| rs.into_iter().find(|r| r.id == run))
                .is_some_and(|r| r.refresh_analysis);
        print_summary(run, &summary, refresh);
    }
    if cancelled_by_interrupt {
        return Err(Interrupted.into());
    }
    Ok(())
}

/// The summary's line about damaged files (ADR-0115): catalogued and copied as is, keep the original.
pub fn damaged_line(n: u64) -> Option<String> {
    (n > 0).then(|| {
        let (files, it) = if n == 1 {
            ("1 damaged file".to_owned(), "it")
        } else {
            (format!("{n} damaged files"), "them")
        };
        format!(
            "{files} could not be read (cut short or corrupt). {} catalogued and copied as is; keep the original \
             card or drive until you have checked {it}. List: `photosquirrel-cli report`",
            if n == 1 { "It is" } else { "They are" }
        )
    })
}

fn print_summary(run: ScanRunId, s: &RunSummary, refresh: bool) {
    println!(
        "run {run} {}: {} files processed ({} read) in {}; {} new, {} changed, {} unchanged, {} missing, {} \
         quarantined, {} errors",
        s.status.as_str(),
        s.processed,
        bytes(s.bytes_read),
        duration_ms(s.elapsed_ms),
        s.diff.new,
        s.diff.changed,
        s.diff.unchanged,
        s.diff.missing,
        s.diff.quarantined,
        s.errors
    );
    if let Some(l) = damaged_line(s.damaged) {
        println!("{l}");
    }
    if s.diff.redated > 0 {
        println!(
            "{} capture dates updated by the current date rules (folder names, local days; no file read)",
            s.diff.redated
        );
    }
    if s.diff.reanalyzed > 0 {
        println!(
            "{} analysed again (TIFFs catalogued without dimensions by an earlier version, or perceptual hashes of an \
             older version whose previews were converted to sRGB)",
            files(s.diff.reanalyzed)
        );
    }
    if s.diff.colour_checked > 0 {
        println!(
            "{} checked once for their colour space (read-only); {} not in sRGB had their previews remade ({} read)",
            files(s.diff.colour_checked),
            s.diff.recoloured,
            bytes(s.diff.colour_bytes)
        );
    }
    if s.diff.posters > 0 {
        println!(
            "{} waiting for a poster-frame tool: {} got their poster frame, {} failed (one ffmpeg call each, no file \
             read)",
            if s.diff.posters == 1 {
                "1 video".to_owned()
            } else {
                format!("{} videos", s.diff.posters)
            },
            s.diff.posters_made,
            s.diff.posters_failed
        );
    }
    if s.diff.frame_reread > 0 {
        let split = s.grouping.as_ref().map_or(0, |g| g.groups_split);
        println!(
            "{} re-read for frame numbers and cut RAW files (header only, read-only), {} split",
            files(s.diff.frame_reread),
            if split == 1 {
                "1 group".to_owned()
            } else {
                format!("{split} groups")
            }
        );
    }
    for l in refresh_lines(&s.diff, s.status, refresh) {
        println!("{l}");
    }
    for l in undecoded_lines(s) {
        println!("{l}");
    }
    if let Some(g) = &s.grouping {
        println!(
            "duplicate groups: {} groups with {} files, {} similar-shot suggestions ({} regrouped in {} ms)",
            g.groups, g.grouped_files, g.suggestions, g.changed, g.elapsed_ms
        );
    }
    if !s.incomplete_sources.is_empty() {
        println!(
            "incomplete (offline) sources: {}; run `photosquirrel-cli scan --resume` when they are back",
            s.incomplete_sources
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if s.status == RunStatus::Cancelled {
        println!("resume with: photosquirrel-cli scan --resume");
    }
}

/// The summary lines about photos without a preview because their decoder is missing (ADR-0091): per format,
/// the count and reason and how to enable previews, or, when the decoder is available now, which sources to
/// rescan.
pub fn undecoded_lines(s: &RunSummary) -> Vec<String> {
    let mut out = Vec::new();
    if s.diff.redecoded > 0 {
        out.push(format!(
            "{} previews filled in (photos that waited for a decoder)",
            s.diff.redecoded
        ));
    }
    for u in &s.undecoded {
        let label = u.key.label();
        let photos = if u.count == 1 {
            format!("1 {label} photo has")
        } else {
            format!("{} {label} photos have", u.count)
        };
        if u.available {
            let sources = u
                .sources
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            out.push(format!(
                "{label} decoder found: rescan source {sources} to fill in {} previews.",
                u.count
            ));
            continue;
        }
        out.push(format!(
            "{photos} no preview: {} (they are still catalogued, de-duplicated and exported).",
            u.reason.as_deref().unwrap_or("no decoder on this system")
        ));
        if let Some(fix) = &u.fix {
            out.push(format!("How to enable {label} previews:"));
            if !fix.summary.is_empty() {
                out.push(format!("  {}", fix.summary));
            }
            for l in fix.lines() {
                out.push(format!("  {l}"));
            }
        }
    }
    out
}

fn files(n: u64) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

/// The summary lines about analysis refresh (ADR-0071). `refresh` says the run refreshed analysis: the files
/// still waiting after a completed refresh run are the ones it could not read (their errors are the run's).
pub fn refresh_lines(d: &DiffCounts, status: RunStatus, refresh: bool) -> Vec<String> {
    let mut out = Vec::new();
    if d.refreshed > 0 {
        out.push(format!(
            "refreshed the analysis of {} analysed by an older version (read up to 256 KB of each)",
            files(d.refreshed)
        ));
    }
    let still = d.still_needs_refresh();
    if still == 0 {
        return out;
    }
    let (were, keep) = if still == 1 {
        ("was", "keeps its")
    } else {
        ("were", "keep their")
    };
    if refresh && status == RunStatus::Completed {
        out.push(format!(
            "{} analysed by an older version could not be read and {keep} class; see this run's errors, then \
             rescan with `photosquirrel-cli scan --refresh-analysis` to try again",
            files(still)
        ));
    } else {
        out.push(format!(
            "{} {were} analysed by an older version and {keep} class; rescan with \
             `photosquirrel-cli scan --refresh-analysis` to re-classify them (reads up to 256 KB of each)",
            files(still)
        ));
    }
    out
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
    /// The live scan's latest snapshot and power pause (a scan running in another terminal), if any.
    live: Option<LiveScan>,
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
            Some(s) => println!("live: {}", progress_line(&s.snapshot, s.on_battery())),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(needs_refresh: u64, refreshed: u64) -> DiffCounts {
        DiffCounts {
            needs_refresh,
            refreshed,
            ..DiffCounts::default()
        }
    }

    #[test]
    fn refresh_lines_say_one_file_in_the_singular() {
        let l = refresh_lines(&diff(1, 0), RunStatus::Completed, false);
        assert_eq!(l.len(), 1);
        assert!(
            l[0].starts_with("1 file was analysed by an older version and keeps its class;"),
            "{l:?}"
        );
        let l = refresh_lines(&diff(3, 0), RunStatus::Completed, false);
        assert!(
            l[0].starts_with("3 files were analysed by an older version and keep their class;"),
            "{l:?}"
        );
        let l = refresh_lines(&diff(1, 1), RunStatus::Completed, true);
        assert_eq!(
            l,
            [
                "refreshed the analysis of 1 file analysed by an older version (read up to 256 KB of each)"
            ]
        );
        assert!(refresh_lines(&diff(0, 0), RunStatus::Completed, false).is_empty());
    }

    #[test]
    fn refresh_lines_say_unreadable_files_after_a_refresh_run() {
        let l = refresh_lines(&diff(5, 3), RunStatus::Completed, true);
        assert_eq!(l.len(), 2, "{l:?}");
        assert!(l[0].contains("refreshed the analysis of 3 files"), "{l:?}");
        assert!(
            l[1].starts_with(
                "2 files analysed by an older version could not be read and keep their class"
            ),
            "{l:?}"
        );
        assert!(l[1].contains("see this run's errors"), "{l:?}");
        let l = refresh_lines(&diff(1, 0), RunStatus::Completed, true);
        assert!(l[0].starts_with(
            "1 file analysed by an older version could not be read and keeps its class"
        ));
        // A cancelled refresh run did not try them all: it says they still need one.
        let l = refresh_lines(&diff(5, 3), RunStatus::Cancelled, true);
        assert!(
            l[1].starts_with("2 files were analysed by an older version"),
            "{l:?}"
        );
    }
}
