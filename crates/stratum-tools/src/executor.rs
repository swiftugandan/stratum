//! Tool executor trait and default implementations.

use async_trait::async_trait;
use serde_json::Value;
use stratum_types::TrustLevel;

/// Error returned by a tool executor.
#[derive(Debug, Clone)]
pub struct ToolExecutionError {
    pub message: String,
    pub remediation_hint: Option<String>,
    pub is_retryable: bool,
}

impl std::fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolExecutionError {}

/// Internal trait for pluggable tool execution backends.
///
/// TODO: Real implementations (subprocess, WASM, etc.) are deferred to later phases.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        tool_name: &str,
        parameters: &Value,
        trust_level: TrustLevel,
    ) -> Result<Value, ToolExecutionError>;
}

/// No-op executor that returns `{"result": "ok"}` for all tools.
/// Used for testing and as a placeholder until real executors are implemented.
#[derive(Debug, Default)]
pub struct NoOpExecutor;

#[async_trait]
impl ToolExecutor for NoOpExecutor {
    async fn execute(
        &self,
        _tool_name: &str,
        _parameters: &Value,
        _trust_level: TrustLevel,
    ) -> Result<Value, ToolExecutionError> {
        Ok(serde_json::json!({"result": "ok"}))
    }
}
