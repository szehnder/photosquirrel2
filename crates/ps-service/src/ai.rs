//! AI insights facade (change `ai-insights`): provider configuration from settings plus keychain keys
//! (ADR-0020, ADR-0031) and the hosted-provider consent gate (ADR-0021).
//!
//! The queue engine itself is `ps_llm::queue` (it needs the `ps-llm` tokio runtime, ADR-0009); its
//! catalog-backed store over migration 5 (`ai_queue`, `ai_result`, `ai_correction`, `ai_suggestion`) is added
//! here once catalog migration 4 has landed and migration 5 can be registered.

use ps_model::settings::{Ai, ProviderId, Settings};

use crate::secrets::{ApiKey as StoredKey, Secrets, StoredIn, default_account};

pub use ps_llm::consent::ConsentRequired;
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

/// `Ok` when previews may be sent: a local provider, or a hosted one the user consented to by name.
pub fn check_consent(settings: &Settings, cfg: &ProviderConfig) -> Result<(), ConsentRequired> {
    ps_llm::consent::check(cfg, settings.ai.hosted_consent)
}

/// Run "Test connection" for the configured provider, blocking the calling (non-UI) thread.
pub fn test_connection(settings: &Settings, secrets: &Secrets) -> TestReport {
    let (cfg, _) = provider_config(settings, secrets);
    let provider = ps_llm::provider_for(cfg);
    ps_llm::LlmRuntime::shared().block_on(ps_llm::test_connection(&*provider))
}
