//! DefaultToolGateway: 3-stage pipeline (Intercept -> Validate -> Execute).
//! No trust-level enforcement (always autonomous).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use stratum_core::ports::ToolGateway;
use stratum_core::*;
use tracing::{debug, warn};

use crate::error::EngineError;
use crate::executor::ToolExecutor;
use crate::registry::PersistentToolRegistry;

#[derive(Debug, Clone)]
pub struct ToolGatewayConfig {
    pub validate_schema: bool,
    pub max_retries: u32,
    pub retry_base_delay: Duration,
    pub retry_max_delay: Duration,
}

impl Default for ToolGatewayConfig {
    fn default() -> Self {
        Self {
            validate_schema: true,
            max_retries: 3,
            retry_base_delay: Duration::from_millis(100),
            retry_max_delay: Duration::from_secs(5),
        }
    }
}

fn delay_for_attempt(config: &ToolGatewayConfig, attempt: u32) -> Duration {
    crate::util::retry_delay(config.retry_base_delay, config.retry_max_delay, attempt)
}

pub struct DefaultToolGateway<E: ToolExecutor> {
    config: ToolGatewayConfig,
    registry: Arc<PersistentToolRegistry>,
    executor: Arc<E>,
}

impl<E: ToolExecutor> DefaultToolGateway<E> {
    pub fn new(
        config: ToolGatewayConfig,
        registry: Arc<PersistentToolRegistry>,
        executor: Arc<E>,
    ) -> Self {
        Self {
            config,
            registry,
            executor,
        }
    }
}

impl<E: ToolExecutor> std::fmt::Debug for DefaultToolGateway<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultToolGateway")
            .field("config", &self.config)
            .finish()
    }
}

#[async_trait]
impl<E: ToolExecutor> ToolGateway for DefaultToolGateway<E> {
    type Error = EngineError;

    async fn call_tool(&self, invocation: ToolInvocation) -> Result<ToolResult, Self::Error> {
        let start = Instant::now();
        let tool_name = &invocation.tool_name;
        let latency = || start.elapsed().as_millis() as u64;

        // --- Stage 1: Intercept ---
        let tool_def = match self.registry.get_tool_owned(tool_name) {
            Some(def) => def,
            None => {
                return Ok(ToolResult {
                    tool_name: tool_name.to_string(),
                    status: ToolResultStatus::ExecutionError,
                    output: serde_json::json!({"error": format!("tool `{tool_name}` not found")}),
                    remediation_hint: Some("check the tool name and try again".to_string()),
                    latency_ms: latency(),
                });
            }
        };

        // --- Stage 2: Validate ---
        if self.config.validate_schema {
            if let Err(e) = self
                .registry
                .validate_params(tool_name, &invocation.parameters)
            {
                let message = e.to_string();
                return Ok(ToolResult {
                    tool_name: tool_name.to_string(),
                    status: ToolResultStatus::ValidationFailure,
                    output: serde_json::json!({"error": message}),
                    remediation_hint: Some(format!(
                        "check parameter schema for `{}`",
                        tool_def.name
                    )),
                    latency_ms: latency(),
                });
            }
        }

        // --- Stage 3: Execute with retry ---
        let mut last_error_msg = String::new();
        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = delay_for_attempt(&self.config, attempt - 1);
                debug!(attempt, ?delay, tool_name, "retrying tool execution");
                tokio::time::sleep(delay).await;
            }

            match self
                .executor
                .execute(tool_name, &invocation.parameters)
                .await
            {
                Ok(output) => {
                    return Ok(ToolResult {
                        tool_name: tool_name.to_string(),
                        status: ToolResultStatus::Success,
                        output,
                        remediation_hint: None,
                        latency_ms: latency(),
                    });
                }
                Err(exec_err) => {
                    last_error_msg = exec_err.message.clone();
                    if !exec_err.is_retryable {
                        return Ok(ToolResult {
                            tool_name: tool_name.to_string(),
                            status: ToolResultStatus::ExecutionError,
                            output: serde_json::json!({"error": exec_err.message}),
                            remediation_hint: exec_err.remediation_hint,
                            latency_ms: latency(),
                        });
                    }
                    warn!(
                        attempt,
                        tool_name,
                        error = %exec_err.message,
                        "retryable tool execution failure"
                    );
                }
            }
        }

        Ok(ToolResult {
            tool_name: tool_name.to_string(),
            status: ToolResultStatus::ExecutionError,
            output: serde_json::json!({
                "error": format!(
                    "retries exhausted after {} attempts: {}",
                    self.config.max_retries, last_error_msg
                )
            }),
            remediation_hint: Some(
                "the tool failed repeatedly; check logs and retry later".to_string(),
            ),
            latency_ms: latency(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::NoOpExecutor;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn make_gateway<E: ToolExecutor>(
        tools: Vec<ToolDefinition>,
        executor: E,
    ) -> DefaultToolGateway<E> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let registry = Arc::new(PersistentToolRegistry::new(Arc::new(Mutex::new(conn))).unwrap());
        registry.register_builtins(tools).unwrap();
        DefaultToolGateway::new(ToolGatewayConfig::default(), registry, Arc::new(executor))
    }

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        }
    }

    fn invocation(tool_name: &str) -> ToolInvocation {
        ToolInvocation {
            tool_name: tool_name.to_string(),
            parameters: serde_json::json!({"path": "/tmp/test"}),
            run_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn successful_call() {
        let gw = make_gateway(vec![tool_def("read_file")], NoOpExecutor);
        let result = gw.call_tool(invocation("read_file")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn unknown_tool() {
        let gw = make_gateway(vec![], NoOpExecutor);
        let result = gw.call_tool(invocation("nonexistent")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ExecutionError);
    }

    #[tokio::test]
    async fn invalid_params() {
        let gw = make_gateway(vec![tool_def("read_file")], NoOpExecutor);
        let inv = ToolInvocation {
            tool_name: "read_file".to_string(),
            parameters: serde_json::json!({}),
            run_id: Uuid::new_v4(),
        };
        let result = gw.call_tool(inv).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ValidationFailure);
    }
}
