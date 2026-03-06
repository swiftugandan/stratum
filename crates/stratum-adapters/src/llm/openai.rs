//! OpenAI-compatible chat completions client implementing `LlmClient`.
//!
//! Works with OpenAI, Azure OpenAI, and any OpenAI-API-compatible provider
//! (e.g. Ollama, vLLM, Together AI).

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use stratum_core::LlmClient;
use stratum_types::*;

use crate::error::AdapterError;

use super::retry::{bearer_headers, retry_post, RetryConfig};

/// OpenAI-compatible chat completions client.
///
/// Works with the `/v1/chat/completions` endpoint used by OpenAI, Azure OpenAI,
/// and any OpenAI-API-compatible provider (Ollama, vLLM, Together AI).
/// For OpenAI's newer Responses API, see [`super::responses::OpenAiResponsesClient`].
pub struct OpenAiChatClient {
    http: Client,
    api_key: String,
    url: String,
    retry: RetryConfig,
    max_tokens: Option<u32>,
}

impl OpenAiChatClient {
    pub fn new(api_key: String) -> Self {
        Self {
            http: Client::new(),
            url: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key,
            retry: RetryConfig::default(),
            max_tokens: None,
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.url = format!("{base_url}/v1/chat/completions");
        self
    }

    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

// -- OpenAI API request/response shapes --

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    r#type: String,
    function: ChatFunction,
}

#[derive(Debug, Serialize)]
struct ChatFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: ChatUsage,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCall {
    #[serde(default)]
    id: Option<String>,
    function: ChatFunctionCall,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    total_tokens: u64,
}

fn build_request(
    messages: &[LlmMessage],
    model: &str,
    tools: Option<&[ToolDefinition]>,
    max_tokens: Option<u32>,
) -> ChatRequest {
    let chat_messages: Vec<ChatMessage> = messages
        .iter()
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let chat_tools = tools.map(|tools| {
        tools
            .iter()
            .map(|t| ChatTool {
                r#type: "function".to_string(),
                function: ChatFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.schema.clone(),
                },
            })
            .collect()
    });

    ChatRequest {
        model: model.to_string(),
        messages: chat_messages,
        max_tokens,
        tools: chat_tools,
    }
}

fn parse_response(resp: ChatResponse) -> Result<LlmResponse, AdapterError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| AdapterError::LlmApi {
            status: 200,
            message: "empty choices in response".to_string(),
        })?;

    let content = choice.message.content.unwrap_or_default();
    let stop_reason = choice.finish_reason;

    let tool_calls = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tc| {
            let arguments: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);
            LlmToolCall {
                id: tc.id,
                tool_name: tc.function.name,
                arguments,
            }
        })
        .collect();

    Ok(LlmResponse {
        content,
        tool_calls,
        usage: LlmUsage {
            input_tokens: resp.usage.prompt_tokens,
            output_tokens: resp.usage.completion_tokens,
            cached_tokens: 0,
        },
        stop_reason,
    })
}

#[async_trait]
impl LlmClient for OpenAiChatClient {
    type Error = AdapterError;

    async fn complete(
        &self,
        messages: &[LlmMessage],
        model: &str,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error> {
        let body = build_request(messages, model, tools, self.max_tokens);
        let headers = bearer_headers(&self.api_key);

        let api_resp: ChatResponse = retry_post(
            &self.http,
            &self.url,
            &headers,
            &body,
            &self.retry,
            "openai",
        )
        .await?;
        parse_response(api_resp)
    }
}

impl std::fmt::Debug for OpenAiChatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiChatClient")
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
        let req = build_request(&messages, "gpt-4o", None, None);
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 1);
        assert!(req.max_tokens.is_none());
        assert!(req.tools.is_none());
    }

    #[test]
    fn test_build_request_with_tools_and_max_tokens() {
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
        let req = build_request(&messages, "gpt-4o", Some(&tools), Some(2048));
        assert!(req.tools.is_some());
        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].r#type, "function");
        assert_eq!(req.max_tokens, Some(2048));
    }

    #[test]
    fn test_parse_text_response() {
        let resp = ChatResponse {
            choices: vec![ChatChoice {
                message: ChatResponseMessage {
                    content: Some("Hello!".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: ChatUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        let llm_resp = parse_response(resp).unwrap();
        assert_eq!(llm_resp.content, "Hello!");
        assert!(llm_resp.tool_calls.is_empty());
        assert_eq!(llm_resp.usage.input_tokens, 10);
        assert_eq!(llm_resp.usage.output_tokens, 5);
        assert_eq!(llm_resp.usage.cached_tokens, 0);
        assert_eq!(llm_resp.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_tool_call_response() {
        let resp = ChatResponse {
            choices: vec![ChatChoice {
                message: ChatResponseMessage {
                    content: None,
                    tool_calls: Some(vec![ChatToolCall {
                        id: Some("call_abc123".to_string()),
                        function: ChatFunctionCall {
                            name: "get_weather".to_string(),
                            arguments: r#"{"location":"Paris"}"#.to_string(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: ChatUsage {
                prompt_tokens: 20,
                completion_tokens: 15,
                total_tokens: 35,
            },
        };
        let llm_resp = parse_response(resp).unwrap();
        assert_eq!(llm_resp.content, "");
        assert_eq!(llm_resp.tool_calls.len(), 1);
        assert_eq!(llm_resp.tool_calls[0].id.as_deref(), Some("call_abc123"));
        assert_eq!(llm_resp.tool_calls[0].tool_name, "get_weather");
        assert_eq!(
            llm_resp.tool_calls[0].arguments,
            serde_json::json!({"location": "Paris"})
        );
        assert_eq!(llm_resp.stop_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn test_parse_empty_choices_returns_error() {
        let resp = ChatResponse {
            choices: vec![],
            usage: ChatUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };
        assert!(parse_response(resp).is_err());
    }
}
