//! OpenAI-compatible chat completions: LM Studio (default), Ollama, OpenAI, OpenRouter, Groq, llama.cpp, vLLM.
//!
//! `POST {base}/chat/completions` with a system message and one user message holding the rules text and one
//! `image_url` part (a `data:image/jpeg;base64,...` URL). Structured output via `response_format`
//! (`json_schema` strict, or `json_object`); a server that rejects `response_format` with HTTP 400 is retried
//! once with the schema in the prompt only, and remembered for later requests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::config::{ProviderConfig, StructuredOutput, WireFormat};
use crate::error::LlmError;
use crate::http::HttpClient;
use crate::prompt::repair_message;
use crate::provider::{
    BoxFuture, DescribeRequest, ModelInfo, RawResponse, VisionProvider, pick_default_model,
};

#[derive(Debug)]
pub struct OpenAiCompatible {
    cfg: ProviderConfig,
    http: HttpClient,
    /// The server rejected `response_format`: send the schema in the prompt only.
    downgraded: AtomicBool,
    /// Model resolved from the server when none is configured.
    resolved: OnceCell<String>,
}

impl OpenAiCompatible {
    pub fn new(cfg: ProviderConfig) -> Result<Self, LlmError> {
        let http = HttpClient::new(&cfg)?;
        Ok(Self {
            cfg,
            http,
            downgraded: AtomicBool::new(false),
            resolved: OnceCell::new(),
        })
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut h = vec![("accept", "application/json".to_owned())];
        if let Some(k) = &self.cfg.api_key {
            h.push(("authorization", format!("Bearer {}", k.expose())));
        }
        h
    }

    async fn model(&self) -> Result<String, LlmError> {
        if !self.cfg.model.is_empty() {
            return Ok(self.cfg.model.clone());
        }
        self.resolved
            .get_or_try_init(|| async {
                let models = self.models(&self.http).await?;
                pick_default_model(&models)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| LlmError::Protocol {
                        provider: self.cfg.name.clone(),
                        message: "no model is loaded; load a vision model or set one in settings"
                            .into(),
                    })
            })
            .await
            .cloned()
    }

    /// LM Studio's richer listing (`/api/v0/models`: type `vlm`, state `loaded`), else `/v1/models`.
    async fn models(&self, http: &HttpClient) -> Result<Vec<ModelInfo>, LlmError> {
        let base = self.cfg.base_url.trim_end_matches('/');
        if let Some(root) = base.strip_suffix("/v1")
            && let Ok(reply) = http
                .without_retries()
                .get_json(&format!("{root}/api/v0/models"), &self.headers())
                .await
            && let Some(models) = parse_lmstudio_models(&reply.body)
        {
            return Ok(models);
        }
        let reply = http
            .get_json(&self.cfg.url("/models"), &self.headers())
            .await?;
        parse_models(&reply.body).ok_or_else(|| LlmError::Protocol {
            provider: self.cfg.name.clone(),
            message: "model list has no `data` array".into(),
        })
    }

    async fn describe_inner(&self, req: DescribeRequest<'_>) -> Result<RawResponse, LlmError> {
        let started = Instant::now();
        let model = self.model().await?;
        let structured = if self.downgraded.load(Ordering::Relaxed) {
            StructuredOutput::PromptOnly
        } else {
            self.cfg.structured
        };
        let body = request_body(&self.cfg, &model, &req, structured);
        let url = self.cfg.url("/chat/completions");
        let reply = match self.http.post_json(&url, &self.headers(), &body).await {
            Err(LlmError::Http {
                status: 400,
                message,
                ..
            }) if structured != StructuredOutput::PromptOnly
                && mentions_response_format(&message) =>
            {
                log::info!(
                    "{}: server rejected response_format ({message}); using the prompt-only schema",
                    self.cfg.name
                );
                self.downgraded.store(true, Ordering::Relaxed);
                let body = request_body(&self.cfg, &model, &req, StructuredOutput::PromptOnly);
                let mut r = self.http.post_json(&url, &self.headers(), &body).await?;
                r.attempts += 1;
                r
            }
            other => other?,
        };
        let text = response_text(&reply.body).ok_or_else(|| LlmError::Protocol {
            provider: self.cfg.name.clone(),
            message: "no `choices[0].message.content` in the answer".into(),
        })?;
        Ok(RawResponse {
            text,
            model: reply.body["model"].as_str().unwrap_or(&model).to_owned(),
            attempts: reply.attempts,
            latency: started.elapsed(),
        })
    }
}

fn mentions_response_format(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("response_format") || m.contains("json_schema") || m.contains("structured")
}

/// The chat-completions request body.
pub fn request_body(
    cfg: &ProviderConfig,
    model: &str,
    req: &DescribeRequest<'_>,
    structured: StructuredOutput,
) -> Value {
    let user = json!({
        "role": "user",
        "content": [
            { "type": "text", "text": req.prompt.user },
            { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", req.image.base64()) } }
        ]
    });
    let mut messages = vec![
        json!({ "role": "system", "content": req.prompt.system }),
        user,
    ];
    if let Some(r) = req.repair {
        messages.push(json!({ "role": "assistant", "content": r.previous_answer }));
        messages.push(json!({ "role": "user", "content": repair_message(&r.error) }));
    }
    let mut body = json!({
        "model": model,
        "messages": messages,
        "temperature": 0,
        "max_tokens": cfg.max_output_tokens,
        "stream": false,
    });
    match structured {
        StructuredOutput::JsonSchema => {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": "image_insight", "strict": true, "schema": strip_meta(&req.prompt.schema) }
            });
        }
        StructuredOutput::JsonObject => body["response_format"] = json!({ "type": "json_object" }),
        StructuredOutput::PromptOnly => {}
    }
    body
}

/// The schema without `$schema`/`title`, which some servers reject inside `response_format`.
pub(crate) fn strip_meta(schema: &Value) -> Value {
    let mut s = schema.clone();
    if let Some(o) = s.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
    }
    s
}

/// `choices[0].message.content` as text (string, or an array of text parts).
pub fn response_text(body: &Value) -> Option<String> {
    let content = &body["choices"][0]["message"]["content"];
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

fn parse_models(body: &Value) -> Option<Vec<ModelInfo>> {
    Some(
        body["data"]
            .as_array()?
            .iter()
            .filter_map(|m| m["id"].as_str())
            .map(|id| ModelInfo {
                id: id.to_owned(),
                vision: None,
                loaded: None,
            })
            .collect(),
    )
}

fn parse_lmstudio_models(body: &Value) -> Option<Vec<ModelInfo>> {
    let data = body["data"].as_array()?;
    // Only LM Studio's listing carries `type`/`state`.
    if !data.iter().any(|m| m.get("state").is_some()) {
        return None;
    }
    Some(
        data.iter()
            .filter(|m| m["type"].as_str() != Some("embeddings"))
            .filter_map(|m| {
                Some(ModelInfo {
                    id: m["id"].as_str()?.to_owned(),
                    vision: m["type"].as_str().map(|t| t == "vlm"),
                    loaded: m["state"].as_str().map(|s| s == "loaded"),
                })
            })
            .collect(),
    )
}

impl VisionProvider for OpenAiCompatible {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn configured_model(&self) -> &str {
        &self.cfg.model
    }

    fn wire(&self) -> WireFormat {
        WireFormat::OpenAiCompatible
    }

    fn describe<'a>(
        &'a self,
        req: DescribeRequest<'a>,
    ) -> BoxFuture<'a, Result<RawResponse, LlmError>> {
        Box::pin(self.describe_inner(req))
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, LlmError>> {
        Box::pin(async move { self.models(&self.http.without_retries()).await })
    }
}
