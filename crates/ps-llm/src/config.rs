//! Provider configuration: wire format, endpoint, model, key, timeouts and retry policy (ADR-0020).

use std::net::IpAddr;
use std::time::Duration;

use ps_model::settings::{Ai, ProviderId};

/// The three wire formats every provider is expressed in (ADR-0020).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WireFormat {
    /// `POST {base}/chat/completions` (LM Studio, Ollama, OpenAI, OpenRouter, Groq, llama.cpp, vLLM, ...).
    OpenAiCompatible,
    /// `POST {base}/messages`.
    Anthropic,
    /// `POST {base}/models/{model}:generateContent`.
    Gemini,
}

/// How the JSON answer is requested from an OpenAI-compatible server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StructuredOutput {
    /// `response_format: {type: json_schema, ...}` (LM Studio, OpenAI, recent Ollama, OpenRouter).
    JsonSchema,
    /// `response_format: {type: json_object}` (Groq and older servers).
    JsonObject,
    /// Schema in the prompt only.
    PromptOnly,
}

/// A provider API key. `Debug` and `Display` never print it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key itself, for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// Request timeout for local (loopback / private-network) providers.
pub const LOCAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Connect and whole-request timeouts. Hosted providers default to 120 s; local ones get [`LOCAL_REQUEST_TIMEOUT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    pub request: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(120),
        }
    }
}

/// Retries inside one describe call (ADR-0020, spec "Rate limits respected", "Bounded retries"):
/// 429 waits for `retry-after` (else exponential backoff from `rate_limit_base`); 5xx, 408, timeouts and
/// unreachable servers back off `server_backoff[i]`. At most `max_retries` retries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub rate_limit_base: Duration,
    pub server_backoff: Vec<Duration>,
    /// Upper bound for a provider-sent `retry-after`.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            rate_limit_base: Duration::from_secs(2),
            server_backoff: vec![Duration::from_secs(2), Duration::from_secs(8)],
            max_retry_after: Duration::from_secs(120),
        }
    }
}

impl RetryPolicy {
    /// No retries at all (connection tests).
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// Delay before retry number `retry` (1-based) after a 429 without `retry-after`.
    pub fn rate_limit_delay(&self, retry: u32) -> Duration {
        self.rate_limit_base
            .saturating_mul(1u32 << retry.saturating_sub(1).min(10))
    }

    /// Delay before retry number `retry` (1-based) after a server error or timeout.
    pub fn server_delay(&self, retry: u32) -> Duration {
        let i = retry.saturating_sub(1) as usize;
        self.server_backoff
            .get(i)
            .or(self.server_backoff.last())
            .copied()
            .unwrap_or(Duration::from_secs(2))
    }
}

/// A named starting point for the provider settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub wire: WireFormat,
    pub base_url: &'static str,
    /// Previews leave the machine: needs consent (ADR-0021).
    pub hosted: bool,
    /// Key override variable (ADR-0031), if the provider takes a key.
    pub env_var: Option<&'static str>,
    /// Suggested model; empty means "the server's loaded model".
    pub default_model: &'static str,
    pub structured: StructuredOutput,
    /// The settings provider id this preset is stored under.
    pub settings_id: ProviderId,
}

/// Every built-in preset. OpenRouter and Groq are OpenAI-compatible endpoints stored as
/// [`ProviderId::OpenAiCompatible`] with their base URL; their non-local URL makes them hosted.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "lm_studio",
        label: "LM Studio (local)",
        wire: WireFormat::OpenAiCompatible,
        base_url: "http://localhost:1234/v1",
        hosted: false,
        env_var: None,
        default_model: "",
        structured: StructuredOutput::JsonSchema,
        settings_id: ProviderId::LmStudio,
    },
    Preset {
        id: "ollama",
        label: "Ollama (local)",
        wire: WireFormat::OpenAiCompatible,
        base_url: "http://localhost:11434/v1",
        hosted: false,
        env_var: None,
        default_model: "",
        structured: StructuredOutput::JsonSchema,
        settings_id: ProviderId::OpenAiCompatible,
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        wire: WireFormat::OpenAiCompatible,
        base_url: "https://api.openai.com/v1",
        hosted: true,
        env_var: Some("OPENAI_API_KEY"),
        default_model: "gpt-4o-mini",
        structured: StructuredOutput::JsonSchema,
        settings_id: ProviderId::OpenAi,
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        wire: WireFormat::OpenAiCompatible,
        base_url: "https://openrouter.ai/api/v1",
        hosted: true,
        env_var: None,
        default_model: "",
        structured: StructuredOutput::JsonSchema,
        settings_id: ProviderId::OpenAiCompatible,
    },
    Preset {
        id: "groq",
        label: "Groq",
        wire: WireFormat::OpenAiCompatible,
        base_url: "https://api.groq.com/openai/v1",
        hosted: true,
        env_var: None,
        default_model: "",
        structured: StructuredOutput::JsonObject,
        settings_id: ProviderId::OpenAiCompatible,
    },
    Preset {
        id: "anthropic",
        label: "Anthropic",
        wire: WireFormat::Anthropic,
        base_url: "https://api.anthropic.com/v1",
        hosted: true,
        env_var: Some("ANTHROPIC_API_KEY"),
        default_model: "claude-haiku-4-5",
        structured: StructuredOutput::PromptOnly,
        settings_id: ProviderId::Anthropic,
    },
    Preset {
        id: "gemini",
        label: "Google Gemini",
        wire: WireFormat::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        hosted: true,
        env_var: Some("GEMINI_API_KEY"),
        default_model: "gemini-2.5-flash",
        structured: StructuredOutput::JsonSchema,
        settings_id: ProviderId::Gemini,
    },
];

/// The preset with this id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Everything needed to talk to one provider.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderConfig {
    /// Shown to the user (consent prompt, errors) and stored as result provenance.
    pub name: String,
    pub wire: WireFormat,
    /// Without a trailing slash.
    pub base_url: String,
    /// Empty: the server's loaded model (OpenAI-compatible local servers only).
    pub model: String,
    pub api_key: Option<ApiKey>,
    pub structured: StructuredOutput,
    pub timeouts: Timeouts,
    pub retry: RetryPolicy,
    /// Upper bound of generated tokens per answer.
    pub max_output_tokens: u32,
    /// Explicitly a hosted provider, regardless of the URL.
    pub hosted_provider: bool,
}

impl ProviderConfig {
    /// A configuration from a preset, with the preset's default model.
    pub fn from_preset(p: &Preset) -> Self {
        Self {
            name: p.label.to_owned(),
            wire: p.wire,
            base_url: p.base_url.to_owned(),
            model: p.default_model.to_owned(),
            api_key: None,
            structured: p.structured,
            timeouts: Timeouts::default(),
            retry: RetryPolicy::default(),
            max_output_tokens: 1024,
            hosted_provider: p.hosted,
        }
    }

    /// The configuration described by the user's AI settings. The key is looked up by the caller
    /// (`ps-service::secrets`, ADR-0031) and passed in; settings never hold it.
    pub fn from_settings(ai: &Ai, api_key: Option<ApiKey>) -> Self {
        let p = match ai.provider {
            ProviderId::LmStudio => preset("lm_studio"),
            ProviderId::OpenAiCompatible => PRESETS
                .iter()
                .find(|p| {
                    p.settings_id == ProviderId::OpenAiCompatible
                        && same_url(p.base_url, &ai.base_url)
                })
                .or_else(|| preset("ollama")),
            ProviderId::OpenAi => preset("openai"),
            ProviderId::Anthropic => preset("anthropic"),
            ProviderId::Gemini => preset("gemini"),
        };
        let mut cfg = Self::from_preset(p.unwrap_or(&PRESETS[0]));
        if ai.provider == ProviderId::OpenAiCompatible
            && !p.is_some_and(|p| same_url(p.base_url, &ai.base_url))
        {
            cfg.name = format!("OpenAI-compatible server at {}", ai.base_url.trim());
        }
        if !ai.base_url.trim().is_empty() {
            cfg.base_url = ai.base_url.trim().trim_end_matches('/').to_owned();
        }
        if !ai.model.trim().is_empty() {
            cfg.model = ai.model.trim().to_owned();
        }
        cfg.api_key = api_key;
        // Local vision models on modest hardware can take minutes per image; don't fail them on a hosted-API
        // timeout. `PHOTOSQUIRREL_LLM_TIMEOUT_S` overrides either default.
        if !cfg.is_hosted() {
            cfg.timeouts.request = LOCAL_REQUEST_TIMEOUT;
        }
        if let Some(secs) = std::env::var("PHOTOSQUIRREL_LLM_TIMEOUT_S")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
        {
            cfg.timeouts.request = Duration::from_secs(secs);
        }
        cfg
    }

    /// The configuration with default settings: LM Studio at `http://localhost:1234/v1`, no key.
    pub fn default_local() -> Self {
        Self::from_settings(&Ai::default(), None)
    }

    /// Previews would leave this machine: the provider is a hosted one, or its URL is not loopback or a
    /// private-network address. Hosted providers need explicit consent (ADR-0021).
    pub fn is_hosted(&self) -> bool {
        self.hosted_provider || !is_local_url(&self.base_url)
    }

    /// `base_url` joined with `path` (which starts with `/`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// Loopback, private (RFC 1918 / ULA), link-local, `localhost` and `*.local` hosts.
pub fn is_local_url(base_url: &str) -> bool {
    let Ok(u) = url::Url::parse(base_url.trim()) else {
        return false;
    };
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local")
        }
        Some(url::Host::Ipv4(ip)) => local_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => local_ip(IpAddr::V6(ip)),
        None => false,
    }
}

fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_detection() {
        assert!(is_local_url("http://localhost:1234/v1"));
        assert!(is_local_url("http://127.0.0.1:1234/v1"));
        assert!(is_local_url("http://192.168.1.20:11434/v1"));
        assert!(is_local_url("http://[::1]:1234/v1"));
        assert!(!is_local_url("https://openrouter.ai/api/v1"));
        let mut ai = Ai {
            provider: ProviderId::OpenAiCompatible,
            base_url: "https://openrouter.ai/api/v1".into(),
            ..Ai::default()
        };
        let cfg = ProviderConfig::from_settings(&ai, None);
        assert_eq!(cfg.name, "OpenRouter");
        assert!(
            cfg.is_hosted(),
            "a local provider id with a remote URL is hosted"
        );
        ai.base_url = "http://localhost:11434/v1".into();
        assert!(!ProviderConfig::from_settings(&ai, None).is_hosted());
        ai.provider = ProviderId::Gemini;
        ai.base_url = ProviderId::Gemini.default_base_url().into();
        let g = ProviderConfig::from_settings(&ai, None);
        assert!(g.is_hosted());
        assert_eq!(g.wire, WireFormat::Gemini);
    }

    #[test]
    fn retry_delays() {
        let r = RetryPolicy::default();
        assert_eq!(r.rate_limit_delay(1), Duration::from_secs(2));
        assert_eq!(r.rate_limit_delay(2), Duration::from_secs(4));
        assert_eq!(r.server_delay(1), Duration::from_secs(2));
        assert_eq!(r.server_delay(2), Duration::from_secs(8));
        assert_eq!(r.server_delay(3), Duration::from_secs(8));
        assert_eq!(
            format!("{:?}", ApiKey::new("sk-secret")),
            "ApiKey(<redacted>)"
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[test]
    fn local_providers_get_the_long_request_timeout() {
        let local = ProviderConfig::default_local();
        assert_eq!(local.timeouts.request, LOCAL_REQUEST_TIMEOUT);
        let hosted = ProviderConfig::from_preset(preset("openai").expect("openai preset"));
        assert_eq!(hosted.timeouts.request, Timeouts::default().request);
    }
}
