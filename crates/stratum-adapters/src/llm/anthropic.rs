//! Anthropic Messages API client implementing `LlmClient`.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use stratum_core::LlmClient;
use stratum_types::*;

use crate::error::AdapterError;

use super::retry::{anthropic_headers, extract_system_message, retry_post, RetryConfig};

/// Anthropic Messages API client.
pub struct AnthropicClient {
    http: Client,
    api_key: String,
    url: String,
    retry: RetryConfig,
    max_tokens: u32,
}

impl AnthropicClient {
    pub fn new(api_key: String) -> Self {
        Self {
            http: Client::new(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            api_key,
            retry: RetryConfig::default(),
            max_tokens: 4096,
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.url = format!("{base_url}/v1/messages");
        self
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

// -- Anthropic API request/response shapes --

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    model: Option<String>,
    content: Vec<AnthropicContentBlock>,
    usage: AnthropicUsage,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    #[allow(dead_code)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

fn build_request(
    messages: &[LlmMessage],
    model: &str,
    tools: Option<&[ToolDefinition]>,
    max_tokens: u32,
) -> AnthropicRequest {
    let (system, rest) = extract_system_message(messages);

    let api_messages: Vec<AnthropicMessage> = rest
        .into_iter()
        .map(|m| AnthropicMessage {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let api_tools = tools.map(|tools| {
        tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.schema.clone(),
            })
            .collect()
    });

    AnthropicRequest {
        model: model.to_string(),
        max_tokens,
        system,
        messages: api_messages,
        tools: api_tools,
    }
}

fn parse_response(resp: AnthropicResponse) -> LlmResponse {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for block in resp.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                content.push_str(&text);
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(LlmToolCall {
                    id: Some(id),
                    tool_name: name,
                    arguments: input,
                });
            }
            AnthropicContentBlock::Unknown => {}
        }
    }

    LlmResponse {
        content,
        tool_calls,
        usage: LlmUsage {
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
            cached_tokens: resp.usage.cache_read_input_tokens,
        },
        stop_reason: resp.stop_reason,
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    type Error = AdapterError;

    async fn complete(
        &self,
        messages: &[LlmMessage],
        model: &str,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error> {
        let body = build_request(messages, model, tools, self.max_tokens);
        let headers = anthropic_headers(&self.api_key);

        let api_resp: AnthropicResponse = retry_post(
            &self.http,
            &self.url,
            &headers,
            &body,
            &self.retry,
            "anthropic",
        )
        .await?;
        Ok(parse_response(api_resp))
    }
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
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
        let req = build_request(&messages, "claude-opus-4-6", None, 4096);
        assert_eq!(req.model, "claude-opus-4-6");
        assert_eq!(req.max_tokens, 4096);
        assert_eq!(req.messages.len(), 1);
        assert!(req.system.is_none());
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
            trust_level_required: TrustLevel::Sandboxed,
        }];
        let req = build_request(&messages, "claude-opus-4-6", Some(&tools), 4096);
        assert!(req.tools.is_some());
        assert_eq!(req.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_build_request_extracts_system_message() {
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
        let req = build_request(&messages, "claude-opus-4-6", None, 8192);
        assert_eq!(req.system.as_deref(), Some("You are helpful."));
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.max_tokens, 8192);
    }

    #[test]
    fn test_parse_text_response() {
        let resp = AnthropicResponse {
            id: None,
            model: None,
            content: vec![AnthropicContentBlock::Text {
                text: "Hello!".to_string(),
            }],
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 3,
            },
            stop_reason: Some("end_turn".to_string()),
        };
        let llm_resp = parse_response(resp);
        assert_eq!(llm_resp.content, "Hello!");
        assert!(llm_resp.tool_calls.is_empty());
        assert_eq!(llm_resp.usage.input_tokens, 10);
        assert_eq!(llm_resp.usage.cached_tokens, 3);
        assert_eq!(llm_resp.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn test_parse_tool_use_response() {
        let resp = AnthropicResponse {
            id: None,
            model: None,
            content: vec![
                AnthropicContentBlock::Text {
                    text: "Let me check.".to_string(),
                },
                AnthropicContentBlock::ToolUse {
                    id: "toolu_123".to_string(),
                    name: "get_weather".to_string(),
                    input: serde_json::json!({"location": "Paris"}),
                },
            ],
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 15,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            stop_reason: Some("tool_use".to_string()),
        };
        let llm_resp = parse_response(resp);
        assert_eq!(llm_resp.content, "Let me check.");
        assert_eq!(llm_resp.tool_calls.len(), 1);
        assert_eq!(llm_resp.tool_calls[0].id.as_deref(), Some("toolu_123"));
        assert_eq!(llm_resp.tool_calls[0].tool_name, "get_weather");
        assert_eq!(llm_resp.stop_reason.as_deref(), Some("tool_use"));
    }
}
