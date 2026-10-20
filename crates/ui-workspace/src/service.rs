//! The shell's link to the engine (ADR-0027): a [`LibraryBackend`] driven from a dedicated service thread, so
//! no catalog or engine call ever runs on the UI thread (ADR-0005). Views ask through a [`ServiceHandle`] and
//! get a future; they render a loading state until it resolves.
//!
//! The backend is a trait because the catalog facade in `ps-service` grows with later changes: until the
//! `scanning`/`gallery` changes land, the app runs on [`EmptyBackend`] (an empty library, which shows
//! onboarding), and tests use fakes. A later change implements [`LibraryBackend`] for the real
//! `ps_service` facade and passes it in [`crate::ShellConfig::backend`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::BackgroundExecutor;
use ps_model::ScanRunId;

/// Catalog totals for the status bar and the "is the library empty" decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogSummary {
    pub sources: u32,
    pub photos: u64,
    pub videos: u64,
    pub duplicate_groups: u64,
    pub excluded: u64,
    pub bytes: u64,
}

impl CatalogSummary {
    pub fn is_empty(&self) -> bool {
        self.sources == 0 && self.photos == 0 && self.videos == 0
    }
}

/// A scan run left unfinished (killed mid-scan), offered for resume (ADR-0008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedRun {
    pub id: ScanRunId,
    /// What was being scanned ("NAS · Photos", or "3 sources").
    pub label: String,
    pub files_done: u64,
    pub files_total: Option<u64>,
}

impl InterruptedRun {
    /// Percent done, when the total is known.
    pub fn percent(&self) -> Option<u32> {
        let total = self.files_total.filter(|t| *t > 0)?;
        Some(((self.files_done.min(total) * 100) / total) as u32)
    }
}

/// The live-activity sample shown in the status bar (ADR-0019: a snapshot, never per-file events).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    /// "Scanning", "Hashing", "Exporting", ...
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
    pub paused: bool,
}

/// What the shell needs from the engine. Implementations block; they are only called on the service thread.
pub trait LibraryBackend: Send + Sync + 'static {
    fn summary(&self) -> anyhow::Result<CatalogSummary>;
    fn interrupted_run(&self) -> anyhow::Result<Option<InterruptedRun>>;
    fn resume_run(&self, run: ScanRunId) -> anyhow::Result<()>;
    /// Rescan every source (incremental, ADR-0007).
    fn rescan(&self) -> anyhow::Result<()>;
    fn add_source(&self, path: PathBuf) -> anyhow::Result<()>;
    /// The current activity snapshot, `None` when idle. Sampled at most every 100 ms while active.
    fn activity(&self) -> Option<ActivitySnapshot>;
    /// The gallery's data (ID lists, details, thumbnails, previews). Called once per window, then used
    /// from background tasks.
    fn gallery(self: Arc<Self>) -> Arc<dyn ui_gallery::source::GallerySource> {
        Arc::new(ui_gallery::source::EmptySource)
    }
    /// Scan control, export and reports for the Sources, Export and Reports views (`None`: those views
    /// explain that the engine is unavailable).
    fn scans(self: Arc<Self>) -> Option<crate::scan_backend::SharedScanBackend> {
        None
    }
    /// Duplicate review and AI review data (`None`: those views show an empty state).
    fn review(self: Arc<Self>) -> Option<Arc<dyn crate::review_source::ReviewSource>> {
        None
    }
}

/// An empty library with no engine behind it (until the catalog facade lands).
#[derive(Debug, Default)]
pub struct EmptyBackend;

impl LibraryBackend for EmptyBackend {
    fn summary(&self) -> anyhow::Result<CatalogSummary> {
        Ok(CatalogSummary::default())
    }
    fn interrupted_run(&self) -> anyhow::Result<Option<InterruptedRun>> {
        Ok(None)
    }
    fn resume_run(&self, _: ScanRunId) -> anyhow::Result<()> {
        anyhow::bail!("scanning is not available in this build yet")
    }
    fn rescan(&self) -> anyhow::Result<()> {
        anyhow::bail!("add a source first")
    }
    fn add_source(&self, path: PathBuf) -> anyhow::Result<()> {
        anyhow::bail!(
            "adding {} needs the scanning engine, which is not in this build yet",
            path.display()
        )
    }
    fn activity(&self) -> Option<ActivitySnapshot> {
        None
    }
}

type Job = Box<dyn FnOnce(&dyn LibraryBackend) + Send>;

/// The service thread stopped before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceGone;

/// A pending answer from the service thread.
///
/// It is polled on GPUI timers rather than woken from the service thread: GPUI's test scheduler rejects
/// wake-ups from foreign threads, and polling (1 ms, backing off to 8 ms, only while a call is pending)
/// keeps production latency negligible.
pub struct Reply<T> {
    slot: Arc<Mutex<Option<T>>>,
}

const POLL_MIN: Duration = Duration::from_millis(1);
const POLL_MAX: Duration = Duration::from_millis(8);

impl<T> Reply<T> {
    /// Take the answer if it has arrived; `Err` once the service can no longer answer.
    pub fn try_take(&self) -> Result<Option<T>, ServiceGone> {
        let value = self.slot.lock().unwrap_or_else(|p| p.into_inner()).take();
        match value {
            Some(v) => Ok(Some(v)),
            // Only we hold the slot: the job was dropped without answering.
            None if Arc::strong_count(&self.slot) == 1 => Err(ServiceGone),
            None => Ok(None),
        }
    }

    /// Wait for the answer without blocking the UI thread.
    pub async fn wait(self, executor: BackgroundExecutor) -> Result<T, ServiceGone> {
        let mut delay = POLL_MIN;
        loop {
            if let Some(v) = self.try_take()? {
                return Ok(v);
            }
            executor.timer(delay).await;
            delay = (delay * 2).min(POLL_MAX);
        }
    }

    /// Wait at most `limit` on the calling thread. The shell uses a few milliseconds of this at start-up so an
    /// instant answer (an empty or small catalog) paints in the first frame instead of flashing a loading state;
    /// anything slower falls through to [`Self::wait`].
    pub fn wait_briefly(&self, limit: Duration) -> Option<T> {
        let start = std::time::Instant::now();
        loop {
            if let Ok(Some(v)) = self.try_take() {
                return Some(v);
            }
            if start.elapsed() >= limit {
                return None;
            }
            std::thread::yield_now();
        }
    }

    /// Block the calling thread until the answer arrives (tests and the CLI, never the UI thread).
    pub fn wait_blocking(self) -> Result<T, ServiceGone> {
        loop {
            if let Some(v) = self.try_take()? {
                return Ok(v);
            }
            std::thread::sleep(POLL_MIN);
        }
    }
}

/// A cheap, cloneable handle to the service thread.
#[derive(Clone)]
pub struct ServiceHandle {
    tx: mpsc::Sender<Job>,
}

impl ServiceHandle {
    /// Start the service thread for `backend`. It ends when the last handle is dropped.
    pub fn spawn(backend: Arc<dyn LibraryBackend>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("ps-service".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(backend.as_ref());
                }
            });
        if let Err(err) = spawned {
            log::error!("could not start the service thread: {err}");
        }
        Self { tx }
    }

    /// Run `f` on the service thread.
    pub fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn LibraryBackend) -> T + Send + 'static,
    ) -> Reply<T> {
        let slot = Arc::new(Mutex::new(None));
        let theirs = slot.clone();
        let job: Job = Box::new(move |backend| {
            let value = f(backend);
            *theirs.lock().unwrap_or_else(|p| p.into_inner()) = Some(value);
        });
        if self.tx.send(job).is_err() {
            log::error!("the service thread has stopped");
        }
        Reply { slot }
    }
}

/// A value a view is waiting for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Loadable<T> {
    #[default]
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loadable<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loadable::Ready(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Loadable::Loading)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn calls_run_off_the_calling_thread() {
        let handle = ServiceHandle::spawn(Arc::new(EmptyBackend));
        let caller = std::thread::current().id();
        let reply = handle.call(move |b| {
            assert_ne!(std::thread::current().id(), caller);
            b.summary().unwrap()
        });
        let summary = reply.wait_blocking().unwrap();
        assert!(summary.is_empty());
    }

    #[test]
    fn percent_of_an_interrupted_run() {
        let run = InterruptedRun {
            id: ScanRunId(1),
            label: "x".into(),
            files_done: 41,
            files_total: Some(100),
        };
        assert_eq!(run.percent(), Some(41));
    }
}
