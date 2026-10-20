//! User settings shared by the app and the CLI (spec `settings`, ADR-0031).
//!
//! Plain data with serde defaults on every level, so older files load (missing keys take defaults) and newer
//! keys survive in memory. The file format is TOML (`ps-service::settings` reads and writes it). **No secrets
//! live here**: AI providers are referenced by id and keychain account name only (ADR-0031).
//!
//! Not glob re-exported from the crate root: use `ps_model::settings::Settings`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ClassifierSettings;

/// Current `settings_version`. Bump with a migration in `ps-service::settings` when a field changes meaning.
pub const SETTINGS_VERSION: u32 = 1;

/// Everything the user can set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub settings_version: u32,
    pub general: General,
    pub appearance: Appearance,
    pub scanning: Scanning,
    pub ai: Ai,
    pub export: Export,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            general: General::default(),
            appearance: Appearance::default(),
            scanning: Scanning::default(),
            ai: Ai::default(),
            export: Export::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Offer to resume an interrupted scan at start-up (ADR-0008).
    pub offer_resume: bool,
    /// Pause running scans while on battery power.
    pub pause_on_battery: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            offer_resume: true,
            pause_on_battery: false,
        }
    }
}

/// Light or dark interface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSetting {
    /// Graphite dark, the photo-first default (ADR-0028).
    #[default]
    Dark,
    Light,
    /// Follow the operating system.
    System,
}

impl ThemeSetting {
    pub const ALL: [ThemeSetting; 3] = [
        ThemeSetting::Dark,
        ThemeSetting::Light,
        ThemeSetting::System,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ThemeSetting::Dark => "dark",
            ThemeSetting::Light => "light",
            ThemeSetting::System => "system",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeSetting::Dark => "Graphite dark",
            ThemeSetting::Light => "Graphite light",
            ThemeSetting::System => "Match system",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// Whether the interface animates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSetting {
    /// Follow the operating system's reduce-motion preference.
    #[default]
    System,
    Reduce,
    Full,
}

impl MotionSetting {
    pub const ALL: [MotionSetting; 3] = [
        MotionSetting::System,
        MotionSetting::Reduce,
        MotionSetting::Full,
    ];

    pub fn id(self) -> &'static str {
        match self {
            MotionSetting::System => "system",
            MotionSetting::Reduce => "reduce",
            MotionSetting::Full => "full",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MotionSetting::System => "Match system",
            MotionSetting::Reduce => "Reduce",
            MotionSetting::Full => "Full",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// Whether to reduce motion, given the OS preference.
    pub fn reduce(self, os_reduce: bool) -> bool {
        match self {
            MotionSetting::System => os_reduce,
            MotionSetting::Reduce => true,
            MotionSetting::Full => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemeSetting,
    pub motion: MotionSetting,
    /// Interface text size in logical pixels; scales the whole UI.
    pub ui_font_size: f32,
}

pub const DEFAULT_UI_FONT_SIZE: f32 = 13.0;
pub const MIN_UI_FONT_SIZE: f32 = 11.0;
pub const MAX_UI_FONT_SIZE: f32 = 20.0;

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeSetting::Dark,
            motion: MotionSetting::System,
            ui_font_size: DEFAULT_UI_FONT_SIZE,
        }
    }
}

/// Settings-level additions to the probe classifier thresholds ([`ClassifierSettings`], ADR-0015).
impl ClassifierSettings {
    /// A hash of every field that affects classification, stable across releases and platforms (FNV-1a over a
    /// canonical text form). Stored per file at classification time; a different hash on rescan means the file
    /// is re-classified from its stored probe fields, without reading its contents again.
    pub fn stable_hash(&self) -> u64 {
        let mut patterns = self.extra_cache_patterns.clone();
        patterns.sort();
        let canonical = format!(
            "classifier/v1;min_side_px={};min_pixels={};min_video_ms={};min_video_side_px={};cache={}",
            self.min_side_px,
            self.min_pixels,
            self.min_video_duration_ms,
            self.min_video_side_px,
            patterns.join("|")
        );
        fnv1a(canonical.as_bytes())
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scanning {
    pub classifier: ClassifierSettings,
    /// External tool for video poster frames (e.g. `ffmpeg`), ADR-0017. `None`: search `PATH`, else skip.
    pub poster_frame_tool: Option<PathBuf>,
    /// Cap of the 2048 px preview cache in GB (ADR-0016).
    pub preview_cache_gb: f64,
    /// Scan hidden folders (dot-folders) too.
    pub include_hidden: bool,
}

impl Default for Scanning {
    fn default() -> Self {
        Self {
            classifier: ClassifierSettings::default(),
            poster_frame_tool: None,
            preview_cache_gb: 4.0,
            include_hidden: false,
        }
    }
}

/// LLM providers (ADR-0020).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// Local LM Studio (OpenAI-compatible), the default: nothing leaves the machine.
    #[default]
    LmStudio,
    /// Any other local OpenAI-compatible server (Ollama, llama.cpp, vLLM).
    OpenAiCompatible,
    OpenAi,
    Anthropic,
    Gemini,
}

impl ProviderId {
    pub const ALL: [ProviderId; 5] = [
        ProviderId::LmStudio,
        ProviderId::OpenAiCompatible,
        ProviderId::OpenAi,
        ProviderId::Anthropic,
        ProviderId::Gemini,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ProviderId::LmStudio => "lm_studio",
            ProviderId::OpenAiCompatible => "openai_compatible",
            ProviderId::OpenAi => "openai",
            ProviderId::Anthropic => "anthropic",
            ProviderId::Gemini => "gemini",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderId::LmStudio => "LM Studio (local)",
            ProviderId::OpenAiCompatible => "OpenAI-compatible (local)",
            ProviderId::OpenAi => "OpenAI",
            ProviderId::Anthropic => "Anthropic",
            ProviderId::Gemini => "Google Gemini",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// Hosted providers send previews off the machine and need explicit consent (ADR-0021).
    pub fn is_hosted(self) -> bool {
        matches!(
            self,
            ProviderId::OpenAi | ProviderId::Anthropic | ProviderId::Gemini
        )
    }

    /// The default endpoint for the provider.
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderId::LmStudio => "http://localhost:1234/v1",
            ProviderId::OpenAiCompatible => "http://localhost:11434/v1",
            ProviderId::OpenAi => "https://api.openai.com/v1",
            ProviderId::Anthropic => "https://api.anthropic.com/v1",
            ProviderId::Gemini => "https://generativelanguage.googleapis.com/v1beta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ai {
    pub provider: ProviderId,
    pub base_url: String,
    /// Model id; empty means "the server's loaded model".
    pub model: String,
    /// Start the AI queue automatically after a scan. Off: the queue stays stopped until started.
    pub auto_start_queue: bool,
    /// The user agreed to send previews to a hosted provider (ADR-0021). Off by default.
    pub hosted_consent: bool,
    pub max_concurrent_requests: u32,
    /// Requests per minute; `None` means the provider default ([`Ai::effective_requests_per_minute`]:
    /// unlimited for local providers, [`HOSTED_DEFAULT_REQUESTS_PER_MINUTE`] for hosted ones).
    pub requests_per_minute: Option<u32>,
    /// Seconds one description request may take; `None` means the provider default
    /// ([`Ai::effective_request_timeout_s`]: 300 s local, 120 s hosted). Slow local vision models need long.
    pub request_timeout_s: Option<u32>,
    /// Keychain account name per provider (the key itself lives in the OS keychain, ADR-0031).
    pub key_accounts: Vec<KeyAccount>,
}

/// Default request timeouts in seconds (mirrors `ps_llm`'s defaults).
pub const LOCAL_REQUEST_TIMEOUT_S: u32 = 300;
pub const HOSTED_REQUEST_TIMEOUT_S: u32 = 120;

/// Default requests per minute for hosted providers (spec `ai-queue` "Concurrency and rate limits").
pub const HOSTED_DEFAULT_REQUESTS_PER_MINUTE: u32 = 30;

impl Ai {
    /// The request timeout in effect: the configured one, else 300 s for local and 120 s for hosted providers.
    pub fn effective_request_timeout_s(&self) -> u32 {
        self.request_timeout_s
            .filter(|s| *s > 0)
            .unwrap_or(if self.provider.is_hosted() {
                HOSTED_REQUEST_TIMEOUT_S
            } else {
                LOCAL_REQUEST_TIMEOUT_S
            })
    }

    /// The rate limit in effect: the configured one, else unlimited (`None`) for local providers and
    /// [`HOSTED_DEFAULT_REQUESTS_PER_MINUTE`] for hosted ones.
    pub fn effective_requests_per_minute(&self) -> Option<u32> {
        self.requests_per_minute.or(self
            .provider
            .is_hosted()
            .then_some(HOSTED_DEFAULT_REQUESTS_PER_MINUTE))
    }
}

/// Where a provider's API key is stored: an account name in the OS keychain, never the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyAccount {
    pub provider: ProviderId,
    pub account: String,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            provider: ProviderId::LmStudio,
            base_url: ProviderId::LmStudio.default_base_url().to_owned(),
            model: String::new(),
            auto_start_queue: false,
            hosted_consent: false,
            max_concurrent_requests: 2,
            requests_per_minute: None,
            request_timeout_s: None,
            key_accounts: Vec::new(),
        }
    }
}

/// Export target profiles (ADR-0022).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportProfile {
    #[default]
    Apple,
    Google,
    Lightroom,
}

impl ExportProfile {
    pub const ALL: [ExportProfile; 3] = [
        ExportProfile::Apple,
        ExportProfile::Google,
        ExportProfile::Lightroom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ExportProfile::Apple => "apple",
            ExportProfile::Google => "google",
            ExportProfile::Lightroom => "lightroom",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportProfile::Apple => "Apple Photos",
            ExportProfile::Google => "Google Photos",
            ExportProfile::Lightroom => "Lightroom",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Export {
    pub profile: ExportProfile,
    /// The last library folder exported to.
    pub library: Option<PathBuf>,
    /// Write XMP sidecars with tags and descriptions.
    pub sidecars: bool,
}

impl Default for Export {
    fn default() -> Self {
        Self {
            profile: ExportProfile::Apple,
            library: None,
            sidecars: true,
        }
    }
}

/// One invalid value: the setting's key (dotted path, e.g. `scanning.preview_cache_gb`) and a message for the
/// user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub key: &'static str,
    pub message: String,
}

impl Settings {
    /// Pure checks (ranges, formats). File-system checks such as "the poster-frame tool is executable" are
    /// done by `ps-service::settings`.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let mut err = |key: &'static str, message: String| errors.push(FieldError { key, message });
        let gb = self.scanning.preview_cache_gb;
        if !gb.is_finite() || gb <= 0.0 {
            err(
                "scanning.preview_cache_gb",
                "The preview cache must be larger than 0 GB.".into(),
            );
        } else if gb > 4096.0 {
            err(
                "scanning.preview_cache_gb",
                "The preview cache can be at most 4096 GB.".into(),
            );
        }
        let c = &self.scanning.classifier;
        if c.min_side_px == 0 || c.min_side_px > 10_000 {
            err(
                "scanning.classifier.min_side_px",
                "Minimum side must be between 1 and 10,000 px.".into(),
            );
        }
        if c.min_pixels > 100_000_000 {
            err(
                "scanning.classifier.min_pixels",
                "Minimum pixels can be at most 100 megapixels.".into(),
            );
        }
        if c.min_video_duration_ms > 3_600_000 {
            err(
                "scanning.classifier.min_video_duration_ms",
                "Minimum video length must be between 0 and 3600 s.".into(),
            );
        }
        let f = self.appearance.ui_font_size;
        if !f.is_finite() || !(MIN_UI_FONT_SIZE..=MAX_UI_FONT_SIZE).contains(&f) {
            err(
                "appearance.ui_font_size",
                format!("Text size must be between {MIN_UI_FONT_SIZE} and {MAX_UI_FONT_SIZE} px."),
            );
        }
        let url = self.ai.base_url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            err(
                "ai.base_url",
                "The server address must start with http:// or https://.".into(),
            );
        }
        if self.ai.max_concurrent_requests == 0 || self.ai.max_concurrent_requests > 64 {
            err(
                "ai.max_concurrent_requests",
                "Concurrent requests must be between 1 and 64.".into(),
            );
        }
        if self
            .ai
            .request_timeout_s
            .is_some_and(|s| !(10..=3_600).contains(&s))
        {
            err(
                "ai.request_timeout_s",
                "The request timeout must be between 10 and 3600 seconds.".into(),
            );
        }
        if self.ai.requests_per_minute == Some(0) {
            err(
                "ai.requests_per_minute",
                "Requests per minute must be at least 1 (leave it empty for the provider default)."
                    .into(),
            );
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_defaults_follow_the_spec() {
        // Spec ai-queue "Concurrency and rate limits": unlimited for local, 30/min for hosted providers.
        let mut ai = Ai::default();
        assert_eq!(ai.requests_per_minute, None);
        assert_eq!(ai.effective_requests_per_minute(), None);
        ai.provider = ProviderId::OpenAiCompatible;
        assert_eq!(ai.effective_requests_per_minute(), None);
        for p in [
            ProviderId::OpenAi,
            ProviderId::Anthropic,
            ProviderId::Gemini,
        ] {
            ai.provider = p;
            assert_eq!(ai.effective_requests_per_minute(), Some(30));
        }
        ai.requests_per_minute = Some(12);
        assert_eq!(ai.effective_requests_per_minute(), Some(12));
        let mut s = Settings::default();
        s.ai.requests_per_minute = Some(0);
        assert!(
            s.validate()
                .iter()
                .any(|e| e.key == "ai.requests_per_minute")
        );
    }

    #[test]
    fn defaults_follow_the_adrs() {
        let s = Settings::default();
        assert_eq!(s.settings_version, SETTINGS_VERSION);
        assert_eq!(s.appearance.theme, ThemeSetting::Dark);
        assert_eq!(s.ai.provider, ProviderId::LmStudio);
        assert_eq!(s.ai.base_url, "http://localhost:1234/v1");
        assert!(!s.ai.hosted_consent && !s.ai.auto_start_queue);
        let c = &s.scanning.classifier;
        assert_eq!((c.min_side_px, c.min_pixels), (200, 120_000));
        assert_eq!(c.min_video_duration_ms, 2_000);
        assert_eq!(s.scanning.preview_cache_gb, 4.0);
        assert!(s.validate().is_empty(), "{:?}", s.validate());
    }

    #[test]
    fn classifier_hash_is_stable_and_sensitive() {
        let a = ClassifierSettings::default();
        assert_eq!(a.stable_hash(), ClassifierSettings::default().stable_hash());
        let mut b = a.clone();
        b.min_side_px = 100;
        assert_ne!(a.stable_hash(), b.stable_hash());
        let mut c = a.clone();
        c.extra_cache_patterns = vec!["b".into(), "a".into()];
        let mut d = a.clone();
        d.extra_cache_patterns = vec!["a".into(), "b".into()];
        assert_eq!(c.stable_hash(), d.stable_hash(), "order-insensitive");
    }

    #[test]
    fn validation_rejects_bad_values() {
        let mut s = Settings::default();
        s.scanning.preview_cache_gb = -1.0;
        s.ai.base_url = "localhost".into();
        let keys: Vec<_> = s.validate().into_iter().map(|e| e.key).collect();
        assert_eq!(keys, vec!["scanning.preview_cache_gb", "ai.base_url"]);
    }

    #[test]
    fn ids_round_trip() {
        for t in ThemeSetting::ALL {
            assert_eq!(ThemeSetting::from_id(t.id()), Some(t));
        }
        for m in MotionSetting::ALL {
            assert_eq!(MotionSetting::from_id(m.id()), Some(m));
        }
        for p in ProviderId::ALL {
            assert_eq!(ProviderId::from_id(p.id()), Some(p));
        }
        for p in ExportProfile::ALL {
            assert_eq!(ExportProfile::from_id(p.id()), Some(p));
        }
    }
}
