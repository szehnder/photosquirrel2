//! AI insights facade (change `ai-insights`): provider configuration from settings plus keychain keys
//! (ADR-0020, ADR-0031) and the hosted-provider consent gate (ADR-0021).
//!
//! The queue engine itself is `ps_llm::queue` (it needs the `ps-llm` tokio runtime, ADR-0009); its
//! catalog-backed store over migration 5 (`ai_queue`, `ai_result`, `ai_correction`, `ai_suggestion`) is added
//! here once catalog migration 4 has landed and migration 5 can be registered.

use ps_model::settings::{Ai, ProviderId, Settings};

use crate::secrets::{ApiKey as StoredKey, Secrets, StoredIn, default_account};

pub use ps_llm::consent::ConsentRequired;
pub use ps_llm::queue::QueueState;
pub use ps_llm::{ProviderConfig, TestReport};

/// The keychain account for `provider`: the one recorded in settings, else the default.
pub fn key_account(ai: &Ai, provider: ProviderId) -> String {
    ai.key_accounts
        .iter()
        .find(|k| k.provider == provider)
        .map(|k| k.account.clone())
        .unwrap_or_else(|| default_account(provider))
}

/// The active provider's key (environment variable first, then keychain, then session) and where it came from.
/// LM Studio takes no key; other OpenAI-compatible endpoints (OpenRouter, Groq, a keyed local server) may.
pub fn api_key(ai: &Ai, secrets: &Secrets) -> Option<StoredKey> {
    if ai.provider == ProviderId::LmStudio {
        return None;
    }
    secrets.api_key(ai.provider, &key_account(ai, ai.provider))
}

/// The provider configuration the settings describe, with its key.
pub fn provider_config(
    settings: &Settings,
    secrets: &Secrets,
) -> (ProviderConfig, Option<StoredIn>) {
    let key = api_key(&settings.ai, secrets);
    let from = key.as_ref().map(|k| k.from);
    let cfg =
        ProviderConfig::from_settings(&settings.ai, key.map(|k| ps_llm::ApiKey::new(k.value)));
    (cfg, from)
}

/// `Ok` when previews may be sent: a local provider, or a hosted one the user consented to for this provider
/// and destination host (ADR-0047).
pub fn check_consent(settings: &Settings, cfg: &ProviderConfig) -> Result<(), ConsentRequired> {
    ps_llm::consent::check(cfg, settings.ai.provider, settings.ai.consent.as_ref())
}

/// Previews would leave this machine with these settings (a hosted provider, or a non-local address).
pub fn is_hosted(ai: &Ai) -> bool {
    ProviderConfig::from_settings(ai, None).is_hosted()
}

/// Where previews would go (`host[:port]`), for the consent card.
pub fn destination_host(ai: &Ai) -> Option<String> {
    ps_llm::consent::destination_host(&ProviderConfig::from_settings(ai, None).base_url)
}

/// The stored consent covers the configured provider and host.
pub fn consent_covers(ai: &Ai) -> bool {
    ps_llm::consent::covers(
        &ProviderConfig::from_settings(ai, None),
        ai.provider,
        ai.consent.as_ref(),
    )
}

/// The consent to store when the user agrees for the configured provider and host.
pub fn consent_for(ai: &Ai) -> Option<ps_model::settings::HostedConsent> {
    ps_llm::consent::consent_for(
        ai.provider,
        &ProviderConfig::from_settings(ai, None).base_url,
    )
}

/// What the AI queue sends to: provider, normalized destination and whether consent covers it. The app pauses
/// a running queue whenever this changes (ADR-0047).
pub fn destination_fingerprint(ai: &Ai) -> (ProviderId, Option<String>, bool) {
    (ai.provider, destination_host(ai), consent_covers(ai))
}

/// A [`ps_llm::queue::SendGate`] for a queue started from `at_start` (ADR-0049): before each request it
/// re-reads `store`'s file and allows the request only while the provider, the destination and the consent
/// coverage are what they were at start and the consent still covers a hosted destination. Withdrawing consent
/// or changing the address in `settings.toml` therefore pauses a CLI `ai start` before its next request.
pub fn settings_file_gate(
    store: crate::settings::SettingsStore,
    at_start: &Settings,
) -> ps_llm::queue::SendGate {
    let expected = destination_fingerprint(&at_start.ai);
    Arc::new(move || {
        let now = store.load().settings;
        let ok = destination_fingerprint(&now.ai) == expected
            && (!is_hosted(&now.ai) || consent_covers(&now.ai));
        if !ok {
            log::warn!(
                "{}: the AI provider, address or consent changed",
                store.path().display()
            );
        }
        ok
    })
}

/// Run "Test connection" for the configured provider, blocking the calling (non-UI) thread.
pub fn test_connection(settings: &Settings, secrets: &Secrets) -> TestReport {
    let (cfg, _) = provider_config(settings, secrets);
    let name = cfg.name.clone();
    match ps_llm::provider_for(cfg) {
        Ok(provider) => ps_llm::LlmRuntime::shared().block_on(ps_llm::test_connection(&*provider)),
        Err(e) => TestReport {
            provider: name,
            reachable: false,
            latency: None,
            error: Some(e.to_string()),
            model: None,
            model_found: None,
            accepts_images: None,
            models: Vec::new(),
        },
    }
}

// ---- The catalog-backed queue (ai-insights tasks 2.1, 2.3, 2.5, 2.8) --------------------------------------

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ps_catalog::{AiItemState, AiResultRow, AiRunState, Catalog};
use ps_llm::queue::{
    AiQueue, PreviewLoader, QueueConfig, QueueCounts, QueueItem, QueueProgress, QueueStore,
    ResultKey, StartError,
};
use ps_llm::{Described, Describer, PreviewError, PreviewInput, PreviewJpeg};
use ps_model::{ContentId, FileId, Format, ImageInsight, MediaKind, PhotoClass, path_from_bytes};
use serde::{Deserialize, Serialize};

use crate::scan::ScanService;

/// [`QueueStore`] over the catalog's `ai_queue`/`ai_result`/`ai_run` tables (migration 5). Every write is
/// committed before it returns, so the queue survives a crash at any point; on restart in-flight items become
/// pending and the queue is paused.
#[derive(Debug, Clone)]
pub struct CatalogQueueStore {
    catalog: Arc<Catalog>,
}

impl CatalogQueueStore {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Self { catalog }
    }
}

fn logged<T: Default>(what: &str, r: Result<T, ps_catalog::CatalogError>) -> T {
    r.unwrap_or_else(|e| {
        log::error!("AI queue store: {what}: {e}");
        T::default()
    })
}

impl QueueStore for CatalogQueueStore {
    fn recover(&self) {
        logged("recover", self.catalog.writer().ai_recover());
    }

    fn claim_next(&self) -> Option<QueueItem> {
        logged("claim", self.catalog.writer().ai_claim_next()).map(|r| QueueItem {
            content: r.content,
            priority: r.priority,
            attempts: r.attempts,
        })
    }

    fn release(&self, item: &QueueItem) {
        logged(
            "release",
            self.catalog
                .writer()
                .ai_set_state(item.content, AiItemState::Pending, 0, None),
        );
    }

    fn has_result(&self, content: &ContentId, key: &ResultKey) -> bool {
        logged(
            "has_result",
            self.catalog.read().ai_has_result(
                content,
                &key.provider,
                &key.model,
                key.schema_version,
                key.prompt_version,
            ),
        )
    }

    /// Store the result for the item's whole capture set (a RAW+JPEG pair is described once, through its
    /// JPEG, and the insight is stored for both contents so every view shows it on both files).
    fn complete(&self, item: &QueueItem, d: &Described) {
        let insight = serde_json::to_string(&d.insight).unwrap_or_default();
        let at_ms = ps_export::now_ms();
        let set = logged("capture set", capture_set(&self.catalog, &item.content));
        let contents = if set.is_empty() {
            vec![item.content]
        } else {
            set
        };
        for content in contents {
            logged(
                "complete",
                self.catalog.writer().ai_complete(AiResultRow {
                    content,
                    provider: d.provenance.provider.clone(),
                    model: d.provenance.model.clone(),
                    schema_version: d.provenance.schema_version,
                    prompt_version: d.provenance.prompt_version,
                    insight: insight.clone(),
                    latency_ms: d.provenance.latency.as_millis() as u64,
                    requests: d.provenance.requests,
                    at_ms,
                }),
            );
        }
    }

    fn mark_done(&self, item: &QueueItem) {
        logged(
            "mark_done",
            self.catalog
                .writer()
                .ai_set_state(item.content, AiItemState::Done, 0, None),
        );
    }

    fn fail(&self, item: &QueueItem, requests: u32, error: &str) {
        logged(
            "fail",
            self.catalog.writer().ai_set_state(
                item.content,
                AiItemState::Failed,
                requests,
                Some(error.to_owned()),
            ),
        );
    }

    fn defer(&self, item: &QueueItem, reason: &str) {
        logged(
            "defer",
            self.catalog.writer().ai_set_state(
                item.content,
                AiItemState::Deferred,
                0,
                Some(reason.to_owned()),
            ),
        );
    }

    fn counts(&self) -> QueueCounts {
        let c = logged("counts", self.catalog.read().ai_counts());
        QueueCounts {
            pending: c.pending,
            in_flight: c.in_flight,
            done: c.done,
            failed: c.failed,
            deferred: c.deferred,
        }
    }
}

/// Produces LLM previews from a source copy of the content (read-only; ADR-0004), preferring copies on online
/// sources. No copy reachable: the item is deferred, not failed.
#[derive(Debug, Clone)]
pub struct SourcePreviewLoader {
    catalog: Arc<Catalog>,
}

impl SourcePreviewLoader {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Self { catalog }
    }
}

impl PreviewLoader for SourcePreviewLoader {
    fn load(&self, content: &ContentId) -> Result<PreviewJpeg, PreviewError> {
        let r = self.catalog.read();
        let ids = r
            .files_with_content(content)
            .map_err(|e| PreviewError::Failed(e.to_string()))?;
        let roots: HashMap<_, _> = r
            .sources()
            .map_err(|e| PreviewError::Failed(e.to_string()))?
            .into_iter()
            .map(|s| (s.id, path_from_bytes(&s.root)))
            .collect();
        let mut last = None;
        for id in ids {
            let Ok(Some(rec)) = r.file_by_id(id) else {
                continue;
            };
            if rec.missing_since_ms.is_some() {
                continue;
            }
            let Some(root) = roots.get(&rec.key.source) else {
                continue;
            };
            let path = rec.key.path.under(root);
            // Read-only: `fs::read` opens the file for reading only.
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let probe = ps_media::probe_buffer(&bytes, Some(&path));
                    return ps_llm::preview::derive(PreviewInput::Source {
                        bytes: &bytes,
                        probe: &probe,
                    })
                    .map(|d| d.llm);
                }
                Err(e) => last = Some(e.to_string()),
            }
        }
        Err(PreviewError::Deferred(last.unwrap_or_else(|| {
            "no reachable copy (source offline)".into()
        })))
    }
}

/// Queue priorities (spec "Persisted queue order"): keepers of class photo or likely photo first, then other
/// keepers, each by capture date (undated last); non-keepers only when opted in, after every keeper.
pub fn queue_priority(keeper: bool, class: Option<PhotoClass>, capture_ms: Option<i64>) -> i64 {
    const TIER: i64 = 100_000_000_000_000;
    let tier = match (keeper, class) {
        (true, Some(PhotoClass::Photo | PhotoClass::LikelyPhoto)) => 0,
        (true, _) => 1,
        (false, _) => 2,
    };
    let t = capture_ms.map_or(TIER - 1, |t| (t + TIER / 2).clamp(0, TIER - 2));
    tier * TIER + t
}

/// Order of preference for the one member of a capture set the AI describes: a JPEG first (the cheapest to
/// decode into a preview), then by best-copy rank.
fn describe_order(format: Option<Format>, rank: usize) -> (bool, usize) {
    (format != Some(Format::Jpeg), rank)
}

/// The distinct contents of `content`'s **capture set**, the one it is described with: the keeper set of its
/// duplicate group when that holds more than one content (a RAW+JPEG pair, ADR-0037). The representative the
/// queue describes comes first ([`describe_order`]). A content outside such a set is alone: `[content]`.
pub fn capture_set(
    catalog: &Catalog,
    content: &ContentId,
) -> Result<Vec<ContentId>, ps_catalog::CatalogError> {
    let r = catalog.read();
    for f in r.files_with_content(content)? {
        let Some(g) = r.group_of_file(f)? else {
            continue;
        };
        let Some(row) = r.group(g)? else { continue };
        if !row.keeper_set.contains(&f) {
            continue;
        }
        let mut set: Vec<((bool, usize), ContentId)> = Vec::new();
        for (rank, m) in row
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| m.in_keeper_set)
        {
            if let Some(rec) = r.file_by_id(m.file)?
                && let Some(c) = rec.identity
                && !set.iter().any(|(_, x)| *x == c)
            {
                set.push((describe_order(rec.format, rank), c));
            }
        }
        if set.len() > 1 {
            set.sort_by_key(|(o, _)| *o);
            return Ok(set.into_iter().map(|(_, c)| c).collect());
        }
    }
    Ok(vec![*content])
}

/// Queue every analyzed image keeper (singletons and group keeper sets), and the other group members when
/// `include_non_keepers`. Byte-identical copies share one entry, and a keeper set of several contents (a
/// RAW+JPEG pair) is queued once, through its JPEG: the other contents get the same result when it completes
/// ([`CatalogQueueStore`]), and entries queued for them earlier are marked done. Returns the number of new
/// entries.
pub fn enqueue(
    catalog: &Catalog,
    include_non_keepers: bool,
) -> Result<u64, ps_catalog::CatalogError> {
    let r = catalog.read();
    let mut keepers: HashSet<FileId> = HashSet::new();
    let mut members: HashSet<FileId> = HashSet::new();
    // Keeper sets with more than one file, and each such file's set and rank in it.
    let mut sets: Vec<Vec<FileId>> = Vec::new();
    let mut in_set: HashMap<FileId, (usize, usize)> = HashMap::new();
    for (_, m, set) in r.group_memberships()? {
        members.extend(m);
        keepers.extend(set.iter().copied());
        if set.len() > 1 {
            for (rank, f) in set.iter().enumerate() {
                in_set.insert(*f, (sets.len(), rank));
            }
            sets.push(set);
        }
    }
    let mut best: HashMap<ContentId, i64> = HashMap::new();
    // Set members' content and describe order, by set.
    let mut set_contents: Vec<Vec<((bool, usize), ContentId)>> = vec![Vec::new(); sets.len()];
    r.for_each_file(|f| {
        let (Some(content), Some(class)) = (f.identity, f.class) else {
            return;
        };
        if f.missing_since_ms.is_some()
            || f.media_kind() != Some(MediaKind::Image)
            || !matches!(
                class,
                PhotoClass::Photo
                    | PhotoClass::LikelyPhoto
                    | PhotoClass::ThumbnailCache
                    | PhotoClass::Graphic
            )
        {
            return;
        }
        let keeper = keepers.contains(&f.id) || !members.contains(&f.id);
        if !keeper && !include_non_keepers {
            return;
        }
        if let Some((set, rank)) = in_set.get(&f.id) {
            set_contents[*set].push((describe_order(f.format, *rank), content));
        }
        let p = queue_priority(keeper, Some(class), f.capture_ms);
        best.entry(content)
            .and_modify(|e| *e = (*e).min(p))
            .or_insert(p);
    })?;
    let mut covered: Vec<ContentId> = Vec::new();
    for mut set in set_contents {
        set.sort_by_key(|(o, _)| *o);
        set.dedup_by_key(|(_, c)| *c);
        let Some(((_, _), rep)) = set.first().copied() else {
            continue;
        };
        for (_, other) in set.into_iter().skip(1) {
            if other == rep {
                continue;
            }
            if let Some(p) = best.remove(&other)
                && let Some(rp) = best.get_mut(&rep)
            {
                *rp = (*rp).min(p);
            }
            covered.push(other);
        }
    }
    let mut items: Vec<(ContentId, i64)> = best.into_iter().collect();
    items.sort_by_key(|(c, p)| (*p, *c));
    let added = catalog.writer().ai_enqueue(items)?;
    // Entries queued for a pair's other content before pairs were described once: their result comes with
    // the representative's.
    for c in covered {
        let pending = r.with(|db| {
            Ok(db
                .query_row(
                    "SELECT state FROM ai_queue WHERE content_id = ?1",
                    [&c.digest[..]],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
                == Some(0))
        })?;
        if pending {
            catalog
                .writer()
                .ai_set_state(c, AiItemState::Done, 0, None)?;
        }
    }
    Ok(added)
}

/// Contents whose newest result came from an older prompt than [`ps_llm::PROMPT_VERSION`]. They are kept and
/// shown as such; a prompt change never queues them again by itself (ai-queue "Prompt upgrades are explicit").
pub fn older_prompt_contents(catalog: &Catalog) -> Result<u64, ps_catalog::CatalogError> {
    Ok(catalog
        .read()
        .ai_older_prompt_contents(ps_llm::PROMPT_VERSION)?
        .len() as u64)
}

/// The user asked to re-describe the results of older prompts: queue those contents again (pending, attempts
/// reset). Nothing is sent until the queue starts; the queue then describes each once with the current
/// prompt, and the new result replaces the old one for the same provider and model. Returns the number queued.
pub fn redescribe_older_prompts(catalog: &Catalog) -> Result<u64, ps_catalog::CatalogError> {
    let digests = catalog
        .read()
        .ai_older_prompt_contents(ps_llm::PROMPT_VERSION)?;
    if digests.is_empty() {
        return Ok(0);
    }
    catalog
        .writer()
        .ai_requeue(digests, queue_priority(true, None, None))
}

/// Queue counts and run state straight from the catalog (no provider, nothing sent).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiStatus {
    pub state: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub pending: u64,
    pub in_flight: u64,
    pub done: u64,
    pub failed: u64,
    pub deferred: u64,
    pub results: u64,
    /// Items finished in the last minute (live queue only).
    pub per_minute: Option<f64>,
    pub eta_s: Option<u64>,
}

/// One stored insight with the files it describes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiResultInfo {
    pub content: String,
    pub files: Vec<FileId>,
    pub paths: Vec<PathBuf>,
    pub provider: String,
    pub model: String,
    pub schema_version: u32,
    pub prompt_version: u32,
    /// From an older prompt than the queue sends now (kept; `ai redescribe` queues it again).
    pub older_prompt: bool,
    pub latency_ms: u64,
    pub insight: ImageInsight,
}

/// Status from the catalog.
/// Record the AI queue as paused when no queue runs in this process (the catalog keeps the state for the
/// next start and for the status line).
pub fn mark_paused(catalog: &Catalog) -> Result<(), ps_catalog::CatalogError> {
    catalog.writer().ai_set_run(AiRunState::Paused, None, None)
}

pub fn status(catalog: &Catalog) -> Result<AiStatus, ps_catalog::CatalogError> {
    let r = catalog.read();
    let c = r.ai_counts()?;
    let run = r.ai_run()?;
    let results = r.with(|c| {
        Ok(c.query_row("SELECT count(*) FROM ai_result", [], |r| r.get::<_, i64>(0))? as u64)
    })?;
    let (state, provider, model) = match run {
        Some((s, p, m)) => (s, p, m),
        None => (AiRunState::Paused, None, None),
    };
    Ok(AiStatus {
        state: match state {
            AiRunState::Paused => "paused",
            AiRunState::Running => "running",
            AiRunState::Finished => "finished",
        }
        .into(),
        provider,
        model,
        pending: c.pending,
        in_flight: c.in_flight,
        done: c.done,
        failed: c.failed,
        deferred: c.deferred,
        results,
        per_minute: None,
        eta_s: None,
    })
}

/// Stored insights, newest first, with the files of each content.
pub fn results(
    catalog: &Catalog,
    limit: u64,
) -> Result<Vec<AiResultInfo>, ps_catalog::CatalogError> {
    let r = catalog.read();
    let roots: HashMap<_, _> = r
        .sources()?
        .into_iter()
        .map(|s| (s.id, path_from_bytes(&s.root)))
        .collect();
    let mut out = Vec::new();
    for row in r.ai_results(limit)? {
        let files = r.files_with_content(&row.content)?;
        let mut paths = Vec::new();
        for f in &files {
            if let Some(rec) = r.file_by_id(*f)? {
                let root = roots.get(&rec.key.source).cloned().unwrap_or_default();
                paths.push(rec.key.path.under(&root));
            }
        }
        let insight: ImageInsight = serde_json::from_str(&row.insight)
            .map_err(|e| ps_catalog::CatalogError::Corrupt(e.to_string()))?;
        out.push(AiResultInfo {
            content: row.content.to_hex(),
            files,
            paths,
            provider: row.provider,
            model: row.model,
            schema_version: row.schema_version,
            prompt_version: row.prompt_version,
            older_prompt: row.prompt_version < ps_llm::PROMPT_VERSION,
            latency_ms: row.latency_ms,
            insight,
        });
    }
    Ok(out)
}

/// The AI queue of one catalog, persisted in it. Nothing is sent before [`AiService::start`] (ADR-0021).
pub struct AiService {
    queue: AiQueue,
    catalog: Arc<Catalog>,
    provider: String,
    model: String,
}

impl std::fmt::Debug for AiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiService")
            .field("provider", &self.provider)
            .field("state", &self.queue.state())
            .finish()
    }
}

impl AiService {
    /// The queue for the provider the settings describe (key from the keychain/environment).
    pub fn from_settings(
        scan: &ScanService,
        settings: &Settings,
        secrets: &Secrets,
    ) -> Result<AiService, ps_llm::LlmError> {
        let (cfg, _) = provider_config(settings, secrets);
        let qc = QueueConfig::from_settings(&settings.ai, &cfg);
        AiService::with_provider(scan.engine().catalog().clone(), cfg, qc)
    }

    /// The queue for an explicit provider configuration (tests, CLI overrides).
    pub fn with_provider(
        catalog: Arc<Catalog>,
        cfg: ProviderConfig,
        qc: QueueConfig,
    ) -> Result<AiService, ps_llm::LlmError> {
        let loader: Arc<dyn PreviewLoader> = Arc::new(SourcePreviewLoader::new(catalog.clone()));
        AiService::with_loader(catalog, cfg, qc, loader)
    }

    /// With a custom preview loader (tests).
    pub fn with_loader(
        catalog: Arc<Catalog>,
        cfg: ProviderConfig,
        qc: QueueConfig,
        loader: Arc<dyn PreviewLoader>,
    ) -> Result<AiService, ps_llm::LlmError> {
        let provider_name = cfg.name.clone();
        let model = cfg.model.clone();
        let store: Arc<dyn QueueStore> = Arc::new(CatalogQueueStore::new(catalog.clone()));
        let queue = AiQueue::new(
            store,
            loader,
            Describer::new(ps_llm::provider_for(cfg)?),
            qc,
            ps_llm::LlmRuntime::shared().handle(),
        );
        Ok(AiService {
            queue,
            catalog,
            provider: provider_name,
            model,
        })
    }

    /// Queue keepers (and non-keepers when opted in) and start sending. A hosted provider without consent
    /// refuses and sends nothing.
    pub fn start(&self, include_non_keepers: bool) -> Result<u64, AiStartError> {
        let added = enqueue(&self.catalog, include_non_keepers)?;
        self.queue.start().map_err(AiStartError::Start)?;
        self.catalog.writer().ai_set_run(
            AiRunState::Running,
            Some(self.provider.clone()),
            Some(self.model.clone()),
        )?;
        Ok(added)
    }

    /// Check `gate` before every request; a `false` pauses the queue ([`settings_file_gate`]).
    pub fn set_send_gate(&self, gate: ps_llm::queue::SendGate) {
        self.queue.set_send_gate(gate);
    }

    /// Stop starting new requests (in-flight ones finish and are stored).
    pub fn pause(&self) -> Result<(), ps_catalog::CatalogError> {
        self.queue.pause();
        self.catalog
            .writer()
            .ai_set_run(AiRunState::Paused, None, None)
    }

    /// Block until nothing is running or in flight; records `finished` when the queue ran dry.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let idle = self.queue.wait_idle(timeout);
        if idle && self.queue.state() == QueueState::Finished {
            logged(
                "finish",
                self.catalog
                    .writer()
                    .ai_set_run(AiRunState::Finished, None, None),
            );
        }
        idle
    }

    pub fn state(&self) -> QueueState {
        self.queue.state()
    }

    pub fn progress(&self) -> QueueProgress {
        self.queue.progress()
    }

    /// Catalog status plus the live rate and ETA.
    pub fn status(&self) -> Result<AiStatus, ps_catalog::CatalogError> {
        let mut s = status(&self.catalog)?;
        let p = self.queue.progress();
        s.state = match p.state {
            QueueState::Paused => "paused",
            QueueState::Running => "running",
            QueueState::Finished => "finished",
        }
        .into();
        s.per_minute = Some(p.per_minute);
        s.eta_s = p.eta.map(|d| d.as_secs());
        Ok(s)
    }
}

/// Why [`AiService::start`] did not start.
#[derive(Debug, thiserror::Error)]
pub enum AiStartError {
    #[error(transparent)]
    Start(StartError),
    #[error(transparent)]
    Catalog(#[from] ps_catalog::CatalogError),
}
