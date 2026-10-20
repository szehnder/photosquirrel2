//! The AI queue engine against a recording provider (in-process HTTP server, OpenAI-compatible wire).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ps_llm::queue::{AiQueue, MemoryStore, QueueConfig, QueueState, StartError};
use ps_llm::testing::{MockServer, Recorded, Reply, insight_json, openai_reply, preview};
use ps_llm::{
    Describer, LlmRuntime, PRESETS, PreviewError, PreviewJpeg, ProviderConfig, RetryPolicy,
    provider_for,
};
use ps_model::ContentId;
use ps_model::settings::{Ai, ProviderId};
use serde_json::json;

fn content(n: u8) -> ContentId {
    let mut d = [0u8; 32];
    d[0] = n;
    d[31] = 0xAB;
    ContentId::full(d)
}

/// The seed of the (single) image in a request.
fn seed_of(r: &Recorded) -> Option<u8> {
    let img = r.images().pop()?;
    (0..=255u8).find(|s| preview(u64::from(*s)).bytes() == img.as_slice())
}

fn loader() -> Arc<dyn ps_llm::queue::PreviewLoader> {
    Arc::new(|c: &ContentId| -> Result<PreviewJpeg, PreviewError> {
        Ok(preview(u64::from(c.digest[0])))
    })
}

fn cfg_for(server: &MockServer) -> ProviderConfig {
    let mut cfg = ProviderConfig::from_preset(&PRESETS[0]);
    cfg.base_url = format!("{}/v1", server.url());
    cfg.model = "recording-vlm".into();
    cfg.retry = RetryPolicy {
        max_retries: 2,
        rate_limit_base: Duration::from_millis(10),
        server_backoff: vec![Duration::from_millis(10), Duration::from_millis(20)],
        max_retry_after: Duration::from_secs(1),
    };
    cfg
}

fn ok_reply() -> Reply {
    Reply::json(
        200,
        &openai_reply(&insight_json("people", 2, 1, &[]).to_string()),
    )
}

fn queue(store: &Arc<MemoryStore>, server: &MockServer, concurrency: usize) -> AiQueue {
    let cfg = cfg_for(server);
    let mut qc = QueueConfig::local(&cfg.name);
    qc.concurrency = concurrency;
    AiQueue::new(
        store.clone(),
        loader(),
        Describer::new(provider_for(cfg)),
        qc,
        LlmRuntime::shared().handle(),
    )
}

fn store_with(n: u8) -> Arc<MemoryStore> {
    let s = Arc::new(MemoryStore::new());
    for i in 0..n {
        s.enqueue(content(i), i64::from(i));
    }
    s
}

#[test]
fn scenario_ai_queue_nothing_sent_before_start() {
    let server = MockServer::with_handler(|_| ok_reply());
    let store = store_with(5);
    let q = queue(&store, &server, 2);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(server.request_count(), 0, "idle queue sends nothing");
    assert_eq!(q.state(), QueueState::Paused);
    assert_eq!(q.progress().counts.pending, 5);
    // Started explicitly, it runs.
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(10)));
    assert_eq!(server.request_count(), 5);
    assert_eq!(q.state(), QueueState::Finished);
}

#[test]
fn scenario_ai_queue_pause_stops_new_requests() {
    let server = MockServer::with_handler(|_| ok_reply().with_delay(Duration::from_millis(300)));
    let store = store_with(10);
    let q = queue(&store, &server, 2);
    q.start().unwrap();
    let t0 = Instant::now();
    while server.request_count() < 2 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    q.pause();
    let paused_at = Instant::now();
    assert!(
        q.wait_idle(Duration::from_secs(5)),
        "in-flight requests finish"
    );
    let after_drain = server.request_count();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        server.request_count(),
        after_drain,
        "no new request after pause"
    );
    assert!(
        server.requests().iter().all(|r| r.at <= paused_at),
        "nothing started after pause"
    );
    let p = q.progress();
    assert_eq!(p.state, QueueState::Paused);
    assert_eq!(
        p.counts.done as usize, after_drain,
        "in-flight results are stored"
    );
    assert_eq!(p.counts.in_flight, 0);
    assert_eq!(p.counts.pending as usize, 10 - after_drain);
    // Resume finishes the rest, each item once.
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(10)));
    assert_eq!(server.request_count(), 10);
    assert_eq!(store.results().len(), 10);
}

#[test]
fn queue_runs_in_priority_order() {
    let server = MockServer::with_handler(|_| ok_reply());
    let store = Arc::new(MemoryStore::new());
    // Enqueued in reverse priority.
    for (i, prio) in [(1u8, 30), (2, 10), (3, 20)] {
        store.enqueue(content(i), prio);
    }
    let q = queue(&store, &server, 1);
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(10)));
    let order: Vec<u8> = server.requests().iter().filter_map(seed_of).collect();
    assert_eq!(order, vec![2, 3, 1]);
}

#[test]
fn scenario_ai_queue_concurrency_limit_respected() {
    let server = MockServer::with_handler(|_| ok_reply().with_delay(Duration::from_millis(150)));
    let store = store_with(8);
    let q = queue(&store, &server, 2);
    q.start().unwrap();
    let mut max_seen = 0;
    let t0 = Instant::now();
    while q.state() == QueueState::Running && t0.elapsed() < Duration::from_secs(10) {
        max_seen = max_seen.max(q.in_flight());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(q.wait_idle(Duration::from_secs(10)));
    assert_eq!(server.request_count(), 8);
    assert!(
        server.max_in_flight() <= 2,
        "server saw {}",
        server.max_in_flight()
    );
    assert_eq!(server.max_in_flight(), 2, "the limit is used");
    assert!(max_seen <= 2);
}

#[test]
fn scenario_ai_queue_duplicates_analyzed_once() {
    let server = MockServer::with_handler(|_| ok_reply());
    let store = Arc::new(MemoryStore::new());
    // Three byte-identical copies (different files, one content identity), "include non-keepers" on.
    let files: HashMap<&str, ContentId> = [
        ("a/IMG_1.jpg", content(9)),
        ("b/IMG_1.jpg", content(9)),
        ("c/copy.jpg", content(9)),
    ]
    .into_iter()
    .collect();
    let queued: Vec<bool> = files.values().map(|c| store.enqueue(*c, 0)).collect();
    assert_eq!(
        queued.iter().filter(|q| **q).count(),
        1,
        "one queue entry per content"
    );
    let q = queue(&store, &server, 2);
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(10)));
    assert_eq!(server.request_count(), 1, "exactly one request");
    let insights: Vec<_> = files
        .values()
        .map(|c| store.result_for(c).unwrap().insight)
        .collect();
    assert!(
        insights.windows(2).all(|w| w[0] == w[1]),
        "all copies show the same insight"
    );

    // Already described by the same provider, model and schema: not sent again.
    let again = store.enqueue(content(9), 0);
    assert!(!again);
    let store2 = Arc::new(MemoryStore::new());
    store2.enqueue(content(9), 0);
    let q2 = queue(&store2, &server, 1);
    q2.start().unwrap();
    assert!(q2.wait_idle(Duration::from_secs(10)));
    assert_eq!(
        server.request_count(),
        2,
        "a fresh store has no result, so it is sent"
    );
    let q3 = queue(&store2, &server, 1);
    store2.enqueue(content(10), 0);
    // content(9) is done; re-running only sends content(10).
    q3.start().unwrap();
    assert!(q3.wait_idle(Duration::from_secs(10)));
    assert_eq!(server.request_count(), 3);
}

#[test]
fn scenario_ai_queue_result_records_provenance() {
    let server = MockServer::with_handler(|_| ok_reply().with_delay(Duration::from_millis(60)));
    let store = store_with(1);
    let q = queue(&store, &server, 1);
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(10)));
    let r = store.result_for(&content(0)).unwrap();
    assert_eq!(r.provenance.provider, "LM Studio (local)");
    assert_eq!(
        r.provenance.model, "test-vlm",
        "the model the server reports"
    );
    assert_eq!(r.provenance.schema_version, 1);
    assert!(
        r.provenance.latency >= Duration::from_millis(60),
        "{:?}",
        r.provenance.latency
    );
    assert_eq!(r.provenance.requests, 1);
}

#[test]
fn scenario_ai_queue_failed_item_retried_twice() {
    // Item 3 always gets a server error.
    let server = MockServer::with_handler(|r| {
        if seed_of(r) == Some(3) {
            Reply::json(500, &json!({"error": {"message": "model crashed"}}))
        } else {
            ok_reply()
        }
    });
    let store = store_with(6);
    let q = queue(&store, &server, 2);
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(20)));
    let per_item = server
        .requests()
        .iter()
        .filter_map(seed_of)
        .fold(HashMap::new(), |mut m, s| {
            *m.entry(s).or_insert(0) += 1;
            m
        });
    assert_eq!(per_item[&3], 3, "attempted 3 times");
    assert!(store.is_failed(&content(3)));
    let (_, err) = store.item(&content(3)).unwrap();
    assert!(err.unwrap().contains("500"));
    let p = q.progress();
    assert_eq!(
        (p.counts.done, p.counts.failed, p.counts.pending),
        (5, 1, 0)
    );
    for i in [0, 1, 2, 4, 5] {
        assert_eq!(per_item[&i], 1);
    }
}

#[test]
fn scenario_llm_providers_hosted_provider_needs_consent() {
    let server = MockServer::with_handler(|_| ok_reply());
    let ai = Ai {
        provider: ProviderId::Gemini,
        base_url: ProviderId::Gemini.default_base_url().into(),
        consent: None,
        ..Ai::default()
    };
    let mut cfg = ProviderConfig::from_settings(&ai, Some(ps_llm::ApiKey::new("test-gemini-key")));
    // Point at the recording server: the provider is still hosted, by id.
    cfg.base_url = format!("{}/v1beta", server.url());
    assert!(cfg.is_hosted());
    let qc = QueueConfig::from_settings(&ai, &cfg);
    let store = store_with(3);
    let q = AiQueue::new(
        store.clone(),
        loader(),
        Describer::new(provider_for(cfg.clone())),
        qc,
        LlmRuntime::shared().handle(),
    );
    let StartError::ConsentRequired(prompt) = q.start().unwrap_err();
    assert_eq!(prompt.provider, "Google Gemini");
    assert!(prompt.message.contains("Google Gemini") && prompt.message.contains("upload"));
    assert_eq!(q.state(), QueueState::Paused);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.request_count(), 0, "no request sent");
    let gemini = ProviderId::Gemini;
    assert!(ps_llm::consent::check(&cfg, gemini, None).is_err());
    let given = ps_llm::consent::consent_for(gemini, &cfg.base_url);
    assert!(ps_llm::consent::check(&cfg, gemini, given.as_ref()).is_ok());
    assert!(
        ps_llm::consent::check(&ProviderConfig::default_local(), ProviderId::LmStudio, None)
            .is_ok()
    );
}

/// ADR-0047: consent names one provider at one host. Consent given for Gemini at its API host does not cover
/// another provider, nor the same provider at another server address.
#[test]
fn scenario_llm_providers_consent_covers_one_host() {
    use ps_llm::consent::{check, consent_for, destination_host};
    let ai = Ai {
        provider: ProviderId::Gemini,
        base_url: ProviderId::Gemini.default_base_url().into(),
        ..Ai::default()
    };
    let cfg = ProviderConfig::from_settings(&ai, None);
    let given = consent_for(ProviderId::Gemini, &ai.base_url).expect("consent");
    assert_eq!(given.host, "generativelanguage.googleapis.com");
    assert!(check(&cfg, ProviderId::Gemini, Some(&given)).is_ok());
    assert!(check(&cfg, ProviderId::OpenAi, Some(&given)).is_err());
    let moved = Ai {
        base_url: "https://gemini-proxy.example.com/v1beta".into(),
        ..ai.clone()
    };
    let cfg = ProviderConfig::from_settings(&moved, None);
    let err = check(&cfg, ProviderId::Gemini, Some(&given)).expect_err("another host");
    assert_eq!(err.host, "gemini-proxy.example.com");
    assert!(
        err.message.contains("gemini-proxy.example.com"),
        "{}",
        err.message
    );
    assert_eq!(
        destination_host("https://gemini-proxy.example.com/v1beta").as_deref(),
        Some("gemini-proxy.example.com")
    );
}

/// A local server answering with a redirect cannot send a request (and its preview) to another host: the
/// client never follows redirects.
#[test]
fn scenario_llm_providers_redirects_are_never_followed() {
    let elsewhere = MockServer::with_handler(|_| ok_reply());
    let target = format!("{}/v1/chat/completions", elsewhere.url());
    let local = MockServer::with_handler(move |_| {
        ps_llm::testing::Reply::json(307, &serde_json::json!({})).with_header("location", &target)
    });
    let ai = Ai {
        base_url: format!("{}/v1", local.url()),
        model: "m".into(),
        ..Ai::default()
    };
    let cfg = ProviderConfig::from_settings(&ai, None);
    let client = ps_llm::http::HttpClient::new(&cfg).without_retries();
    let r = LlmRuntime::shared().block_on(client.post_json(
        &cfg.url("/chat/completions"),
        &[],
        &serde_json::json!({"model": "m"}),
    ));
    assert!(r.is_err(), "a 307 is an error, not followed: {r:?}");
    assert_eq!(local.request_count(), 1);
    assert_eq!(
        elsewhere.request_count(),
        0,
        "nothing reached the other host"
    );
}

#[test]
fn offline_previews_are_deferred_and_progress_counts() {
    let server = MockServer::with_handler(|_| ok_reply());
    let store = store_with(4);
    let cfg = cfg_for(&server);
    let offline: Arc<dyn ps_llm::queue::PreviewLoader> = Arc::new(|c: &ContentId| {
        if c.digest[0] == 1 {
            Err(PreviewError::Deferred("source offline".into()))
        } else {
            Ok(preview(u64::from(c.digest[0])))
        }
    });
    let q = AiQueue::new(
        store.clone(),
        offline,
        Describer::new(provider_for(cfg.clone())),
        QueueConfig::local(&cfg.name),
        LlmRuntime::shared().handle(),
    );
    q.start().unwrap();
    assert!(q.wait_idle(Duration::from_secs(10)));
    let p = q.progress();
    assert_eq!(
        (p.counts.done, p.counts.deferred, p.counts.failed),
        (3, 1, 0)
    );
    assert_eq!(p.requests_sent, 3);
    assert!(p.per_minute >= 3.0);
}
