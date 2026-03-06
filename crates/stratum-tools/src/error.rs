//! Error types for the Tool Execution Gateway.

use stratum_types::TrustLevel;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("policy denied for tool `{tool_name}`: requires {required:?}, run has {actual:?}")]
    PolicyDenied {
        tool_name: String,
        required: TrustLevel,
        actual: TrustLevel,
    },

    #[error("validation failed for tool `{tool_name}`: {message}")]
    ValidationFailed {
        tool_name: String,
        message: String,
        remediation_hint: Option<String>,
    },

    #[error("execution failed for tool `{tool_name}`: {message}")]
    ExecutionFailed {
        tool_name: String,
        message: String,
        remediation_hint: Option<String>,
    },

    #[error("retries exhausted for tool `{tool_name}` after {max_retries} attempts: {last_error}")]
    RetriesExhausted {
        tool_name: String,
        max_retries: u32,
        last_error: String,
    },

    #[error("constraint not found: {0}")]
    ConstraintNotFound(String),

    #[error("trajectory error: {0}")]
    Trajectory(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("executor error: {0}")]
    Executor(String),

    #[error("schema compilation error: {0}")]
    SchemaCompilation(String),
}
