//! Shared retry logic, error types, and helpers for LLM clients.

use rand::Rng;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::time::Duration;
use stratum_types::LlmMessage;
use tracing::{debug, warn};

use crate::error::AdapterError;

/// Configuration for retry with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryConfig {
    /// Compute the delay for a given attempt (0-indexed) with full jitter.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_ms = self.base_delay.as_millis() as u64;
        let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(10));
        let capped_ms = exp_ms.min(self.max_delay.as_millis() as u64);
        let jitter_ms = rand::thread_rng().gen_range(0..=capped_ms);
        Duration::from_millis(jitter_ms)
    }
}

/// Returns true if the HTTP status code is retryable (429, 500, 503, 529).
/// 529 is Anthropic's "overloaded" status.
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 503 | 529)
}

/// Common error response envelope used by Anthropic, OpenAI, and other providers.
/// Shape: `{ "error": { "message": "..." } }`
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ApiErrorEnvelope {
    pub error: ApiErrorDetail,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ApiErrorDetail {
    pub message: String,
}

/// POST a JSON body to `url` with retries, returning the raw response bytes on success.
///
/// Handles transient failures (network errors, retryable HTTP status codes) with
/// exponential backoff. On non-retryable errors, attempts to extract the error
/// message from the standard `{ "error": { "message": "..." } }` envelope.
pub(crate) async fn retry_post<T: DeserializeOwned>(
    http: &Client,
    url: &str,
    headers: &HeaderMap,
    body: &impl Serialize,
    retry: &RetryConfig,
    provider: &str,
) -> Result<T, AdapterError> {
    // Serialize body once before the retry loop.
    let body_bytes = serde_json::to_vec(body).map_err(|e| AdapterError::Http(e.to_string()))?;

    let mut last_error = None;

    for attempt in 0..=retry.max_retries {
        if attempt > 0 {
            let delay = retry.delay_for_attempt(attempt - 1);
            debug!(attempt, ?delay, provider, "retrying API call");
            tokio::time::sleep(delay).await;
        }

        let result = http
            .post(url)
            .headers(headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body_bytes.clone())
            .send()
            .await;

        let response = match result {
            Ok(r) => r,
            Err(e) => {
                warn!(attempt, error = %e, provider, "API request failed");
                last_error = Some(AdapterError::Http(e.to_string()));
                continue;
            }
        };

        let status = response.status().as_u16();

        if status == 200 {
            let api_resp: T = response
                .json()
                .await
                .map_err(|e| AdapterError::Http(e.to_string()))?;
            return Ok(api_resp);
        }

        if is_retryable_status(status) {
            let error_body = response.text().await.unwrap_or_default();
            warn!(attempt, status, error_body, provider, "retryable API error");
            last_error = Some(AdapterError::LlmApi {
                status,
                message: error_body,
            });
            continue;
        }

        // Non-retryable error — extract message from standard envelope
        let error_body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ApiErrorEnvelope>(&error_body)
            .map(|e| e.error.message)
            .unwrap_or(error_body);
        return Err(AdapterError::LlmApi { status, message });
    }

    Err(last_error.unwrap_or_else(|| AdapterError::Http("unknown error".to_string())))
}

/// Extract system messages from a message list, returning `(system_content, non_system_messages)`.
///
/// Used by Anthropic (top-level `system` field) and OpenAI Responses API (`instructions` field).
/// If multiple system messages exist, the last one wins.
pub(crate) fn extract_system_message(
    messages: &[LlmMessage],
) -> (Option<String>, Vec<&LlmMessage>) {
    let mut system = None;
    let rest: Vec<&LlmMessage> = messages
        .iter()
        .filter(|m| {
            if m.role == "system" {
                system = Some(m.content.clone());
                false
            } else {
                true
            }
        })
        .collect();
    (system, rest)
}

/// Build a `HeaderMap` for Bearer token authentication (OpenAI-style).
pub(crate) fn bearer_headers(api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let val = format!("Bearer {api_key}");
    headers.insert("Authorization", HeaderValue::from_str(&val).unwrap());
    headers
}

/// Build a `HeaderMap` for Anthropic authentication.
pub(crate) fn anthropic_headers(api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_str(api_key).unwrap());
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_retry_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay, Duration::from_millis(500));
    }

    #[test]
    fn test_delay_is_bounded() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
        };
        for attempt in 0..10 {
            let delay = config.delay_for_attempt(attempt);
            assert!(delay <= config.max_delay);
        }
    }

    #[test]
    fn test_retryable_status_codes() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(529));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
    }

    #[test]
    fn test_extract_system_message() {
        let messages = vec![
            LlmMessage {
                role: "system".to_string(),
                content: "You are helpful.".to_string(),
            },
            LlmMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
        ];
        let (system, rest) = extract_system_message(&messages);
        assert_eq!(system.as_deref(), Some("You are helpful."));
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].role, "user");
    }

    #[test]
    fn test_extract_system_message_none() {
        let messages = vec![LlmMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }];
        let (system, rest) = extract_system_message(&messages);
        assert!(system.is_none());
        assert_eq!(rest.len(), 1);
    }
}
