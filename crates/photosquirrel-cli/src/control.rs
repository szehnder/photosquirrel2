//! Cross-process control of a foreground scan or AI queue (headless-cli): the running command keeps a
//! heartbeat file with its latest progress in `<data-dir>/cli/` and polls a request file; `scan pause`,
//! `scan resume`, `scan cancel`, `ai pause` and the status commands in another terminal use them. Ctrl-C in
//! the running command sets [`interrupted`]; a second Ctrl-C exits at once (the catalog stays consistent,
//! ADR-0008).
//!
//! Only files inside the data directory are written here; sources and libraries are never touched.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Exit status after an interrupt.
pub const INTERRUPTED_EXIT: u8 = 130;

static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);

/// Install the Ctrl-C handler (once per process).
pub fn install_interrupt_handler() {
    let r = ctrlc::set_handler(|| {
        if INTERRUPTS.fetch_add(1, Ordering::SeqCst) >= 1 {
            eprintln!("\ninterrupted again: exiting now (the catalog stays consistent)");
            std::process::exit(i32::from(INTERRUPTED_EXIT));
        }
        eprintln!("\ninterrupt: stopping cleanly (press Ctrl-C again to exit at once)");
    });
    if let Err(e) = r {
        log::warn!("cannot install the Ctrl-C handler: {e}");
    }
}

/// Ctrl-C was pressed.
pub fn interrupted() -> bool {
    INTERRUPTS.load(Ordering::SeqCst) > 0
}

/// The command ended because of an interrupt (exit status 130).
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// A foreground job: `scan` or `ai`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Scan,
    Ai,
}

impl Job {
    fn name(self) -> &'static str {
        match self {
            Job::Scan => "scan",
            Job::Ai => "ai",
        }
    }
}

fn dir(data: &Path) -> PathBuf {
    data.join("cli")
}

fn heartbeat_path(data: &Path, job: Job) -> PathBuf {
    dir(data).join(format!("{}.json", job.name()))
}

fn request_path(data: &Path, job: Job) -> PathBuf {
    dir(data).join(format!("{}.request", job.name()))
}

/// A heartbeat older than this means the job is gone (crashed or killed).
pub const STALE: Duration = Duration::from_secs(3);

#[derive(Serialize, Deserialize)]
struct Beat<T> {
    pid: u32,
    at_ms: u64,
    progress: T,
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Record the running job's progress (call every few hundred ms).
pub fn beat<T: Serialize>(data: &Path, job: Job, progress: &T) {
    let b = Beat {
        pid: std::process::id(),
        at_ms: now_ms(),
        progress,
    };
    if let Ok(bytes) = serde_json::to_vec(&b)
        && let Err(e) = write_atomic(&heartbeat_path(data, job), &bytes)
    {
        log::debug!("heartbeat: {e}");
    }
}

/// The job ended: remove its heartbeat and any pending request.
pub fn end(data: &Path, job: Job) {
    let _ = std::fs::remove_file(heartbeat_path(data, job));
    let _ = std::fs::remove_file(request_path(data, job));
}

/// The live job's latest progress, if a job is running (fresh heartbeat).
pub fn live<T: DeserializeOwned>(data: &Path, job: Job) -> Option<T> {
    let bytes = std::fs::read(heartbeat_path(data, job)).ok()?;
    let b: Beat<T> = serde_json::from_slice(&bytes).ok()?;
    (now_ms().saturating_sub(b.at_ms) <= STALE.as_millis() as u64).then_some(b.progress)
}

/// A job is running in another process.
pub fn is_live(data: &Path, job: Job) -> bool {
    live::<serde_json::Value>(data, job).is_some()
}

/// Ask the live job to `pause`, `resume` or `cancel`.
pub fn request(data: &Path, job: Job, what: &str) -> std::io::Result<()> {
    write_atomic(&request_path(data, job), what.as_bytes())
}

/// Take a pending request (the running job polls this).
pub fn take_request(data: &Path, job: Job) -> Option<String> {
    let p = request_path(data, job);
    let s = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    Some(s.trim().to_owned())
}
