//! Google Gemini: `POST {base}/models/{model}:generateContent` with the key in `x-goog-api-key` (never in the
//! URL, so it cannot leak through error messages), a system instruction and one user turn with one
//! `inline_data` JPEG part and the rules text. JSON via `responseMimeType` + `responseSchema` (the OpenAPI
//! subset Gemini accepts, converted from the insight JSON Schema).

use std::time::Instant;

use serde_json::{Map, Value, json};

use crate::config::{ProviderConfig, StructuredOutput, WireFormat};
use crate::error::LlmError;
use crate::http::HttpClient;
use crate::prompt::repair_message;
use crate::provider::{BoxFuture, DescribeRequest, ModelInfo, RawResponse, VisionProvider};

#[derive(Debug)]
pub struct Gemini {
    cfg: ProviderConfig,
    http: HttpClient,
}

impl Gemini {
    pub fn new(cfg: ProviderConfig) -> Self {
        let http = HttpClient::new(&cfg);
        Self { cfg, http }
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut h = vec![("accept", "application/json".to_owned())];
        if let Some(k) = &self.cfg.api_key {
            h.push(("x-goog-api-key", k.expose().to_owned()));
        }
        h
    }

    fn model_path(&self) -> String {
        let m = self.cfg.model.trim();
        if m.starts_with("models/") {
            m.to_owned()
        } else {
            format!("models/{m}")
        }
    }

    async fn describe_inner(&self, req: DescribeRequest<'_>) -> Result<RawResponse, LlmError> {
        let started = Instant::now();
        let body = request_body(&self.cfg, &req);
        let url = self
            .cfg
            .url(&format!("/{}:generateContent", self.model_path()));
        let reply = self.http.post_json(&url, &self.headers(), &body).await?;
        if let Some(reason) = reply.body["promptFeedback"]["blockReason"].as_str() {
            return Err(LlmError::Protocol {
                provider: self.cfg.name.clone(),
                message: format!("the request was blocked ({reason})"),
            });
        }
        let text = response_text(&reply.body).ok_or_else(|| LlmError::Protocol {
            provider: self.cfg.name.clone(),
            message: format!(
                "no text in `candidates[0].content.parts` (finishReason {})",
                reply.body["candidates"][0]["finishReason"]
                    .as_str()
                    .unwrap_or("unknown")
            ),
        })?;
        Ok(RawResponse {
            text,
            model: reply.body["modelVersion"]
                .as_str()
                .unwrap_or(&self.cfg.model)
                .to_owned(),
            attempts: reply.attempts,
            latency: started.elapsed(),
        })
    }
}

/// The generate-content request body.
pub fn request_body(cfg: &ProviderConfig, req: &DescribeRequest<'_>) -> Value {
    let mut contents = vec![json!({
        "role": "user",
        "parts": [
            { "inline_data": { "mime_type": "image/jpeg", "data": req.image.base64() } },
            { "text": req.prompt.user }
        ]
    })];
    if let Some(r) = req.repair {
        contents.push(json!({ "role": "model", "parts": [{ "text": r.previous_answer }] }));
        contents.push(json!({ "role": "user", "parts": [{ "text": repair_message(&r.error) }] }));
    }
    let mut generation = json!({
        "temperature": 0,
        "maxOutputTokens": cfg.max_output_tokens,
        "responseMimeType": "application/json",
    });
    if cfg.structured != StructuredOutput::PromptOnly {
        generation["responseSchema"] = to_gemini_schema(&req.prompt.schema);
    }
    json!({
        "systemInstruction": { "parts": [{ "text": req.prompt.system }] },
        "contents": contents,
        "generationConfig": generation,
    })
}

/// JSON Schema -> Gemini's OpenAPI-subset `Schema`: upper-case types, `nullable` instead of `["x","null"]`,
/// no `$schema`/`title`/`additionalProperties`.
pub fn to_gemini_schema(s: &Value) -> Value {
    let Some(obj) = s.as_object() else {
        return s.clone();
    };
    let mut out = Map::new();
    match &obj.get("type") {
        Some(Value::String(t)) => {
            out.insert("type".into(), Value::String(t.to_ascii_uppercase()));
        }
        Some(Value::Array(ts)) => {
            if let Some(t) = ts.iter().filter_map(Value::as_str).find(|t| *t != "null") {
                out.insert("type".into(), Value::String(t.to_ascii_uppercase()));
            }
            if ts.iter().any(|t| t == "null") {
                out.insert("nullable".into(), Value::Bool(true));
            }
        }
        _ => {}
    }
    for (k, v) in obj {
        match k.as_str() {
            "type" | "$schema" | "title" | "additionalProperties" => {}
            "properties" => {
                let props: Map<String, Value> = v
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(name, p)| (name.clone(), to_gemini_schema(p)))
                    .collect();
                out.insert("properties".into(), Value::Object(props));
            }
            "items" => {
                out.insert("items".into(), to_gemini_schema(v));
            }
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    // Keep the answer in schema order (Gemini otherwise orders keys alphabetically).
    if let Some(Value::Array(req)) = obj.get("required") {
        out.insert("propertyOrdering".into(), Value::Array(req.clone()));
    }
    Value::Object(out)
}

/// The concatenated non-thought text parts of the first candidate.
pub fn response_text(body: &Value) -> Option<String> {
    let parts = body["candidates"][0]["content"]["parts"].as_array()?;
    let text: Vec<&str> = parts
        .iter()
        .filter(|p| p["thought"] != Value::Bool(true))
        .filter_map(|p| p["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| text.join(""))
}

impl VisionProvider for Gemini {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn configured_model(&self) -> &str {
        &self.cfg.model
    }

    fn wire(&self) -> WireFormat {
        WireFormat::Gemini
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
                .get_json(&self.cfg.url("/models?pageSize=1000"), &self.headers())
                .await?;
            let models = reply.body["models"]
                .as_array()
                .ok_or_else(|| LlmError::Protocol {
                    provider: self.cfg.name.clone(),
                    message: "model list has no `models` array".into(),
                })?;
            Ok(models
                .iter()
                .filter(|m| {
                    m["supportedGenerationMethods"]
                        .as_array()
                        .is_none_or(|a| a.iter().any(|x| x == "generateContent"))
                })
                .filter_map(|m| m["name"].as_str())
                .map(|name| ModelInfo {
                    id: name.strip_prefix("models/").unwrap_or(name).to_owned(),
                    // Gemini generateContent models are multimodal.
                    vision: Some(true),
                    loaded: None,
                })
                .collect())
        })
    }
}
