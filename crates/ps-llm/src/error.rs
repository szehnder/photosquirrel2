//! Errors of the provider layer. Messages are shown to the user and written to logs, so they never contain an
//! API key (see [`redact`]).

use std::time::Duration;

/// One failed provider interaction.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LlmError {
    /// Nothing answered (connection refused, DNS failure, TLS failure).
    #[error("cannot reach {provider}: {reason}")]
    Unreachable { provider: String, reason: String },
    #[error("{provider} did not answer within {} s", .after.as_secs())]
    Timeout { provider: String, after: Duration },
    /// HTTP 429 after the retries were used up.
    #[error("{provider} is rate-limiting requests (HTTP 429)")]
    RateLimited {
        provider: String,
        retry_after: Option<Duration>,
    },
    /// HTTP 401/403.
    #[error("{provider} rejected the API key (HTTP {status})")]
    Auth { provider: String, status: u16 },
    /// Any other non-success status.
    #[error("{provider} answered HTTP {status}: {message}")]
    Http {
        provider: String,
        status: u16,
        message: String,
    },
    /// A response that is not the provider's documented shape.
    #[error("unexpected answer from {provider}: {message}")]
    Protocol { provider: String, message: String },
    /// The HTTP client could not be set up (TLS backend, configuration). Nothing was sent: there is no fallback
    /// client, because a default one would follow redirects and have no timeouts (ADR-0047).
    #[error("cannot set up the HTTP client for {provider}: {message}")]
    Client { provider: String, message: String },
    /// The model's answer is not a valid `ImageInsight` v1 (after the repair round).
    #[error("invalid insight: {0}")]
    Invalid(String),
}

impl LlmError {
    /// Worth retrying with backoff (ADR-0020 retry policy): timeouts, unreachable, 429, 408 and 5xx.
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Unreachable { .. }
            | LlmError::Timeout { .. }
            | LlmError::RateLimited { .. } => true,
            LlmError::Http { status, .. } => *status >= 500 || *status == 408,
            LlmError::Auth { .. }
            | LlmError::Protocol { .. }
            | LlmError::Client { .. }
            | LlmError::Invalid(_) => false,
        }
    }
}

/// Replace every occurrence of `secret` in `text` (no-op for empty secrets).
pub fn redact(text: &str, secret: Option<&str>) -> String {
    match secret {
        Some(s) if s.len() >= 4 => text.replace(s, "<redacted>"),
        _ => text.to_owned(),
    }
}

/// Shorten a response body for an error message.
pub(crate) fn excerpt(body: &str) -> String {
    let body = body.trim();
    // Prefer the provider's own error message when the body is a JSON error object.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let msg = v["error"]["message"]
            .as_str()
            .or_else(|| v["error"].as_str())
            .or_else(|| v["message"].as_str());
        if let Some(m) = msg {
            return truncate(m, 300);
        }
    }
    truncate(body, 300)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_and_excerpts() {
        assert_eq!(
            redact("key sk-abcdef here", Some("sk-abcdef")),
            "key <redacted> here"
        );
        assert_eq!(redact("x", None), "x");
        assert_eq!(excerpt(r#"{"error":{"message":"bad model"}}"#), "bad model");
        assert_eq!(excerpt(&"a".repeat(400)).chars().count(), 301);
    }
}
