//! API keys for AI providers: keychain + env override, never on disk (spec `llm-providers`, ADR-0031).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ps_llm::testing::{MockServer, Reply, insight_json, preview};
use ps_model::settings::{ProviderId, Settings};
use ps_service::ai;
use ps_service::secrets::{MemoryBackend, Secrets, StoredIn};
use ps_service::settings::SettingsStore;
use serde_json::json;

/// Captures every log record of this test binary into a file (as the app's log file would).
struct FileLogger(Mutex<std::fs::File>);

impl log::Log for FileLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, r: &log::Record<'_>) {
        let mut f = self.0.lock().unwrap();
        let _ = writeln!(f, "{} {}: {}", r.level(), r.target(), r.args());
    }
    fn flush(&self) {
        let _ = self.0.lock().unwrap().flush();
    }
}

fn log_file() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let p = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ps-service-ai-keys-{}.log", std::process::id()));
        let f = std::fs::File::create(&p).unwrap();
        log::set_logger(Box::leak(Box::new(FileLogger(Mutex::new(f))))).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        p
    })
}

fn anthropic_reply() -> Reply {
    Reply::json(
        200,
        &json!({
            "id": "msg_test", "type": "message", "role": "assistant", "model": "claude-haiku-4-5",
            "content": [{ "type": "text", "text": insight_json("pets", 1, 0, &["cat"]).to_string() }],
            "stop_reason": "end_turn", "usage": { "input_tokens": 10, "output_tokens": 10 }
        }),
    )
}

fn anthropic_settings(server: &MockServer) -> Settings {
    let mut s = Settings::default();
    s.ai.provider = ProviderId::Anthropic;
    s.ai.base_url = format!("{}/v1", server.url());
    s.ai.model = "claude-haiku-4-5".into();
    s.ai.consent = ps_service::ai::consent_for(&s.ai);
    s
}

fn describe(settings: &Settings, secrets: &Secrets) {
    let (cfg, _) = ai::provider_config(settings, secrets);
    let d = ps_llm::Describer::new(ps_llm::provider_for(cfg));
    ps_llm::LlmRuntime::shared()
        .block_on(d.describe(&preview(1)))
        .unwrap();
}

fn file_contains(path: &Path, needle: &str) -> bool {
    std::fs::read(path).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes()))
}

fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(all_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn scenario_llm_providers_key_not_written_to_disk() {
    let log = log_file();
    const KEY: &str = "sk-ant-api03-SECRET-never-on-disk-7f3a";
    // First a server error (logged retry) and an error body echoing the key, then success.
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let server = MockServer::with_handler(move |_| {
        let mut n = c.lock().unwrap();
        *n += 1;
        match *n {
            1 => Reply::json(
                503,
                &json!({"error": {"message": format!("overloaded for key {KEY}")}}),
            ),
            _ => anthropic_reply(),
        }
    });
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    let data_dir = tmp.path().join("data");

    // Save the key (keychain) and the settings (file).
    let secrets = Secrets::with_backend(Arc::new(MemoryBackend::default())).with_env(|_| None);
    let mut settings = anthropic_settings(&server);
    let account = ai::key_account(&settings.ai, ProviderId::Anthropic);
    assert_eq!(
        secrets.set_api_key(&account, KEY).unwrap(),
        StoredIn::Keychain
    );
    settings
        .ai
        .key_accounts
        .push(ps_model::settings::KeyAccount {
            provider: ProviderId::Anthropic,
            account: account.clone(),
        });
    let store = SettingsStore::in_dir(&config_dir);
    store.save(&settings).unwrap();
    let catalog = ps_catalog::Catalog::open(&data_dir).unwrap();

    // Make a request (with a fast retry so the retry is logged).
    let (mut cfg, from) = ai::provider_config(&settings, &secrets);
    assert_eq!(from, Some(StoredIn::Keychain));
    assert!(!format!("{cfg:?}").contains(KEY), "Debug redacts the key");
    cfg.retry.server_backoff = vec![std::time::Duration::from_millis(10)];
    let d = ps_llm::Describer::new(ps_llm::provider_for(cfg));
    ps_llm::LlmRuntime::shared()
        .block_on(d.describe(&preview(1)))
        .unwrap();
    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[1].header("x-api-key"),
        Some(KEY),
        "the key is sent, in the header only"
    );
    catalog.close().unwrap();
    log::logger().flush();

    // Absent from the settings file, the catalog and the log file.
    let settings_text = std::fs::read_to_string(store.path()).unwrap();
    assert!(
        settings_text.contains(&account),
        "settings reference the keychain account"
    );
    assert!(!settings_text.contains(KEY));
    let mut checked = 0;
    for f in all_files(tmp.path()) {
        assert!(!file_contains(&f, KEY), "key found in {}", f.display());
        checked += 1;
    }
    assert!(checked >= 2, "settings file and catalog checked");
    let log_text = std::fs::read_to_string(log).unwrap();
    assert!(
        log_text.contains("retry 1/2"),
        "the retry was logged: {log_text}"
    );
    assert!(!log_text.contains(KEY), "key found in the log");
}

#[test]
fn scenario_llm_providers_env_var_overrides_key() {
    let _ = log_file();
    let server = MockServer::with_handler(|_| anthropic_reply());
    let backend = Arc::new(MemoryBackend::default());
    let secrets = Secrets::with_backend(backend)
        .with_env(|name| (name == "ANTHROPIC_API_KEY").then(|| "sk-ant-from-env".to_owned()));
    let settings = anthropic_settings(&server);
    secrets
        .set_api_key(
            &ai::key_account(&settings.ai, ProviderId::Anthropic),
            "sk-ant-stored",
        )
        .unwrap();
    let (_, from) = ai::provider_config(&settings, &secrets);
    assert_eq!(from, Some(StoredIn::Environment));
    describe(&settings, &secrets);
    assert_eq!(
        server.requests()[0].header("x-api-key"),
        Some("sk-ant-from-env")
    );
}

#[test]
fn local_provider_sends_no_key_and_needs_no_consent() {
    let server = MockServer::with_handler(|_| {
        Reply::json(
            200,
            &ps_llm::testing::openai_reply(&insight_json("food", 0, 0, &[]).to_string()),
        )
    });
    let secrets = Secrets::with_backend(Arc::new(MemoryBackend::default()))
        .with_env(|_| Some("sk-should-not-be-sent".to_owned()));
    let mut settings = Settings::default();
    settings.ai.base_url = format!("{}/v1", server.url());
    settings.ai.model = "m".into();
    let (cfg, from) = ai::provider_config(&settings, &secrets);
    assert!(from.is_none());
    assert!(ai::check_consent(&settings, &cfg).is_ok());
    describe(&settings, &secrets);
    assert!(server.requests()[0].header("authorization").is_none());

    settings.ai.provider = ProviderId::Gemini;
    settings.ai.base_url = ProviderId::Gemini.default_base_url().into();
    let (cfg, _) = ai::provider_config(&settings, &secrets);
    let err = ai::check_consent(&settings, &cfg).unwrap_err();
    assert_eq!(err.provider, "Google Gemini");
}
