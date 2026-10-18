//! Anthropic Messages: `POST {base}/messages` with `x-api-key` and `anthropic-version`, a system prompt and one
//! user turn holding one base64 image block and the rules text. JSON is requested through the prompt (no tools,
//! no prefill).

use std::time::Instant;

use serde_json::{Value, json};

use crate::config::{ProviderConfig, WireFormat};
use crate::error::LlmError;
use crate::http::HttpClient;
use crate::prompt::repair_message;
use crate::provider::{BoxFuture, DescribeRequest, ModelInfo, RawResponse, VisionProvider};

/// The Messages API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug)]
pub struct Anthropic {
    cfg: ProviderConfig,
    http: HttpClient,
}

impl Anthropic {
    pub fn new(cfg: ProviderConfig) -> Self {
        let http = HttpClient::new(&cfg);
        Self { cfg, http }
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut h = vec![
            ("accept", "application/json".to_owned()),
            ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
        ];
        if let Some(k) = &self.cfg.api_key {
            h.push(("x-api-key", k.expose().to_owned()));
        }
        h
    }

    async fn describe_inner(&self, req: DescribeRequest<'_>) -> Result<RawResponse, LlmError> {
        let started = Instant::now();
        let body = request_body(&self.cfg, &req);
        let reply = self
            .http
            .post_json(&self.cfg.url("/messages"), &self.headers(), &body)
            .await?;
        let text = response_text(&reply.body).ok_or_else(|| LlmError::Protocol {
            provider: self.cfg.name.clone(),
            message: "no text block in `content`".into(),
        })?;
        Ok(RawResponse {
            text,
            model: reply.body["model"]
                .as_str()
                .unwrap_or(&self.cfg.model)
                .to_owned(),
            attempts: reply.attempts,
            latency: started.elapsed(),
        })
    }
}

/// The Messages request body.
pub fn request_body(cfg: &ProviderConfig, req: &DescribeRequest<'_>) -> Value {
    let mut messages = vec![json!({
        "role": "user",
        "content": [
            {
                "type": "image",
                "source": { "type": "base64", "media_type": "image/jpeg", "data": req.image.base64() }
            },
            { "type": "text", "text": req.prompt.user }
        ]
    })];
    if let Some(r) = req.repair {
        messages.push(json!({ "role": "assistant", "content": r.previous_answer }));
        messages.push(json!({ "role": "user", "content": repair_message(&r.error) }));
    }
    json!({
        "model": cfg.model,
        "max_tokens": cfg.max_output_tokens,
        "temperature": 0,
        "system": req.prompt.system,
        "messages": messages,
    })
}

/// The concatenated `text` blocks of `content`.
pub fn response_text(body: &Value) -> Option<String> {
    let blocks = body["content"].as_array()?;
    let text: Vec<&str> = blocks
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| text.join(""))
}

impl VisionProvider for Anthropic {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn configured_model(&self) -> &str {
        &self.cfg.model
    }

    fn wire(&self) -> WireFormat {
        WireFormat::Anthropic
    }

    fn describe<'a>(
        &'a self,
        req: DescribeRequest<'a>,
    ) -> BoxFuture<'a, Result<RawResponse, LlmError>> {
        Box::pin(self.describe_inner(req))
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, LlmError>> {
        Box::pin(async move {
            let reply = self
                .http
                .without_retries()
                .get_json(&self.cfg.url("/models?limit=1000"), &self.headers())
                .await?;
            let data = reply.body["data"]
                .as_array()
                .ok_or_else(|| LlmError::Protocol {
                    provider: self.cfg.name.clone(),
                    message: "model list has no `data` array".into(),
                })?;
            // Every current Claude model accepts images.
            Ok(data
                .iter()
                .filter_map(|m| m["id"].as_str())
                .map(|id| ModelInfo {
                    id: id.to_owned(),
                    vision: Some(true),
                    loaded: None,
                })
                .collect())
        })
    }
}
