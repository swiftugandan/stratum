use async_trait::async_trait;
use stratum_adapters::{AdapterError, AnthropicClient, OpenAiChatClient, OpenAiResponsesClient};
use stratum_core::LlmClient;
use stratum_types::{LlmMessage, LlmResponse, ToolDefinition};

use crate::config::{LlmProvider, StratumConfig};

/// Enum dispatch wrapper for LLM clients.
///
/// Avoids generics bubbling through CLI code. The enum match cost
/// is negligible compared to actual LLM API latency.
pub enum LlmClientAdapter {
    Anthropic(AnthropicClient),
    OpenAi(OpenAiChatClient),
    OpenAiResponses(OpenAiResponsesClient),
}

impl std::fmt::Debug for LlmClientAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic(_) => f.debug_tuple("Anthropic").finish(),
            Self::OpenAi(_) => f.debug_tuple("OpenAi").finish(),
            Self::OpenAiResponses(_) => f.debug_tuple("OpenAiResponses").finish(),
        }
    }
}

impl LlmClientAdapter {
    /// Construct from CLI config.
    pub fn from_config(config: &StratumConfig) -> Self {
        match config.llm_provider {
            LlmProvider::Anthropic => {
                let client =
                    AnthropicClient::new(config.api_key.clone()).with_max_tokens(config.max_tokens);
                Self::Anthropic(client)
            }
            LlmProvider::OpenAi => {
                let client = OpenAiChatClient::new(config.api_key.clone())
                    .with_max_tokens(config.max_tokens);
                Self::OpenAi(client)
            }
            LlmProvider::OpenAiResponses => {
                let client = OpenAiResponsesClient::new(config.api_key.clone())
                    .with_max_output_tokens(config.max_tokens);
                Self::OpenAiResponses(client)
            }
        }
    }
}

#[async_trait]
impl LlmClient for LlmClientAdapter {
    type Error = AdapterError;

    async fn complete(
        &self,
        messages: &[LlmMessage],
        model: &str,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error> {
        match self {
            Self::Anthropic(c) => c.complete(messages, model, tools).await,
            Self::OpenAi(c) => c.complete(messages, model, tools).await,
            Self::OpenAiResponses(c) => c.complete(messages, model, tools).await,
        }
    }
}
