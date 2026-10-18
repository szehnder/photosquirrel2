//! Describe one preview: request, validate, and at most one repair round (spec "Versioned insight schema").

use std::sync::Arc;
use std::time::{Duration, Instant};

use ps_model::ImageInsight;

use crate::error::LlmError;
use crate::preview::PreviewJpeg;
use crate::prompt::Prompt;
use crate::provider::{DescribeRequest, Repair, VisionProvider};
use crate::validate::parse_insight;

/// Where an insight came from (spec "Provenance stored").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    pub provider: String,
    pub model: String,
    pub schema_version: u32,
    pub prompt_version: u32,
    /// Wall time of the whole describe (all requests).
    pub latency: Duration,
    /// HTTP requests used, including retries and the repair round.
    pub requests: u32,
}

/// A validated insight and its provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct Described {
    pub insight: ImageInsight,
    pub provenance: Provenance,
    pub repaired: bool,
}

/// A failed describe and how many requests it used.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{error}")]
pub struct DescribeFailure {
    pub error: LlmError,
    pub requests: u32,
}

/// A provider plus the prompt.
#[derive(Clone)]
pub struct Describer {
    provider: Arc<dyn VisionProvider>,
    prompt: Prompt,
}

impl std::fmt::Debug for Describer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Describer")
            .field("provider", &self.provider.name())
            .field("prompt_version", &self.prompt.version)
            .finish()
    }
}

impl Describer {
    pub fn new(provider: Arc<dyn VisionProvider>) -> Self {
        Self::with_prompt(provider, Prompt::insight_v1())
    }

    pub fn with_prompt(provider: Arc<dyn VisionProvider>, prompt: Prompt) -> Self {
        Self { provider, prompt }
    }

    pub fn provider(&self) -> &Arc<dyn VisionProvider> {
        &self.provider
    }

    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// Request an insight; on an invalid answer send exactly one repair request carrying the validation error;
    /// a second invalid answer fails with that error.
    pub async fn describe(&self, image: &PreviewJpeg) -> Result<Described, DescribeFailure> {
        let started = Instant::now();
        let mut requests = 0;
        let first = self
            .provider
            .describe(DescribeRequest {
                image,
                prompt: &self.prompt,
                repair: None,
            })
            .await
            .map_err(|error| DescribeFailure {
                // A lower bound: retries inside the HTTP layer are logged, not counted, on failure.
                error,
                requests: requests + 1,
            })?;
        requests += first.attempts;
        let error = match parse_insight(&first.text) {
            Ok(insight) => return Ok(self.done(insight, first.model, started, requests, false)),
            Err(e) => e,
        };
        log::info!(
            "{}: invalid insight ({error}); sending one repair request",
            self.provider.name()
        );
        let repair = Repair {
            previous_answer: first.text,
            error: error.0.clone(),
        };
        let second = self
            .provider
            .describe(DescribeRequest {
                image,
                prompt: &self.prompt,
                repair: Some(&repair),
            })
            .await
            .map_err(|error| DescribeFailure {
                error,
                requests: requests + 1,
            })?;
        requests += second.attempts;
        match parse_insight(&second.text) {
            Ok(insight) => Ok(self.done(insight, second.model, started, requests, true)),
            Err(e) => Err(DescribeFailure {
                error: LlmError::Invalid(e.0),
                requests,
            }),
        }
    }

    fn done(
        &self,
        insight: ImageInsight,
        model: String,
        started: Instant,
        requests: u32,
        repaired: bool,
    ) -> Described {
        Described {
            insight,
            provenance: Provenance {
                provider: self.provider.name().to_owned(),
                model: if model.is_empty() {
                    self.provider.configured_model().to_owned()
                } else {
                    model
                },
                schema_version: self.prompt.schema_version,
                prompt_version: self.prompt.version,
                latency: started.elapsed(),
                requests,
            },
            repaired,
        }
    }
}
