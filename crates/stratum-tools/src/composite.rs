//! Composite executor that routes between built-in and subprocess executors.

use async_trait::async_trait;
use serde_json::Value;
use stratum_types::TrustLevel;

use crate::builtin::BuiltinExecutor;
use crate::executor::{ToolExecutionError, ToolExecutor};
use crate::subprocess::SubprocessExecutor;

/// Routes tool execution between built-in handlers and the subprocess executor.
///
/// Tries the builtin executor first (for bash, file ops, memory, etc.),
/// then falls back to the subprocess executor for external tools.
pub struct CompositeExecutor {
    pub builtin: BuiltinExecutor,
    pub subprocess: SubprocessExecutor,
}

impl CompositeExecutor {
    pub fn new(builtin: BuiltinExecutor, subprocess: SubprocessExecutor) -> Self {
        Self {
            builtin,
            subprocess,
        }
    }
}

impl std::fmt::Debug for CompositeExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeExecutor").finish()
    }
}

#[async_trait]
impl ToolExecutor for CompositeExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        parameters: &Value,
        trust_level: TrustLevel,
    ) -> Result<Value, ToolExecutionError> {
        if self.builtin.handles(tool_name) {
            self.builtin
                .execute(tool_name, parameters, trust_level)
                .await
        } else {
            self.subprocess
                .execute(tool_name, parameters, trust_level)
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subprocess::SubprocessExecutorConfig;
    use std::collections::HashMap;

    fn make_composite() -> CompositeExecutor {
        CompositeExecutor::new(
            BuiltinExecutor,
            SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new()),
        )
    }

    #[tokio::test]
    async fn routes_builtin() {
        let executor = make_composite();
        let result = executor
            .execute(
                "bash",
                &serde_json::json!({"command": "echo composite"}),
                TrustLevel::Autonomous,
            )
            .await
            .unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("composite"));
    }

    #[tokio::test]
    async fn falls_back_to_subprocess() {
        let executor = make_composite();
        // No command spec registered, should get an error from subprocess executor
        let result = executor
            .execute(
                "unknown_tool",
                &serde_json::json!({}),
                TrustLevel::Autonomous,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("no command spec"));
    }
}
