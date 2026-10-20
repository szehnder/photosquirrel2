//! The AI queue engine (spec `ai-queue`): explicit start, pause, a concurrency semaphore, a requests-per-minute
//! token bucket, one result per content identity, bounded retries (in the HTTP layer) and progress snapshots.
//!
//! Persistence is behind [`QueueStore`]: the catalog-backed store (`ai_queue`/`ai_result` tables, catalog
//! migration 5) lives in `ps-service::ai`; [`MemoryStore`] serves tests and headless one-off runs. The engine
//! runs on the `ps-llm` tokio runtime (ADR-0009); store and preview-loader calls happen on blocking threads.
//! Nothing is sent before [`AiQueue::start`], and a hosted provider without consent refuses to start
//! (ADR-0021).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use ps_model::settings::Ai;
use ps_model::{ContentId, ImageInsight};
use tokio::sync::{Notify, Semaphore};

use crate::config::ProviderConfig;
use crate::consent::{ConsentRequired, consent_prompt};
use crate::describe::{Described, Describer, Provenance};
use crate::limits::RateLimiter;
use crate::preview::{PreviewError, PreviewJpeg};

/// One queued content identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueItem {
    pub content: ContentId,
    /// Lower runs first (keepers of class photo/likely photo first, then by capture date).
    pub priority: i64,
    /// Requests already spent on it in earlier runs.
    pub attempts: u32,
}

/// Results are unique per content, provider, model and schema version (spec "One result per content").
/// `prompt_version` is the prompt this queue sends: a pending item whose stored result came from an older
/// prompt is described again (it is pending only when the user asked to re-describe it; older results are
/// never queued again on their own, ai-queue "Prompt upgrades are explicit").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResultKey {
    pub provider: String,
    pub model: String,
    pub schema_version: u32,
    pub prompt_version: u32,
}

/// Item counts by state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueCounts {
    pub pending: u64,
    pub in_flight: u64,
    pub done: u64,
    pub failed: u64,
    /// Waiting for an offline source.
    pub deferred: u64,
}

/// Where queue state is persisted. Calls must be short (they run on a blocking thread, one at a time per item).
pub trait QueueStore: Send + Sync {
    /// At engine creation: items left in flight by a crash or quit become pending again.
    fn recover(&self);
    /// Claim the first pending item in priority order, marking it in flight.
    fn claim_next(&self) -> Option<QueueItem>;
    /// Return a claimed item to pending without spending an attempt (paused before sending).
    fn release(&self, item: &QueueItem);
    /// A result for this content and key exists already, from this prompt version or a newer one.
    fn has_result(&self, content: &ContentId, key: &ResultKey) -> bool;
    /// Store the insight and mark the item done.
    fn complete(&self, item: &QueueItem, described: &Described);
    /// Mark done without a new result (already described).
    fn mark_done(&self, item: &QueueItem);
    /// Mark failed with the error, after `requests` more requests.
    fn fail(&self, item: &QueueItem, requests: u32, error: &str);
    /// Set aside (source offline); pending again on the next start.
    fn defer(&self, item: &QueueItem, reason: &str);
    fn counts(&self) -> QueueCounts;
}

/// Produces the 1024 px preview of a content identity (from the preview cache or the source).
pub trait PreviewLoader: Send + Sync {
    fn load(&self, content: &ContentId) -> Result<PreviewJpeg, PreviewError>;
}

impl<F> PreviewLoader for F
where
    F: Fn(&ContentId) -> Result<PreviewJpeg, PreviewError> + Send + Sync,
{
    fn load(&self, content: &ContentId) -> Result<PreviewJpeg, PreviewError> {
        self(content)
    }
}

/// Queue limits and the consent gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueConfig {
    /// Maximum requests in flight (default 2).
    pub concurrency: usize,
    /// `None`: unlimited.
    pub requests_per_minute: Option<u32>,
    pub provider_name: String,
    /// Previews would leave the machine.
    pub hosted: bool,
    pub hosted_consent: bool,
}

/// Default requests per minute for hosted providers (spec "Concurrency and rate limits").
pub const HOSTED_DEFAULT_RPM: u32 = ps_model::settings::HOSTED_DEFAULT_REQUESTS_PER_MINUTE;

impl QueueConfig {
    pub fn local(provider_name: &str) -> Self {
        Self {
            concurrency: 2,
            requests_per_minute: None,
            provider_name: provider_name.to_owned(),
            hosted: false,
            hosted_consent: false,
        }
    }

    /// From the AI settings and the provider configuration built from them.
    pub fn from_settings(ai: &Ai, provider: &ProviderConfig) -> Self {
        let hosted = provider.is_hosted();
        Self {
            concurrency: ai.max_concurrent_requests.clamp(1, 64) as usize,
            requests_per_minute: ai.effective_requests_per_minute().filter(|n| *n > 0),
            provider_name: provider.name.clone(),
            hosted,
            hosted_consent: ai.hosted_consent,
        }
    }
}

/// Why the queue did not start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    ConsentRequired(ConsentRequired),
}

/// The queue's run state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueState {
    /// Not running: never started, paused by the user, or after a restart.
    Paused,
    Running,
    /// Started and nothing is left to do.
    Finished,
}

/// A progress snapshot (spec "Queue progress").
#[derive(Clone, Debug, PartialEq)]
pub struct QueueProgress {
    pub state: QueueState,
    pub counts: QueueCounts,
    /// Requests started in this process.
    pub requests_sent: u64,
    /// Items finished in the last minute.
    pub per_minute: f64,
    pub eta: Option<Duration>,
}

struct Inner {
    store: Arc<dyn QueueStore>,
    loader: Arc<dyn PreviewLoader>,
    describer: Describer,
    key: ResultKey,
    cfg: QueueConfig,
    running: AtomicBool,
    finished: AtomicBool,
    runner_active: AtomicBool,
    in_flight: AtomicUsize,
    requests_sent: AtomicU64,
    sem: Arc<Semaphore>,
    limiter: RateLimiter,
    finished_at: Mutex<VecDeque<Instant>>,
    changed: Notify,
    rt: tokio::runtime::Handle,
}

/// The AI queue. Cheap to clone.
#[derive(Clone)]
pub struct AiQueue {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AiQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiQueue")
            .field("provider", &self.inner.cfg.provider_name)
            .field("state", &self.state())
            .finish()
    }
}

impl AiQueue {
    /// A paused queue. Recovers items left in flight by an earlier process.
    pub fn new(
        store: Arc<dyn QueueStore>,
        loader: Arc<dyn PreviewLoader>,
        describer: Describer,
        cfg: QueueConfig,
        rt: tokio::runtime::Handle,
    ) -> Self {
        store.recover();
        let key = ResultKey {
            provider: describer.provider().name().to_owned(),
            model: describer.provider().configured_model().to_owned(),
            schema_version: describer.prompt().schema_version,
            prompt_version: describer.prompt().version,
        };
        let concurrency = cfg.concurrency.max(1);
        let limiter = RateLimiter::new(cfg.requests_per_minute);
        Self {
            inner: Arc::new(Inner {
                store,
                loader,
                describer,
                key,
                cfg,
                running: AtomicBool::new(false),
                finished: AtomicBool::new(false),
                runner_active: AtomicBool::new(false),
                in_flight: AtomicUsize::new(0),
                requests_sent: AtomicU64::new(0),
                sem: Arc::new(Semaphore::new(concurrency)),
                limiter,
                finished_at: Mutex::new(VecDeque::new()),
                changed: Notify::new(),
                rt,
            }),
        }
    }

    /// The key results of this queue are stored under.
    pub fn result_key(&self) -> &ResultKey {
        &self.inner.key
    }

    /// Start (or resume). A hosted provider needs consent first; then nothing is started and nothing sent.
    pub fn start(&self) -> Result<(), StartError> {
        let cfg = &self.inner.cfg;
        if cfg.hosted && !cfg.hosted_consent {
            return Err(StartError::ConsentRequired(consent_prompt(
                &cfg.provider_name,
            )));
        }
        self.inner.finished.store(false, Ordering::SeqCst);
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        if !self.inner.runner_active.swap(true, Ordering::SeqCst) {
            let inner = self.inner.clone();
            self.inner.rt.spawn(run(inner));
        }
        Ok(())
    }

    /// Stop starting new requests; in-flight requests finish and are stored.
    pub fn pause(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
        self.inner.changed.notify_waiters();
    }

    pub fn state(&self) -> QueueState {
        if self.inner.running.load(Ordering::SeqCst) {
            QueueState::Running
        } else if self.inner.finished.load(Ordering::SeqCst) {
            QueueState::Finished
        } else {
            QueueState::Paused
        }
    }

    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    /// Block (from a non-runtime thread) until the queue is not running and nothing is in flight.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !self.inner.running.load(Ordering::SeqCst)
                && self.in_flight() == 0
                && !self.inner.runner_active.load(Ordering::SeqCst)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    pub fn progress(&self) -> QueueProgress {
        let counts = self.inner.store.counts();
        let per_minute = {
            let mut q = lock(&self.inner.finished_at);
            let cutoff = Instant::now().checked_sub(Duration::from_secs(60));
            while q.front().is_some_and(|t| cutoff.is_some_and(|c| *t < c)) {
                q.pop_front();
            }
            q.len() as f64
        };
        let eta = (per_minute > 0.0 && counts.pending > 0)
            .then(|| Duration::from_secs_f64(counts.pending as f64 / per_minute * 60.0));
        QueueProgress {
            state: self.state(),
            counts,
            requests_sent: self.inner.requests_sent.load(Ordering::SeqCst),
            per_minute,
            eta,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

async fn run(inner: Arc<Inner>) {
    loop {
        run_once(&inner).await;
        inner.runner_active.store(false, Ordering::SeqCst);
        // A start() between the loop's last check and the flag reset must not be lost.
        if inner.running.load(Ordering::SeqCst) && !inner.runner_active.swap(true, Ordering::SeqCst)
        {
            continue;
        }
        break;
    }
}

async fn run_once(inner: &Arc<Inner>) {
    while inner.running.load(Ordering::SeqCst) {
        let Ok(permit) = inner.sem.clone().acquire_owned().await else {
            return;
        };
        if !inner.running.load(Ordering::SeqCst) {
            return;
        }
        let store = inner.store.clone();
        let item = blocking(move || store.claim_next()).await.flatten();
        let Some(item) = item else {
            drop(permit);
            if inner.in_flight.load(Ordering::SeqCst) == 0 {
                // Nothing pending, nothing in flight: done.
                inner.finished.store(true, Ordering::SeqCst);
                inner.running.store(false, Ordering::SeqCst);
                log::info!("{}: AI queue finished", inner.cfg.provider_name);
                return;
            }
            let _ =
                tokio::time::timeout(Duration::from_millis(100), inner.changed.notified()).await;
            continue;
        };
        inner.in_flight.fetch_add(1, Ordering::SeqCst);
        let task_inner = inner.clone();
        inner.rt.spawn(async move {
            process(&task_inner, item).await;
            task_inner.in_flight.fetch_sub(1, Ordering::SeqCst);
            drop(permit);
            task_inner.changed.notify_waiters();
        });
    }
}

async fn process(inner: &Arc<Inner>, item: QueueItem) {
    let store = inner.store.clone();
    let (content, key) = (item.content, inner.key.clone());
    if blocking(move || store.has_result(&content, &key)).await == Some(true) {
        let store = inner.store.clone();
        blocking(move || store.mark_done(&item)).await;
        return;
    }
    let loader = inner.loader.clone();
    let preview = blocking(move || loader.load(&content)).await;
    let preview = match preview {
        Some(Ok(p)) => p,
        Some(Err(PreviewError::Deferred(reason))) => {
            let store = inner.store.clone();
            blocking(move || store.defer(&item, &reason)).await;
            return;
        }
        Some(Err(PreviewError::Failed(e))) => {
            let store = inner.store.clone();
            blocking(move || store.fail(&item, 0, &format!("no preview: {e}"))).await;
            return;
        }
        None => {
            let store = inner.store.clone();
            blocking(move || store.release(&item)).await;
            return;
        }
    };
    inner.limiter.acquire().await;
    if !inner.running.load(Ordering::SeqCst) {
        let store = inner.store.clone();
        blocking(move || store.release(&item)).await;
        return;
    }
    inner.requests_sent.fetch_add(1, Ordering::SeqCst);
    let result = inner.describer.describe(&preview).await;
    let store = inner.store.clone();
    match result {
        Ok(d) => {
            lock(&inner.finished_at).push_back(Instant::now());
            blocking(move || store.complete(&item, &d)).await;
        }
        Err(f) => {
            log::warn!("AI insight failed for {}: {}", item.content, f.error);
            let msg = f.error.to_string();
            blocking(move || store.fail(&item, f.requests, &msg)).await;
        }
    }
}

/// A stored insight.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredResult {
    pub content: ContentId,
    pub insight: ImageInsight,
    pub provenance: Provenance,
    pub at: SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemState {
    Pending,
    InFlight,
    Done,
    Failed,
    Deferred,
}

#[derive(Clone, Debug)]
struct Entry {
    priority: i64,
    seq: u64,
    state: ItemState,
    attempts: u32,
    last_error: Option<String>,
}

#[derive(Default, Debug)]
struct Mem {
    items: HashMap<ContentId, Entry>,
    results: Vec<StoredResult>,
    seq: u64,
}

/// An in-memory [`QueueStore`] (tests, one-off headless runs).
#[derive(Default, Debug)]
pub struct MemoryStore {
    m: Mutex<Mem>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a content identity; `false` when it is queued already (one entry per content).
    pub fn enqueue(&self, content: ContentId, priority: i64) -> bool {
        let mut m = lock(&self.m);
        if m.items.contains_key(&content) {
            return false;
        }
        m.seq += 1;
        let seq = m.seq;
        m.items.insert(
            content,
            Entry {
                priority,
                seq,
                state: ItemState::Pending,
                attempts: 0,
                last_error: None,
            },
        );
        true
    }

    pub fn results(&self) -> Vec<StoredResult> {
        lock(&self.m).results.clone()
    }

    pub fn result_for(&self, content: &ContentId) -> Option<StoredResult> {
        lock(&self.m)
            .results
            .iter()
            .rev()
            .find(|r| r.content == *content)
            .cloned()
    }

    /// Attempts and last error of an item.
    pub fn item(&self, content: &ContentId) -> Option<(u32, Option<String>)> {
        lock(&self.m)
            .items
            .get(content)
            .map(|e| (e.attempts, e.last_error.clone()))
    }

    pub fn is_failed(&self, content: &ContentId) -> bool {
        lock(&self.m)
            .items
            .get(content)
            .is_some_and(|e| e.state == ItemState::Failed)
    }

    fn set(&self, content: &ContentId, f: impl FnOnce(&mut Entry)) {
        if let Some(e) = lock(&self.m).items.get_mut(content) {
            f(e);
        }
    }
}

impl QueueStore for MemoryStore {
    fn recover(&self) {
        for e in lock(&self.m).items.values_mut() {
            if matches!(e.state, ItemState::InFlight | ItemState::Deferred) {
                e.state = ItemState::Pending;
            }
        }
    }

    fn claim_next(&self) -> Option<QueueItem> {
        let mut m = lock(&self.m);
        let (content, e) = m
            .items
            .iter_mut()
            .filter(|(_, e)| e.state == ItemState::Pending)
            .min_by_key(|(_, e)| (e.priority, e.seq))?;
        e.state = ItemState::InFlight;
        Some(QueueItem {
            content: *content,
            priority: e.priority,
            attempts: e.attempts,
        })
    }

    fn release(&self, item: &QueueItem) {
        self.set(&item.content, |e| e.state = ItemState::Pending);
    }

    fn has_result(&self, content: &ContentId, key: &ResultKey) -> bool {
        lock(&self.m).results.iter().any(|r| {
            r.content == *content
                && r.provenance.provider == key.provider
                && (key.model.is_empty() || r.provenance.model == key.model)
                && r.provenance.schema_version == key.schema_version
                && r.provenance.prompt_version >= key.prompt_version
        })
    }

    fn complete(&self, item: &QueueItem, d: &Described) {
        let mut m = lock(&self.m);
        if let Some(e) = m.items.get_mut(&item.content) {
            e.state = ItemState::Done;
            e.attempts += d.provenance.requests;
            e.last_error = None;
        }
        m.results.push(StoredResult {
            content: item.content,
            insight: d.insight.clone(),
            provenance: d.provenance.clone(),
            at: SystemTime::now(),
        });
    }

    fn mark_done(&self, item: &QueueItem) {
        self.set(&item.content, |e| e.state = ItemState::Done);
    }

    fn fail(&self, item: &QueueItem, requests: u32, error: &str) {
        self.set(&item.content, |e| {
            e.state = ItemState::Failed;
            e.attempts += requests;
            e.last_error = Some(error.to_owned());
        });
    }

    fn defer(&self, item: &QueueItem, reason: &str) {
        self.set(&item.content, |e| {
            e.state = ItemState::Deferred;
            e.last_error = Some(reason.to_owned());
        });
    }

    fn counts(&self) -> QueueCounts {
        let mut c = QueueCounts::default();
        for e in lock(&self.m).items.values() {
            match e.state {
                ItemState::Pending => c.pending += 1,
                ItemState::InFlight => c.in_flight += 1,
                ItemState::Done => c.done += 1,
                ItemState::Failed => c.failed += 1,
                ItemState::Deferred => c.deferred += 1,
            }
        }
        c
    }
}
