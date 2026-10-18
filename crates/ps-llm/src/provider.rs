//! The `VisionProvider` trait (ADR-0020) and the connection test.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{ProviderConfig, WireFormat};
use crate::error::LlmError;
use crate::preview::PreviewJpeg;
use crate::prompt::Prompt;

/// A boxed, sendable future (keeps the trait object-safe).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The single repair round: the previous answer and what was wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    pub previous_answer: String,
    pub error: String,
}

/// One describe request.
#[derive(Clone, Copy, Debug)]
pub struct DescribeRequest<'a> {
    pub image: &'a PreviewJpeg,
    pub prompt: &'a Prompt,
    pub repair: Option<&'a Repair>,
}

/// A model's raw text answer.
#[derive(Clone, Debug, PartialEq)]
pub struct RawResponse {
    pub text: String,
    /// The model that answered (as the server reports it, else as configured).
    pub model: String,
    /// HTTP requests used, including retries.
    pub attempts: u32,
    pub latency: Duration,
}

/// A model the server offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    /// Whether it accepts images, when the server says.
    pub vision: Option<bool>,
    /// Currently loaded (LM Studio), when the server says.
    pub loaded: Option<bool>,
}

/// A vision-capable model behind one of the three wire formats.
pub trait VisionProvider: Send + Sync {
    /// The provider's display name (provenance, consent prompt).
    fn name(&self) -> &str;

    /// The configured model; empty means "the server's loaded model".
    fn configured_model(&self) -> &str;

    fn wire(&self) -> WireFormat;

    /// Ask for an insight of one image.
    fn describe<'a>(
        &'a self,
        req: DescribeRequest<'a>,
    ) -> BoxFuture<'a, Result<RawResponse, LlmError>>;

    /// The models the server offers. Never sends a photo.
    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, LlmError>>;

    /// "Test connection" (spec "Test connection").
    fn test(&self) -> BoxFuture<'_, TestReport>
    where
        Self: Sized,
    {
        Box::pin(test_connection(self))
    }
}

/// The result of a connection test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestReport {
    pub provider: String,
    pub reachable: bool,
    /// Round trip of the model listing.
    pub latency: Option<Duration>,
    /// Why it is unreachable or failed.
    pub error: Option<String>,
    /// The model that would be used.
    pub model: Option<String>,
    /// `None` when the server could not be asked.
    pub model_found: Option<bool>,
    /// `None` when the server does not say.
    pub accepts_images: Option<bool>,
    pub models: Vec<String>,
}

/// Hard upper bound of a connection test (spec: report within 5 s).
pub const TEST_TIMEOUT: Duration = Duration::from_millis(4_500);

/// Run the connection test: list models (no photo is sent), check the configured model exists and whether it
/// accepts images.
pub async fn test_connection(p: &(impl VisionProvider + ?Sized)) -> TestReport {
    let started = Instant::now();
    let mut report = TestReport {
        provider: p.name().to_owned(),
        reachable: false,
        latency: None,
        error: None,
        model: None,
        model_found: None,
        accepts_images: None,
        models: Vec::new(),
    };
    let models = match tokio::time::timeout(TEST_TIMEOUT, p.list_models()).await {
        Err(_) => {
            report.error = Some(format!(
                "no answer within {:.1} s",
                TEST_TIMEOUT.as_secs_f64()
            ));
            return report;
        }
        Ok(Err(e)) => {
            // An auth or HTTP error means the server is there.
            report.reachable =
                !matches!(e, LlmError::Unreachable { .. } | LlmError::Timeout { .. });
            report.latency = report.reachable.then(|| started.elapsed());
            report.error = Some(e.to_string());
            return report;
        }
        Ok(Ok(m)) => m,
    };
    report.reachable = true;
    report.latency = Some(started.elapsed());
    report.models = models.iter().map(|m| m.id.clone()).collect();
    let wanted = p.configured_model();
    let chosen = if wanted.is_empty() {
        pick_default_model(&models)
    } else {
        models.iter().find(|m| model_matches(&m.id, wanted))
    };
    report.model_found = Some(chosen.is_some());
    report.model = chosen
        .map(|m| m.id.clone())
        .or_else(|| (!wanted.is_empty()).then(|| wanted.to_owned()));
    report.accepts_images = chosen.and_then(|m| m.vision);
    if chosen.is_none() {
        report.error = Some(if wanted.is_empty() {
            "the server offers no model".into()
        } else {
            format!("model \"{wanted}\" is not offered by {}", p.name())
        });
    } else if report.accepts_images == Some(false) {
        report.error = Some(format!(
            "model \"{}\" does not accept images; choose a vision model",
            report.model.clone().unwrap_or_default()
        ));
    }
    report
}

/// `models/gemini-x` matches `gemini-x`.
pub(crate) fn model_matches(id: &str, wanted: &str) -> bool {
    id == wanted || id.strip_prefix("models/") == Some(wanted)
}

/// The loaded vision model, else any loaded model, else any vision model, else the first one.
pub(crate) fn pick_default_model(models: &[ModelInfo]) -> Option<&ModelInfo> {
    models
        .iter()
        .find(|m| m.loaded == Some(true) && m.vision == Some(true))
        .or_else(|| {
            models
                .iter()
                .find(|m| m.loaded == Some(true) && m.vision != Some(false))
        })
        .or_else(|| models.iter().find(|m| m.vision == Some(true)))
        .or_else(|| models.iter().find(|m| m.vision != Some(false)))
}

/// Build the provider for a configuration.
pub fn provider_for(cfg: ProviderConfig) -> Arc<dyn VisionProvider> {
    match cfg.wire {
        WireFormat::OpenAiCompatible => Arc::new(crate::wire::openai::OpenAiCompatible::new(cfg)),
        WireFormat::Anthropic => Arc::new(crate::wire::anthropic::Anthropic::new(cfg)),
        WireFormat::Gemini => Arc::new(crate::wire::gemini::Gemini::new(cfg)),
    }
}
