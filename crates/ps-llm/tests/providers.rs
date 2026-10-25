//! Recorded-fixture tests of the three wire formats and the provider layer (spec `llm-providers`).
//! No real network: every request goes to an in-process server on 127.0.0.1.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ps_llm::testing::{
    Exchange, MockServer, Reply, closed_port, insight_json, openai_reply, preview,
};
use ps_llm::{
    ApiKey, Describer, LlmError, PRESETS, ProviderConfig, RetryPolicy, StructuredOutput,
    WireFormat, provider_for, test_connection,
};
use ps_model::SceneCategory;
use serde_json::json;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn preset(id: &str) -> ProviderConfig {
    ProviderConfig::from_preset(PRESETS.iter().find(|p| p.id == id).unwrap())
}

fn lm_studio(server: &MockServer) -> ProviderConfig {
    let mut cfg = preset("lm_studio");
    cfg.base_url = format!("{}/v1", server.url());
    cfg.model = "qwen2.5-vl-7b-instruct".into();
    cfg
}

fn fast_retries() -> RetryPolicy {
    RetryPolicy {
        max_retries: 2,
        rate_limit_base: Duration::from_millis(20),
        server_backoff: vec![Duration::from_millis(20), Duration::from_millis(40)],
        max_retry_after: Duration::from_secs(10),
    }
}

fn describe(cfg: ProviderConfig) -> Result<ps_llm::Described, ps_llm::DescribeFailure> {
    let d = Describer::new(provider_for(cfg).expect("provider"));
    rt().block_on(d.describe(&preview(1)))
}

fn assert_beach_insight(i: &ps_model::ImageInsight) {
    assert_eq!(i.category, SceneCategory::Pets);
    assert_eq!(i.also, vec![SceneCategory::People, SceneCategory::Travel]);
    assert_eq!(
        (i.people.total, i.people.adults, i.people.children),
        (3, 2, 1)
    );
    assert_eq!(i.pets, vec!["dog"]);
    assert!(i.is_photo);
}

#[test]
fn mock_server_replays_fixture_and_fails_on_shape_mismatch() {
    // Replays: the recorded response comes back and the request matches.
    let server = MockServer::from_fixtures(&[fixture("openai/describe.json")]);
    let d = describe(lm_studio(&server)).unwrap();
    assert_beach_insight(&d.insight);
    server.assert_all_matched();

    // Mismatch: a different model name changes the body; the server reports where.
    let server = MockServer::from_fixtures(&[fixture("openai/describe.json")]);
    let mut cfg = lm_studio(&server);
    cfg.model = "some-other-model".into();
    let err = describe(cfg).unwrap_err();
    assert!(
        matches!(err.error, LlmError::Http { status: 422, .. }),
        "{err:?}"
    );
    let mismatches = server.mismatches();
    if std::env::var("PHOTOSQUIRREL_UPDATE_FIXTURES").is_err() {
        assert_eq!(mismatches.len(), 1, "{mismatches:?}");
        assert!(mismatches[0].contains("$.model"), "{mismatches:?}");
    }
}

#[test]
fn scenario_llm_providers_openai_compatible_request_shape() {
    let server = MockServer::from_fixtures(&[fixture("openai/describe.json")]);
    let d = describe(lm_studio(&server)).unwrap();
    server.assert_all_matched();
    let req = &server.requests()[0];
    let body = req.json();
    let parts = body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(
        parts.iter().filter(|p| p["type"] == "image_url").count(),
        1,
        "one image part"
    );
    assert_eq!(req.images().len(), 1);
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_beach_insight(&d.insight);
    assert_eq!(d.provenance.model, "qwen2.5-vl-7b-instruct");
    assert_eq!(d.provenance.provider, "LM Studio (local)");
}

#[test]
fn scenario_llm_providers_default_is_lm_studio() {
    let cfg = ProviderConfig::default_local();
    assert_eq!(cfg.wire, WireFormat::OpenAiCompatible);
    assert_eq!(cfg.base_url, "http://localhost:1234/v1");
    assert!(cfg.api_key.is_none());
    assert!(!cfg.is_hosted());
    assert!(cfg.model.is_empty(), "uses the server's loaded model");
}

#[test]
fn scenario_llm_providers_anthropic_request_shape() {
    let server = MockServer::from_fixtures(&[fixture("anthropic/describe.json")]);
    let mut cfg = preset("anthropic");
    cfg.base_url = format!("{}/v1", server.url());
    cfg.api_key = Some(ApiKey::new("test-anthropic-key"));
    let d = describe(cfg).unwrap();
    server.assert_all_matched();
    let body = server.requests()[0].json();
    let blocks = body["messages"][0]["content"].as_array().unwrap();
    let images: Vec<_> = blocks.iter().filter(|b| b["type"] == "image").collect();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["source"]["type"], "base64");
    assert_eq!(images[0]["source"]["media_type"], "image/jpeg");
    assert_beach_insight(&d.insight);
    assert_eq!(d.provenance.model, "claude-haiku-4-5");
}

#[test]
fn scenario_llm_providers_gemini_request_shape() {
    let server = MockServer::from_fixtures(&[fixture("gemini/describe.json")]);
    let mut cfg = preset("gemini");
    cfg.base_url = format!("{}/v1beta", server.url());
    cfg.api_key = Some(ApiKey::new("test-gemini-key"));
    let d = describe(cfg).unwrap();
    server.assert_all_matched();
    let req = &server.requests()[0];
    assert!(
        !req.path.contains("key="),
        "the key is a header, never in the URL"
    );
    let body = req.json();
    let parts = body["contents"][0]["parts"].as_array().unwrap();
    assert_eq!(
        parts
            .iter()
            .filter(|p| p.get("inline_data").is_some())
            .count(),
        1
    );
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(body["generationConfig"]["responseSchema"]["type"], "OBJECT");
    assert_beach_insight(&d.insight);
}

#[test]
fn scenario_llm_providers_original_never_uploaded() {
    // A 12 MP synthetic corpus photo on disk (never a real photo).
    let dir = std::env::temp_dir().join(format!("claude-1000-ps-llm-orig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("IMG_4000x3000_original.jpg");
    let original = ps_fixtures::media::jpeg(4000, 3000, 7, 92);
    std::fs::write(&path, &original).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    let probe = ps_media::probe_bytes(&bytes, bytes.len() as u64, Some(&path));
    let derived = ps_llm::preview::derive(ps_llm::PreviewInput::Source {
        bytes: &bytes,
        probe: &probe,
        decoders: &ps_media::Decoders::none(),
    })
    .unwrap();
    assert!(
        derived.generated_large.is_some(),
        "the 2048 px preview is generated for the cache"
    );

    let server = MockServer::with_handler(|_| {
        Reply::json(
            200,
            &openai_reply(&insight_json("landscape", 0, 0, &[]).to_string()),
        )
    });
    let d = Describer::new(provider_for(lm_studio(&server)).expect("provider"));
    rt().block_on(d.describe(&derived.llm)).unwrap();

    let req = &server.requests()[0];
    let images = req.images();
    assert_eq!(images.len(), 1);
    let up = &images[0];
    assert_eq!(&up[..2], &[0xFF, 0xD8], "a JPEG");
    let sof = ps_media::jpeg::sof_of(up).unwrap();
    assert!(
        sof.width.max(sof.height) <= 1024,
        "{}x{}",
        sof.width,
        sof.height
    );
    assert_eq!(u32::from(sof.width.max(sof.height)), 1024);
    assert_ne!(up, &original, "not the original bytes");
    assert!(up.len() < original.len());
    let text = req.body_text();
    assert!(!text.contains(path.to_str().unwrap()));
    assert!(!text.contains("IMG_4000x3000_original"));
    assert!(!text.contains(dir.to_str().unwrap()));

    // Offline sources are deferred, not failed.
    assert!(matches!(
        ps_llm::preview::derive(ps_llm::PreviewInput::Offline),
        Err(ps_llm::PreviewError::Deferred(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scenario_llm_providers_valid_insight_parsed() {
    let answer = insight_json("pets", 2, 1, &["dog"]).to_string();
    let server = MockServer::start(vec![Exchange::reply(Reply::json(
        200,
        &openai_reply(&answer),
    ))]);
    let d = describe(lm_studio(&server)).unwrap();
    let i = &d.insight;
    assert_eq!(i.category, SceneCategory::Pets);
    assert_eq!(
        (i.people.adults, i.people.children, i.people.total),
        (2, 1, 3)
    );
    assert_eq!(i.pets, vec!["dog"]);
    assert_eq!(d.provenance.schema_version, 1);
    assert_eq!(d.provenance.prompt_version, ps_llm::PROMPT_VERSION);
    assert!(!d.repaired);
}

#[test]
fn scenario_llm_providers_invalid_response_repaired_once() {
    let valid = insight_json("food", 0, 0, &[]).to_string();
    let server = MockServer::start(vec![
        Exchange::reply(Reply::json(
            200,
            &openai_reply("Sure! The image shows a plate of pasta."),
        )),
        Exchange::reply(Reply::json(200, &openai_reply(&valid))),
    ]);
    let d = describe(lm_studio(&server)).unwrap();
    assert_eq!(server.request_count(), 2, "exactly two requests");
    assert!(d.repaired);
    assert_eq!(d.insight.category, SceneCategory::Food);
    assert_eq!(d.provenance.requests, 2);
    // The repair request carries the previous answer and the validation error, and the same single image.
    let repair = server.requests()[1].json();
    let msgs = repair["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[2]["role"], "assistant");
    assert!(
        msgs[3]["content"]
            .as_str()
            .unwrap()
            .contains("no JSON object")
    );
    assert_eq!(server.requests()[1].images().len(), 1);
    server.assert_all_matched();
}

#[test]
fn scenario_llm_providers_second_failure_recorded() {
    let server = MockServer::start(vec![
        Exchange::reply(Reply::json(200, &openai_reply("I see a cat."))),
        Exchange::reply(Reply::json(
            200,
            &openai_reply(r#"{"subject":"A cat","category":"kitten-ish"}"#),
        )),
    ]);
    let err = describe(lm_studio(&server)).unwrap_err();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 2, "no third request");
    server.assert_all_matched();
    match &err.error {
        LlmError::Invalid(msg) => assert!(msg.contains("kitten-ish"), "{msg}"),
        other => panic!("expected a validation failure, got {other:?}"),
    }
    assert_eq!(err.requests, 2);
}

#[test]
fn scenario_llm_providers_test_unreachable_provider() {
    let mut cfg = ProviderConfig::default_local();
    cfg.base_url = format!("http://127.0.0.1:{}/v1", closed_port());
    let p = provider_for(cfg).expect("provider");
    let t0 = Instant::now();
    let report = rt().block_on(test_connection(&*p));
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert!(!report.reachable);
    let reason = report.error.unwrap();
    assert!(reason.contains("cannot reach"), "{reason}");
    assert!(report.model_found.is_none() && report.accepts_images.is_none());
}

#[test]
fn connection_test_reports_model_and_vision() {
    // LM Studio's richer listing says which model is a loaded vision model; no photo is sent.
    let server = MockServer::start(vec![Exchange::reply(Reply::json(
        200,
        &json!({"object": "list", "data": [
            {"id": "text-embedding-nomic-embed-text-v1.5", "object": "model", "type": "embeddings", "state": "not-loaded"},
            {"id": "qwen/qwen3.5-9b", "object": "model", "type": "llm", "state": "not-loaded"},
            {"id": "google/gemma-4-e4b", "object": "model", "type": "vlm", "state": "loaded"}
        ]}),
    ))]);
    let mut cfg = ProviderConfig::default_local();
    cfg.base_url = format!("{}/v1", server.url());
    let report = rt().block_on(test_connection(&*provider_for(cfg).expect("provider")));
    assert!(report.reachable, "{report:?}");
    assert_eq!(report.model.as_deref(), Some("google/gemma-4-e4b"));
    assert_eq!(
        (report.model_found, report.accepts_images),
        (Some(true), Some(true))
    );
    assert!(report.latency.is_some());
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].path, "/api/v0/models");
    assert!(reqs[0].images().is_empty());

    // A plain OpenAI-compatible server and a model that is not offered.
    let server = MockServer::start(vec![
        Exchange::reply(Reply::json(404, &json!({"error": "not found"}))),
        Exchange::reply(Reply::json(
            200,
            &json!({"object": "list", "data": [{"id": "llava:13b"}]}),
        )),
    ]);
    let mut cfg = preset("ollama");
    cfg.base_url = format!("{}/v1", server.url());
    cfg.model = "qwen2.5vl:7b".into();
    let report = rt().block_on(test_connection(&*provider_for(cfg).expect("provider")));
    assert!(report.reachable);
    assert_eq!(report.model_found, Some(false));
    assert!(report.error.unwrap().contains("qwen2.5vl:7b"));
}

#[test]
fn scenario_llm_providers_rate_limit_backs_off() {
    let valid = insight_json("nature", 0, 0, &[]).to_string();
    let server = MockServer::start(vec![
        Exchange::reply(
            Reply::json(429, &json!({"error": {"message": "Rate limit reached"}}))
                .with_header("retry-after", "3"),
        ),
        Exchange::reply(Reply::json(200, &openai_reply(&valid))),
    ]);
    let d = describe(lm_studio(&server)).unwrap();
    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    let gap = reqs[1].at.duration_since(reqs[0].at);
    assert!(gap >= Duration::from_secs(3), "retried after {gap:?}");
    assert!(gap < Duration::from_secs(6), "{gap:?}");
    assert_eq!(d.insight.category, SceneCategory::Nature);
    assert_eq!(d.provenance.requests, 2);
}

#[test]
fn server_errors_retry_twice_then_fail_and_auth_is_not_retried() {
    let server = MockServer::with_handler(|_| {
        Reply::json(503, &json!({"error": {"message": "loading model"}}))
    });
    let mut cfg = lm_studio(&server);
    cfg.retry = fast_retries();
    let err = describe(cfg).unwrap_err();
    assert_eq!(server.request_count(), 3);
    assert!(matches!(err.error, LlmError::Http { status: 503, .. }));

    let server = MockServer::with_handler(|_| {
        Reply::json(
            401,
            &json!({"error": {"message": "invalid x-api-key sk-ant-secret-123"}}),
        )
    });
    let mut cfg = preset("anthropic");
    cfg.base_url = format!("{}/v1", server.url());
    cfg.api_key = Some(ApiKey::new("sk-ant-secret-123"));
    cfg.retry = fast_retries();
    let err = describe(cfg).unwrap_err();
    assert_eq!(server.request_count(), 1);
    assert!(!err.to_string().contains("sk-ant-secret-123"));
}

#[test]
fn rejected_response_format_falls_back_to_prompt_only() {
    let valid = insight_json("object", 0, 0, &[]).to_string();
    let server = MockServer::start(vec![
        Exchange::reply(Reply::json(
            400,
            &json!({"error": {"message": "'response_format.type' must be 'json_object' or 'text'"}}),
        )),
        Exchange::reply(Reply::json(200, &openai_reply(&valid))),
        Exchange::reply(Reply::json(200, &openai_reply(&valid))),
    ]);
    let cfg = lm_studio(&server);
    assert_eq!(cfg.structured, StructuredOutput::JsonSchema);
    let d = Describer::new(provider_for(cfg).expect("provider"));
    let rt = rt();
    rt.block_on(d.describe(&preview(1))).unwrap();
    rt.block_on(d.describe(&preview(2))).unwrap();
    let reqs = server.requests();
    assert_eq!(reqs.len(), 3);
    assert!(reqs[0].json().get("response_format").is_some());
    assert!(reqs[1].json().get("response_format").is_none());
    assert!(
        reqs[2].json().get("response_format").is_none(),
        "remembered"
    );
}

#[test]
fn unconfigured_model_resolves_to_the_loaded_one() {
    let valid = insight_json("urban", 0, 0, &[]).to_string();
    let server = MockServer::start(vec![
        Exchange::reply(Reply::json(
            200,
            &json!({"data": [{"id": "google/gemma-4-e4b", "type": "vlm", "state": "loaded"}]}),
        )),
        Exchange::reply(Reply::json(200, &openai_reply(&valid))),
    ]);
    let mut cfg = ProviderConfig::default_local();
    cfg.base_url = format!("{}/v1", server.url());
    let d = Describer::new(provider_for(cfg).expect("provider"));
    rt().block_on(d.describe(&preview(3))).unwrap();
    assert_eq!(server.requests()[1].json()["model"], "google/gemma-4-e4b");
}
