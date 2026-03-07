//! Tool executor trait and error type.

use async_trait::async_trait;
use serde_json::Value;

/// Error returned by a tool executor.
#[derive(Debug, Clone)]
pub struct ToolExecutionError {
    pub message: String,
    pub remediation_hint: Option<String>,
    pub is_retryable: bool,
}

impl ToolExecutionError {
    pub fn simple(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            remediation_hint: None,
            is_retryable: false,
        }
    }
}

impl std::fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolExecutionError {}

/// Internal trait for pluggable tool execution backends.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        tool_name: &str,
        parameters: &Value,
    ) -> Result<Value, ToolExecutionError>;
}

/// No-op executor for testing.
#[derive(Debug, Default)]
pub struct NoOpExecutor;

#[async_trait]
impl ToolExecutor for NoOpExecutor {
    async fn execute(
        &self,
        _tool_name: &str,
        _parameters: &Value,
    ) -> Result<Value, ToolExecutionError> {
        Ok(serde_json::json!({"result": "ok"}))
    }
}
