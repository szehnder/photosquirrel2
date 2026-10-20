//! Opt-in live smoke test against a running LM Studio (spec "Live local smoke test").
//!
//! `PHOTOSQUIRREL_LIVE_LLM=1 cargo test -p ps-llm --test live -- --nocapture`
//! Optional: `PHOTOSQUIRREL_LIVE_LLM_URL` (default `http://localhost:1234/v1`), `PHOTOSQUIRREL_LIVE_LLM_MODEL`
//! (default: the loaded vision model), `PHOTOSQUIRREL_LIVE_LLM_IMAGES` (default 1),
//! `PHOTOSQUIRREL_LIVE_LLM_TIMEOUT_S` (default 600 per request).
//! Only synthetic `ps-fixtures` scenes are sent, never real photos. Skipped (passes) without the variable.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use ps_llm::{Describer, PreviewInput, ProviderConfig, provider_for, test_connection};

fn live() -> bool {
    std::env::var("PHOTOSQUIRREL_LIVE_LLM").is_ok_and(|v| v == "1")
}

/// A synthetic 2048 px "photo" (the cached-preview tier), as JPEG.
fn synthetic_preview(i: u64) -> Vec<u8> {
    let mut rng = ps_fixtures::rng::derive(7, "live-llm", i);
    let scene = ps_fixtures::synth::Scene::random(&mut rng);
    let img = ps_fixtures::synth::render(
        &scene,
        &ps_fixtures::synth::Perturb::identity(i),
        2048,
        1536,
    );
    ps_fixtures::jpeg::encode_jpeg(&img, 90)
}

#[test]
fn scenario_llm_providers_live_lm_studio_smoke() {
    if !live() {
        eprintln!("skipped: set PHOTOSQUIRREL_LIVE_LLM=1 with LM Studio serving a vision model");
        return;
    }
    let mut cfg = ProviderConfig::default_local();
    // This machine runs local models slowly: allow up to 10 minutes per image unless overridden.
    cfg.timeouts.request = std::time::Duration::from_secs(
        std::env::var("PHOTOSQUIRREL_LIVE_LLM_TIMEOUT_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(600),
    );
    if let Ok(u) = std::env::var("PHOTOSQUIRREL_LIVE_LLM_URL") {
        cfg.base_url = u;
    }
    if let Ok(m) = std::env::var("PHOTOSQUIRREL_LIVE_LLM_MODEL") {
        cfg.model = m;
    }
    let n: u64 = std::env::var("PHOTOSQUIRREL_LIVE_LLM_IMAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let rt = ps_llm::LlmRuntime::shared();
    let provider = provider_for(cfg);
    let report = rt.block_on(test_connection(&*provider));
    eprintln!("connection test: {report:?}");
    assert!(
        report.reachable,
        "LM Studio is not reachable: {:?}",
        report.error
    );
    let describer = Describer::new(provider);
    for i in 0..n {
        let jpeg = synthetic_preview(i);
        let derived = ps_llm::preview::derive(PreviewInput::Cached(&jpeg)).unwrap();
        let t0 = Instant::now();
        let d = rt
            .block_on(describer.describe(&derived.llm))
            .expect("schema-valid insight");
        eprintln!(
            "image {i}: {:.1} s, model {}, prompt v{}, requests {}, repaired {}\n  {}",
            t0.elapsed().as_secs_f64(),
            d.provenance.model,
            d.provenance.prompt_version,
            d.provenance.requests,
            d.repaired,
            serde_json::to_string(&d.insight).unwrap()
        );
        assert_eq!(d.provenance.schema_version, 1);
        assert_eq!(d.provenance.prompt_version, ps_llm::PROMPT_VERSION);
        assert!(!d.insight.subject.is_empty());
    }
}
