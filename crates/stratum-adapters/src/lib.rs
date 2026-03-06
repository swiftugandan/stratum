//! stratum-adapters: Concrete implementations of port traits.
//!
//! Houses external adapters: LLM clients, SQLite stores, vector DB,
//! filesystem memory, and notification backends.

pub mod collector;
pub mod error;
pub mod hitl;
pub mod llm;
pub mod metrics;
pub mod prompt;
pub mod server;
pub mod session;
pub mod trajectory;
pub mod util;

pub use collector::MetricsCollector;
pub use error::AdapterError;
pub use hitl::{
    GateAction, GatePolicyEngine, SqliteHitlController, StdoutNotifier, WebhookNotifier,
};
pub use llm::{AnthropicClient, OpenAiChatClient, OpenAiResponsesClient, RetryConfig};
pub use metrics::InMemoryMetrics;
pub use prompt::{initialiser_prompt, worker_prompt};
pub use server::start_metrics_server;
pub use session::SqliteSessionManager;
pub use trajectory::SqliteTrajectoryStore;

pub use stratum_core::LlmClient;
