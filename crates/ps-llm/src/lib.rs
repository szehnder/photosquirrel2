//! Vision LLM providers and the AI insight schema (ADR-0020, ADR-0021).
//!
//! - [`provider::VisionProvider`]: one trait, three wire formats in [`wire`]: OpenAI-compatible chat
//!   completions (LM Studio default at `http://localhost:1234/v1`, Ollama, OpenAI, OpenRouter, Groq),
//!   Anthropic Messages and Google Gemini generate-content. Presets in [`config::PRESETS`].
//! - [`prompt`]: the versioned insight prompt; [`validate`]: parsing an answer as `ImageInsight` v1;
//!   [`describe::Describer`]: request + validation + one repair round.
//! - [`preview`]: the 1024 px JPEG that is the only thing ever uploaded (ADR-0036).
//! - [`http`]: timeouts (connect 5 s; request 120 s hosted, 300 s local), retries with backoff, `retry-after`.
//! - [`consent`]: hosted providers need explicit consent naming the provider (ADR-0021).
//! - [`queue`]: the AI queue engine (concurrency, rate limit, start/pause, progress) over a [`queue::QueueStore`].
//! - [`runtime`]: the tokio runtime owned by this crate (ADR-0009).
//!
//! Keys are passed in as [`config::ApiKey`] (looked up by `ps-service::secrets`, ADR-0031); they are sent only
//! in request headers and never logged or put in error messages.

pub mod config;
pub mod consent;
pub mod describe;
pub mod error;
pub mod http;
pub mod limits;
pub mod preview;
pub mod prompt;
pub mod provider;
pub mod queue;
pub mod runtime;
#[cfg(feature = "testing")]
pub mod testing;
pub mod validate;
pub mod wire;

pub use config::{
    ApiKey, PRESETS, Preset, ProviderConfig, RetryPolicy, StructuredOutput, Timeouts, WireFormat,
};
pub use describe::{DescribeFailure, Described, Describer, Provenance};
pub use error::LlmError;
pub use preview::{PreviewError, PreviewInput, PreviewJpeg};
pub use prompt::{PROMPT_VERSION, Prompt};
pub use provider::{
    ModelInfo, RawResponse, TestReport, VisionProvider, provider_for, test_connection,
};
pub use runtime::LlmRuntime;
