//! Integration tests for LLM clients using wiremock mock HTTP server.

use std::time::Duration;

use stratum_adapters::{AnthropicClient, OpenAiChatClient, OpenAiResponsesClient, RetryConfig};
use stratum_core::LlmClient;
use stratum_types::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn user_message(text: &str) -> LlmMessage {
    LlmMessage {
        role: "user".to_string(),
        content: text.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Anthropic client tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_basic_text_completion() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_123",
            "model": "claude-opus-4-6",
            "content": [{"type": "text", "text": "Paris is the capital of France."}],
            "usage": {"input_tokens": 15, "output_tokens": 8, "cache_read_input_tokens": 0},
            "stop_reason": "end_turn"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicClient::new("test-key".to_string()).with_base_url(server.uri());

    let messages = vec![user_message("What is the capital of France?")];
    let resp = client
        .complete(&messages, "claude-opus-4-6", None)
        .await
        .unwrap();

    assert_eq!(resp.content, "Paris is the capital of France.");
    assert_eq!(resp.usage.input_tokens, 15);
    assert_eq!(resp.usage.output_tokens, 8);
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
}

#[tokio::test]
async fn anthropic_tool_use_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [
                {"type": "text", "text": "I'll check the weather."},
                {"type": "tool_use", "id": "toolu_123", "name": "get_weather", "input": {"location": "Paris"}}
            ],
            "usage": {"input_tokens": 25, "output_tokens": 20, "cache_read_input_tokens": 5},
            "stop_reason": "tool_use"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicClient::new("test-key".to_string()).with_base_url(server.uri());

    let tools = vec![ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get weather".to_string(),
        schema: serde_json::json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        trust_level_required: TrustLevel::Sandboxed,
    }];

    let messages = vec![user_message("What's the weather in Paris?")];
    let resp = client
        .complete(&messages, "claude-opus-4-6", Some(&tools))
        .await
        .unwrap();

    assert_eq!(resp.content, "I'll check the weather.");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id.as_deref(), Some("toolu_123"));
    assert_eq!(resp.tool_calls[0].tool_name, "get_weather");
    assert_eq!(resp.tool_calls[0].arguments["location"], "Paris");
    assert_eq!(resp.usage.cached_tokens, 5);
    assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
}

#[tokio::test]
async fn anthropic_non_retryable_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"type": "authentication_error", "message": "invalid api key"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicClient::new("bad-key".to_string()).with_base_url(server.uri());

    let messages = vec![user_message("Hello")];
    let err = client
        .complete(&messages, "claude-opus-4-6", None)
        .await
        .unwrap_err();
    let err_string = err.to_string();
    assert!(
        err_string.contains("401"),
        "expected 401 error, got: {err_string}"
    );
    assert!(err_string.contains("invalid api key"));
}

#[tokio::test]
async fn anthropic_retries_on_429() {
    let server = MockServer::start().await;

    // First two calls return 429, third returns 200
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "error": {"type": "rate_limit_error", "message": "rate limited"}
        })))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type": "text", "text": "Success after retry"}],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 0},
            "stop_reason": "end_turn"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicClient::new("test-key".to_string())
        .with_base_url(server.uri())
        .with_retry(RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        });

    let messages = vec![user_message("Hello")];
    let resp = client
        .complete(&messages, "claude-opus-4-6", None)
        .await
        .unwrap();
    assert_eq!(resp.content, "Success after retry");
}

#[tokio::test]
async fn anthropic_exhausts_retries() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
        .expect(3) // initial + 2 retries
        .mount(&server)
        .await;

    let client = AnthropicClient::new("test-key".to_string())
        .with_base_url(server.uri())
        .with_retry(RetryConfig {
            max_retries: 2,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        });

    let messages = vec![user_message("Hello")];
    let err = client
        .complete(&messages, "claude-opus-4-6", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("500"));
}

// ---------------------------------------------------------------------------
// OpenAI client tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_basic_text_completion() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Paris is the capital of France."},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 15, "completion_tokens": 8}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiChatClient::new("test-key".to_string()).with_base_url(server.uri());

    let messages = vec![user_message("What is the capital of France?")];
    let resp = client.complete(&messages, "gpt-4o", None).await.unwrap();

    assert_eq!(resp.content, "Paris is the capital of France.");
    assert_eq!(resp.usage.input_tokens, 15);
    assert_eq!(resp.usage.output_tokens, 8);
    assert!(resp.tool_calls.is_empty());
}

#[tokio::test]
async fn openai_tool_call_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"Paris\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 15}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiChatClient::new("test-key".to_string()).with_base_url(server.uri());

    let tools = vec![ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get weather".to_string(),
        schema: serde_json::json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        trust_level_required: TrustLevel::Sandboxed,
    }];

    let messages = vec![user_message("What's the weather in Paris?")];
    let resp = client
        .complete(&messages, "gpt-4o", Some(&tools))
        .await
        .unwrap();

    assert_eq!(resp.content, "");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id.as_deref(), Some("call_abc123"));
    assert_eq!(resp.tool_calls[0].tool_name, "get_weather");
    assert_eq!(resp.tool_calls[0].arguments["location"], "Paris");
    assert_eq!(resp.stop_reason.as_deref(), Some("tool_calls"));
}

#[tokio::test]
async fn openai_non_retryable_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {"message": "invalid model", "type": "invalid_request_error"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiChatClient::new("test-key".to_string()).with_base_url(server.uri());

    let messages = vec![user_message("Hello")];
    let err = client
        .complete(&messages, "bad-model", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("400"));
    assert!(err.to_string().contains("invalid model"));
}

#[tokio::test]
async fn openai_retries_on_503() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Recovered!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiChatClient::new("test-key".to_string())
        .with_base_url(server.uri())
        .with_retry(RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        });

    let messages = vec![user_message("Hello")];
    let resp = client.complete(&messages, "gpt-4o", None).await.unwrap();
    assert_eq!(resp.content, "Recovered!");
}

// ---------------------------------------------------------------------------
// OpenAI Responses API client tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn responses_basic_text_completion() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_123",
            "object": "response",
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "Paris is the capital of France."}]
            }],
            "usage": {"input_tokens": 15, "output_tokens": 8, "total_tokens": 23}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiResponsesClient::new("test-key".to_string()).with_base_url(server.uri());

    let messages = vec![user_message("What is the capital of France?")];
    let resp = client.complete(&messages, "gpt-4.1", None).await.unwrap();

    assert_eq!(resp.content, "Paris is the capital of France.");
    assert_eq!(resp.usage.input_tokens, 15);
    assert_eq!(resp.usage.output_tokens, 8);
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.stop_reason.as_deref(), Some("completed"));
}

#[tokio::test]
async fn responses_function_call() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_456",
            "object": "response",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "I'll check the weather."}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_xyz",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"Paris\"}"
                }
            ],
            "usage": {"input_tokens": 25, "output_tokens": 20, "total_tokens": 45}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiResponsesClient::new("test-key".to_string()).with_base_url(server.uri());

    let tools = vec![ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get weather".to_string(),
        schema: serde_json::json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        trust_level_required: TrustLevel::Sandboxed,
    }];

    let messages = vec![user_message("What's the weather in Paris?")];
    let resp = client
        .complete(&messages, "gpt-4.1", Some(&tools))
        .await
        .unwrap();

    assert_eq!(resp.content, "I'll check the weather.");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id.as_deref(), Some("call_xyz"));
    assert_eq!(resp.tool_calls[0].tool_name, "get_weather");
    assert_eq!(resp.tool_calls[0].arguments["location"], "Paris");
}

#[tokio::test]
async fn responses_non_retryable_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {"message": "invalid model", "type": "invalid_request_error"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiResponsesClient::new("test-key".to_string()).with_base_url(server.uri());

    let messages = vec![user_message("Hello")];
    let err = client
        .complete(&messages, "bad-model", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("400"));
    assert!(err.to_string().contains("invalid model"));
}

#[tokio::test]
async fn responses_retries_on_429() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_retry",
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "Recovered!"}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 3, "total_tokens": 13}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiResponsesClient::new("test-key".to_string())
        .with_base_url(server.uri())
        .with_retry(RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        });

    let messages = vec![user_message("Hello")];
    let resp = client.complete(&messages, "gpt-4.1", None).await.unwrap();
    assert_eq!(resp.content, "Recovered!");
}
