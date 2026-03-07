//! Unified error type for stratum-engine.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("LLM API error (status {status}): {message}")]
    LlmApi { status: u16, message: String },

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Validation failed for tool `{tool_name}`: {message}")]
    ValidationFailed {
        tool_name: String,
        message: String,
        remediation_hint: Option<String>,
    },

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Schema compilation error: {0}")]
    SchemaCompilation(String),

    #[error("Depth limit exceeded: current {current}, limit {limit}")]
    DepthLimitExceeded { current: u32, limit: u32 },

    #[error("Dispatch error: {0}")]
    Dispatch(String),

    #[error("Timeout awaiting result for run {0}")]
    Timeout(stratum_core::RunId),
}
