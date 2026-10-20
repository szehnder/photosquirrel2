//! JSON over HTTP with timeouts, retries and backoff (ADR-0020). Built on `reqwest` with rustls; runs inside
//! the `ps-llm` tokio runtime ([`crate::runtime`]).

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::{ProviderConfig, RetryPolicy};
use crate::error::{LlmError, excerpt, redact};

/// A JSON answer and how many HTTP requests it took (1 + retries).
#[derive(Debug, Clone)]
pub struct JsonReply {
    pub body: Value,
    pub attempts: u32,
    /// Time of the successful request only.
    pub latency: Duration,
}

/// A provider's HTTP client.
#[derive(Clone, Debug)]
pub struct HttpClient {
    inner: reqwest::Client,
    provider: String,
    secret: Option<String>,
    retry: RetryPolicy,
    request_timeout: Duration,
}

impl HttpClient {
    /// The client for `cfg`: its timeouts, no redirects. Fails when the client cannot be built; there is no
    /// fallback (a default `reqwest::Client` follows redirects and has no timeouts).
    pub fn new(cfg: &ProviderConfig) -> Result<Self, LlmError> {
        Self::from_builder(cfg, Self::builder(cfg))
    }

    fn builder(cfg: &ProviderConfig) -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .connect_timeout(cfg.timeouts.connect)
            .timeout(cfg.timeouts.request)
            .user_agent(concat!("PhotoSquirrel/", env!("CARGO_PKG_VERSION")))
            // Never follow redirects: consent covers one host (ADR-0047), and a local server must not be able
            // to send previews elsewhere by answering 3xx.
            .redirect(reqwest::redirect::Policy::none())
    }

    fn from_builder(
        cfg: &ProviderConfig,
        builder: reqwest::ClientBuilder,
    ) -> Result<Self, LlmError> {
        let inner = builder.build().map_err(|e| LlmError::Client {
            provider: cfg.name.clone(),
            message: e.to_string(),
        })?;
        Ok(Self {
            inner,
            provider: cfg.name.clone(),
            secret: cfg.api_key.as_ref().map(|k| k.expose().to_owned()),
            retry: cfg.retry.clone(),
            request_timeout: cfg.timeouts.request,
        })
    }

    /// The same client without retries (connection tests).
    pub fn without_retries(&self) -> Self {
        Self {
            retry: RetryPolicy::none(),
            ..self.clone()
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// `POST url` with a JSON body, retrying per the policy.
    pub async fn post_json(
        &self,
        url: &str,
        headers: &[(&'static str, String)],
        body: &Value,
    ) -> Result<JsonReply, LlmError> {
        let bytes = serde_json::to_vec(body).map_err(|e| LlmError::Protocol {
            provider: self.provider.clone(),
            message: e.to_string(),
        })?;
        self.send(reqwest::Method::POST, url, headers, Some(bytes))
            .await
    }

    /// `GET url`, retrying per the policy.
    pub async fn get_json(
        &self,
        url: &str,
        headers: &[(&'static str, String)],
    ) -> Result<JsonReply, LlmError> {
        self.send(reqwest::Method::GET, url, headers, None).await
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        headers: &[(&'static str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<JsonReply, LlmError> {
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let started = Instant::now();
            let result = self.once(method.clone(), url, headers, body.clone()).await;
            let err = match result {
                Ok(body) => {
                    return Ok(JsonReply {
                        body,
                        attempts,
                        latency: started.elapsed(),
                    });
                }
                Err(e) => e,
            };
            let retry = attempts; // retry number about to happen
            if !err.is_retryable() || retry > self.retry.max_retries {
                return Err(err);
            }
            let delay = match &err {
                LlmError::RateLimited {
                    retry_after: Some(d),
                    ..
                } => (*d).min(self.retry.max_retry_after),
                LlmError::RateLimited { .. } => self.retry.rate_limit_delay(retry),
                _ => self.retry.server_delay(retry),
            };
            log::info!(
                "{}: {err}; retry {retry}/{} in {:.1} s",
                self.provider,
                self.retry.max_retries,
                delay.as_secs_f64()
            );
            tokio::time::sleep(delay).await;
        }
    }

    async fn once(
        &self,
        method: reqwest::Method,
        url: &str,
        headers: &[(&'static str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<Value, LlmError> {
        let mut req = self.inner.request(method, url);
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        if let Some(b) = body {
            req = req.header("content-type", "application/json").body(b);
        }
        let resp = req.send().await.map_err(|e| self.map_send_error(&e))?;
        let status = resp.status().as_u16();
        let retry_after = retry_after(resp.headers());
        let text = resp.text().await.map_err(|e| self.map_send_error(&e))?;
        let provider = self.provider.clone();
        match status {
            200..=299 => serde_json::from_str(&text).map_err(|e| LlmError::Protocol {
                provider,
                message: format!("response is not JSON ({e})"),
            }),
            429 => Err(LlmError::RateLimited {
                provider,
                retry_after,
            }),
            401 | 403 => Err(LlmError::Auth { provider, status }),
            _ => Err(LlmError::Http {
                provider,
                status,
                message: redact(&excerpt(&text), self.secret.as_deref()),
            }),
        }
    }

    fn map_send_error(&self, e: &reqwest::Error) -> LlmError {
        let provider = self.provider.clone();
        if e.is_timeout() {
            return LlmError::Timeout {
                provider,
                after: self.request_timeout,
            };
        }
        // The full error chain carries the useful part ("Connection refused").
        let mut reason = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            reason = format!("{reason}: {s}");
            src = s.source();
        }
        let reason = redact(&reason, self.secret.as_deref());
        if e.is_connect() || e.is_request() {
            LlmError::Unreachable { provider, reason }
        } else {
            LlmError::Protocol {
                provider,
                message: reason,
            }
        }
    }
}

/// `retry-after-ms` (OpenAI) or `retry-after` in seconds. HTTP-date values fall back to backoff.
fn retry_after(h: &reqwest::header::HeaderMap) -> Option<Duration> {
    let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(ms) = get("retry-after-ms").and_then(|v| v.parse::<f64>().ok())
        && ms.is_finite()
        && ms >= 0.0
    {
        return Some(Duration::from_secs_f64(ms / 1000.0));
    }
    get("retry-after")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// N4: a client that cannot be built is an error, never a silent default client (which would follow
    /// redirects and have no timeouts).
    #[test]
    fn a_failing_builder_is_an_error_not_a_default_client() {
        let cfg = ProviderConfig::default_local();
        let bad = HttpClient::builder(&cfg).user_agent("bad\nagent");
        let err = HttpClient::from_builder(&cfg, bad).expect_err("invalid header value");
        assert!(matches!(err, LlmError::Client { .. }), "{err:?}");
        assert!(!err.is_retryable());
        assert!(HttpClient::new(&cfg).is_ok());
    }
}
