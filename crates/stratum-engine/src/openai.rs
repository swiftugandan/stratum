//! OpenAI-compatible API client implementing `LlmClient`.
//!
//! Works with any OpenAI-compatible endpoint (Groq, OpenAI, Together, etc.).

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use stratum_core::ports::LlmClient;
use stratum_core::*;
use tracing::{debug, warn};

use crate::error::EngineError;
use crate::llm::RetryConfig;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct OpenAiClient {
    http: Client,
    api_key: String,
    url: String,
    retry: RetryConfig,
    max_tokens: u32,
}

impl OpenAiClient {
    pub fn new(api_key: String, base_url: String) -> Self {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        Self {
            http: Client::new(),
            url,
            api_key,
            retry: RetryConfig::default(),
            max_tokens: 4096,
        }
    }

    /// Convenience constructor for Groq.
    pub fn groq(api_key: String) -> Self {
        Self::new(api_key, "https://api.groq.com/openai/v1".to_string())
    }

    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

impl std::fmt::Debug for OpenAiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiClient")
            .field("url", &self.url)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// OpenAI API shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct OaiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OaiTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OaiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiFunction,
}

#[derive(Debug, Serialize)]
struct OaiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: OaiUsage,
}

#[derive(Debug, Deserialize)]
struct OaiChoice {
    message: OaiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OaiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OaiToolCall {
    id: String,
    function: OaiToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OaiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OaiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OaiPromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct OaiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct OaiErrorEnvelope {
    error: OaiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct OaiErrorDetail {
    message: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 503)
}

fn oai_headers(api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let bearer = format!("Bearer {api_key}");
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&bearer).unwrap());
    headers
}

fn build_request(
    messages: &[LlmMessage],
    model: &str,
    tools: Option<&[ToolDefinition]>,
    max_tokens: u32,
) -> OaiRequest {
    let api_messages: Vec<OaiMessage> = messages
        .iter()
        .map(|m| OaiMessage {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let api_tools = tools.map(|tools| {
        tools
            .iter()
            .map(|t| OaiTool {
                tool_type: "function".to_string(),
                function: OaiFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.schema.clone(),
                },
            })
            .collect()
    });

    OaiRequest {
        model: model.to_string(),
        max_tokens,
        messages: api_messages,
        tools: api_tools,
    }
}

fn parse_response(resp: OaiResponse) -> LlmResponse {
    let choice = resp.choices.into_iter().next();
    let (content, tool_calls, finish_reason) = match choice {
        Some(c) => {
            let content = c.message.content.unwrap_or_default();
            let tool_calls = c
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(|tc| {
                    let arguments: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    LlmToolCall {
                        id: Some(tc.id),
                        tool_name: tc.function.name,
                        arguments,
                    }
                })
                .collect();
            (content, tool_calls, c.finish_reason)
        }
        None => (String::new(), Vec::new(), None),
    };

    let cached = resp
        .usage
        .prompt_tokens_details
        .map(|d| d.cached_tokens)
        .unwrap_or(0);

    LlmResponse {
        content,
        tool_calls,
        usage: LlmUsage {
            input_tokens: resp.usage.prompt_tokens,
            output_tokens: resp.usage.completion_tokens,
            cached_tokens: cached,
        },
        stop_reason: finish_reason,
    }
}

async fn retry_post<T: serde::de::DeserializeOwned>(
    http: &Client,
    url: &str,
    headers: &HeaderMap,
    body: &impl Serialize,
    retry: &RetryConfig,
) -> Result<T, EngineError> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| EngineError::Http(e.to_string()))?;

    let mut last_error = None;

    for attempt in 0..=retry.max_retries {
        if attempt > 0 {
            let delay = retry.delay_for_attempt(attempt - 1);
            debug!(attempt, ?delay, "retrying OpenAI-compatible API call");
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
                warn!(attempt, error = %e, "API request failed");
                last_error = Some(EngineError::Http(e.to_string()));
                continue;
            }
        };

        let status = response.status().as_u16();

        if status == 200 {
            let api_resp: T = response
                .json()
                .await
                .map_err(|e| EngineError::Http(e.to_string()))?;
            return Ok(api_resp);
        }

        if is_retryable_status(status) {
            let error_body = response.text().await.unwrap_or_default();
            warn!(attempt, status, error_body, "retryable API error");
            last_error = Some(EngineError::LlmApi {
                status,
                message: error_body,
            });
            continue;
        }

        let error_body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<OaiErrorEnvelope>(&error_body)
            .map(|e| e.error.message)
            .unwrap_or(error_body);
        return Err(EngineError::LlmApi { status, message });
    }

    Err(last_error.unwrap_or_else(|| EngineError::Http("unknown error".to_string())))
}

#[async_trait]
impl LlmClient for OpenAiClient {
    type Error = EngineError;

    async fn complete(
        &self,
        messages: &[LlmMessage],
        model: &str,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error> {
        let body = build_request(messages, model, tools, self.max_tokens);
        let headers = oai_headers(&self.api_key);
        let api_resp: OaiResponse =
            retry_post(&self.http, &self.url, &headers, &body, &self.retry).await?;
        Ok(parse_response(api_resp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_no_tools() {
        let messages = vec![LlmMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }];
        let req = build_request(&messages, "llama-3.3-70b-versatile", None, 4096);
        assert_eq!(req.model, "llama-3.3-70b-versatile");
        assert_eq!(req.max_tokens, 4096);
        assert_eq!(req.messages.len(), 1);
        assert!(req.tools.is_none());
    }

    #[test]
    fn test_build_request_with_tools() {
        let messages = vec![LlmMessage {
            role: "user".to_string(),
            content: "Use the tool".to_string(),
        }];
        let tools = vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }];
        let req = build_request(&messages, "llama-3.3-70b-versatile", Some(&tools), 4096);
        assert!(req.tools.is_some());
        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_type, "function");
        assert_eq!(tools[0].function.name, "get_weather");
    }

    #[test]
    fn test_build_request_keeps_system_messages_inline() {
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
        let req = build_request(&messages, "llama-3.3-70b-versatile", None, 8192);
        // OpenAI format keeps system messages inline
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
    }

    #[test]
    fn test_parse_text_response() {
        let resp = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiResponseMessage {
                    content: Some("Hello!".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: OaiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                prompt_tokens_details: None,
            },
        };
        let llm_resp = parse_response(resp);
        assert_eq!(llm_resp.content, "Hello!");
        assert!(llm_resp.tool_calls.is_empty());
        assert_eq!(llm_resp.usage.input_tokens, 10);
        assert_eq!(llm_resp.usage.output_tokens, 5);
        assert_eq!(llm_resp.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_tool_call_response() {
        let resp = OaiResponse {
            choices: vec![OaiChoice {
                message: OaiResponseMessage {
                    content: Some("Let me check.".to_string()),
                    tool_calls: Some(vec![OaiToolCall {
                        id: "call_123".to_string(),
                        function: OaiToolCallFunction {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"Paris"}"#.to_string(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: OaiUsage {
                prompt_tokens: 20,
                completion_tokens: 15,
                prompt_tokens_details: Some(OaiPromptTokensDetails { cached_tokens: 5 }),
            },
        };
        let llm_resp = parse_response(resp);
        assert_eq!(llm_resp.content, "Let me check.");
        assert_eq!(llm_resp.tool_calls.len(), 1);
        assert_eq!(llm_resp.tool_calls[0].id.as_deref(), Some("call_123"));
        assert_eq!(llm_resp.tool_calls[0].tool_name, "get_weather");
        assert_eq!(llm_resp.usage.cached_tokens, 5);
    }

    #[test]
    fn test_groq_constructor_sets_url() {
        let client = OpenAiClient::groq("test-key".to_string());
        assert_eq!(client.url, "https://api.groq.com/openai/v1/chat/completions");
    }

    #[test]
    fn test_retryable_status_codes() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
    }
}
