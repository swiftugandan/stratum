//! OpenAI Responses API client implementing `LlmClient`.
//!
//! Targets the `POST /v1/responses` endpoint introduced as the successor to
//! Chat Completions. For OpenAI-compatible providers that still use Chat
//! Completions, see [`super::openai::OpenAiChatClient`].

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use stratum_core::LlmClient;
use stratum_types::*;

use crate::error::AdapterError;

use super::retry::{bearer_headers, extract_system_message, retry_post, RetryConfig};

/// OpenAI Responses API client.
pub struct OpenAiResponsesClient {
    http: Client,
    api_key: String,
    url: String,
    retry: RetryConfig,
    max_output_tokens: Option<u32>,
}

impl OpenAiResponsesClient {
    pub fn new(api_key: String) -> Self {
        Self {
            http: Client::new(),
            url: "https://api.openai.com/v1/responses".to_string(),
            api_key,
            retry: RetryConfig::default(),
            max_output_tokens: None,
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.url = format!("{base_url}/v1/responses");
        self
    }

    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }
}

// -- Responses API request/response shapes --

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
}

#[derive(Debug, Serialize)]
struct ResponsesInputItem {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesTool {
    Function {
        name: String,
        description: String,
        parameters: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    status: String,
    output: Vec<ResponsesOutputItem>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
    #[serde(default)]
    error: Option<ResponsesError>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesOutputItem {
    Message(ResponsesOutputMessage),
    FunctionCall(ResponsesFunctionCall),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct ResponsesOutputMessage {
    content: Vec<ResponsesOutputContent>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesOutputContent {
    OutputText {
        text: String,
    },
    Refusal {
        refusal: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct ResponsesFunctionCall {
    #[serde(default)]
    call_id: Option<String>,
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    total_tokens: u64,
}

/// Error object inside a 200-status response with `status: "failed"`.
#[derive(Debug, Deserialize)]
struct ResponsesError {
    message: String,
}

fn build_request(
    messages: &[LlmMessage],
    model: &str,
    tools: Option<&[ToolDefinition]>,
    max_output_tokens: Option<u32>,
) -> ResponsesRequest {
    let (instructions, rest) = extract_system_message(messages);

    let input: Vec<ResponsesInputItem> = rest
        .into_iter()
        .map(|m| ResponsesInputItem {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let api_tools = tools.map(|tools| {
        tools
            .iter()
            .map(|t| ResponsesTool::Function {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.schema.clone(),
            })
            .collect()
    });

    ResponsesRequest {
        model: model.to_string(),
        input,
        instructions,
        max_output_tokens,
        tools: api_tools,
    }
}

fn parse_response(resp: ResponsesResponse) -> Result<LlmResponse, AdapterError> {
    let stop_reason = match resp.status.as_str() {
        "completed" => Some("completed".to_string()),
        "failed" => {
            let msg = resp
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(AdapterError::LlmApi {
                status: 200,
                message: msg,
            });
        }
        other => Some(other.to_string()),
    };

    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for item in resp.output {
        match item {
            ResponsesOutputItem::Message(msg) => {
                for block in msg.content {
                    match block {
                        ResponsesOutputContent::OutputText { text } => {
                            content.push_str(&text);
                        }
                        ResponsesOutputContent::Refusal { refusal } => {
                            content.push_str(&refusal);
                        }
                        ResponsesOutputContent::Unknown => {}
                    }
                }
            }
            ResponsesOutputItem::FunctionCall(fc) => {
                let arguments: serde_json::Value =
                    serde_json::from_str(&fc.arguments).unwrap_or(serde_json::Value::Null);
                tool_calls.push(LlmToolCall {
                    id: fc.call_id,
                    tool_name: fc.name,
                    arguments,
                });
            }
            ResponsesOutputItem::Unknown => {}
        }
    }

    let usage = resp.usage.unwrap_or(ResponsesUsage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
    });

    Ok(LlmResponse {
        content,
        tool_calls,
        usage: LlmUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_tokens: 0,
        },
        stop_reason,
    })
}

#[async_trait]
impl LlmClient for OpenAiResponsesClient {
    type Error = AdapterError;

    async fn complete(
        &self,
        messages: &[LlmMessage],
        model: &str,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error> {
        let body = build_request(messages, model, tools, self.max_output_tokens);
        let headers = bearer_headers(&self.api_key);

        let api_resp: ResponsesResponse = retry_post(
            &self.http,
            &self.url,
            &headers,
            &body,
            &self.retry,
            "openai_responses",
        )
        .await?;
        parse_response(api_resp)
    }
}

impl std::fmt::Debug for OpenAiResponsesClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiResponsesClient")
            .field("url", &self.url)
            .finish()
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
        let req = build_request(&messages, "gpt-4.1", None, None);
        assert_eq!(req.model, "gpt-4.1");
        assert_eq!(req.input.len(), 1);
        assert!(req.instructions.is_none());
        assert!(req.max_output_tokens.is_none());
        assert!(req.tools.is_none());
    }

    #[test]
    fn test_build_request_extracts_system_to_instructions() {
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
        let req = build_request(&messages, "gpt-4.1", None, Some(4096));
        assert_eq!(req.instructions.as_deref(), Some("You are helpful."));
        assert_eq!(req.input.len(), 1);
        assert_eq!(req.input[0].role, "user");
        assert_eq!(req.max_output_tokens, Some(4096));
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
            trust_level_required: TrustLevel::Sandboxed,
        }];
        let req = build_request(&messages, "gpt-4.1", Some(&tools), None);
        assert!(req.tools.is_some());
        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        // Verify flat structure (name at top level, not nested under "function")
        let serialized = serde_json::to_value(&tools[0]).unwrap();
        assert_eq!(serialized["type"], "function");
        assert_eq!(serialized["name"], "get_weather");
        assert!(serialized.get("function").is_none());
    }

    #[test]
    fn test_parse_text_response() {
        let resp = ResponsesResponse {
            id: Some("resp_123".to_string()),
            status: "completed".to_string(),
            output: vec![ResponsesOutputItem::Message(ResponsesOutputMessage {
                content: vec![ResponsesOutputContent::OutputText {
                    text: "Hello!".to_string(),
                }],
            })],
            usage: Some(ResponsesUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            }),
            error: None,
        };
        let llm_resp = parse_response(resp).unwrap();
        assert_eq!(llm_resp.content, "Hello!");
        assert!(llm_resp.tool_calls.is_empty());
        assert_eq!(llm_resp.usage.input_tokens, 10);
        assert_eq!(llm_resp.usage.output_tokens, 5);
        assert_eq!(llm_resp.stop_reason.as_deref(), Some("completed"));
    }

    #[test]
    fn test_parse_function_call_response() {
        let resp = ResponsesResponse {
            id: Some("resp_456".to_string()),
            status: "completed".to_string(),
            output: vec![ResponsesOutputItem::FunctionCall(ResponsesFunctionCall {
                call_id: Some("call_abc".to_string()),
                name: "get_weather".to_string(),
                arguments: r#"{"location":"Paris"}"#.to_string(),
            })],
            usage: Some(ResponsesUsage {
                input_tokens: 20,
                output_tokens: 15,
                total_tokens: 35,
            }),
            error: None,
        };
        let llm_resp = parse_response(resp).unwrap();
        assert_eq!(llm_resp.content, "");
        assert_eq!(llm_resp.tool_calls.len(), 1);
        assert_eq!(llm_resp.tool_calls[0].id.as_deref(), Some("call_abc"));
        assert_eq!(llm_resp.tool_calls[0].tool_name, "get_weather");
        assert_eq!(
            llm_resp.tool_calls[0].arguments,
            serde_json::json!({"location": "Paris"})
        );
    }

    #[test]
    fn test_parse_failed_response_returns_error() {
        let resp = ResponsesResponse {
            id: Some("resp_789".to_string()),
            status: "failed".to_string(),
            output: vec![],
            usage: None,
            error: Some(ResponsesError {
                message: "context length exceeded".to_string(),
            }),
        };
        let err = parse_response(resp).unwrap_err();
        assert!(err.to_string().contains("context length exceeded"));
    }
}
