//! DefaultToolGateway: the main ToolGateway implementation.
//!
//! Implements the 4-stage pipeline: Intercept → Validate → Execute → Log.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::{debug, warn};

use stratum_core::{FrozenToolRegistry, ToolGateway, TrajectoryStore};
use stratum_types::*;

use crate::config::ToolGatewayConfig;
use crate::error::ToolError;
use crate::executor::ToolExecutor;
use crate::registry::InMemoryFrozenToolRegistry;
use crate::retry::delay_for_attempt;

/// Default implementation of the Tool Execution Gateway.
///
/// Generic over `T: TrajectoryStore` and `E: ToolExecutor`.
pub struct DefaultToolGateway<T: TrajectoryStore, E: ToolExecutor> {
    config: ToolGatewayConfig,
    registry: Arc<InMemoryFrozenToolRegistry>,
    trajectory: Arc<T>,
    executor: Arc<E>,
    run_trust_level: TrustLevel,
    parent_run_id: Option<RunId>,
}

impl<T: TrajectoryStore, E: ToolExecutor> DefaultToolGateway<T, E> {
    pub fn new(
        config: ToolGatewayConfig,
        registry: Arc<InMemoryFrozenToolRegistry>,
        trajectory: Arc<T>,
        executor: Arc<E>,
        run_trust_level: TrustLevel,
        parent_run_id: Option<RunId>,
    ) -> Self {
        Self {
            config,
            registry,
            trajectory,
            executor,
            run_trust_level,
            parent_run_id,
        }
    }

    async fn emit_event(
        &self,
        run_id: RunId,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> Result<(), ToolError> {
        let event = TrajectoryEvent::new(
            run_id,
            self.parent_run_id,
            event_type,
            StratumLayer::ToolGateway,
            payload,
        );
        self.trajectory
            .emit_event(event)
            .await
            .map_err(|e| ToolError::Trajectory(e.to_string()))
    }

    fn make_result(
        tool_name: &str,
        status: ToolResultStatus,
        output: serde_json::Value,
        remediation_hint: Option<String>,
        latency_ms: u64,
    ) -> ToolResult {
        ToolResult {
            tool_name: tool_name.to_string(),
            status,
            output,
            remediation_hint,
            latency_ms,
            cached_tokens_used: 0,
            uncached_tokens_used: 0,
        }
    }
}

impl<T: TrajectoryStore, E: ToolExecutor> std::fmt::Debug for DefaultToolGateway<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultToolGateway")
            .field("config", &self.config)
            .field("run_trust_level", &self.run_trust_level)
            .field("parent_run_id", &self.parent_run_id)
            .finish()
    }
}

#[async_trait]
impl<T: TrajectoryStore, E: ToolExecutor> ToolGateway for DefaultToolGateway<T, E> {
    type Error = ToolError;

    async fn call_tool(&self, invocation: ToolInvocation) -> Result<ToolResult, Self::Error> {
        let start = Instant::now();
        let run_id = invocation.run_id;
        let tool_name = &invocation.tool_name;
        let latency = || start.elapsed().as_millis() as u64;

        // --- Stage 1: Intercept ---
        self.emit_event(
            run_id,
            EventType::ToolCalled,
            serde_json::json!({ "tool_name": tool_name }),
        )
        .await?;

        let tool_def = match self.registry.get_tool(tool_name) {
            Some(def) => def,
            None => {
                self.emit_event(
                    run_id,
                    EventType::ToolFailed,
                    serde_json::json!({
                        "tool_name": tool_name,
                        "error": "tool not found",
                    }),
                )
                .await?;
                return Ok(Self::make_result(
                    tool_name,
                    ToolResultStatus::ExecutionError,
                    serde_json::json!({"error": format!("tool `{tool_name}` not found")}),
                    Some("check the tool name and try again".to_string()),
                    latency(),
                ));
            }
        };

        // --- Stage 2: Validate ---
        // Trust level check (use tool_def from stage 1 to avoid redundant lookup)
        if self.run_trust_level < tool_def.trust_level_required {
            self.emit_event(
                run_id,
                EventType::ToolFailed,
                serde_json::json!({
                    "tool_name": tool_name,
                    "error": "policy denied",
                    "required": format!("{:?}", tool_def.trust_level_required),
                    "actual": format!("{:?}", self.run_trust_level),
                }),
            )
            .await?;
            return Ok(Self::make_result(
                tool_name,
                ToolResultStatus::PolicyDenied,
                serde_json::json!({
                    "error": format!(
                        "trust level {:?} insufficient; tool requires {:?}",
                        self.run_trust_level, tool_def.trust_level_required
                    )
                }),
                Some(format!(
                    "escalate trust level to {:?} or higher",
                    tool_def.trust_level_required
                )),
                latency(),
            ));
        }

        // Schema validation (uses pre-compiled validators cached in registry)
        if self.config.validate_schema {
            if let Err(e) = self
                .registry
                .validate_params(tool_name, &invocation.parameters)
            {
                let (message, hint) = match e {
                    ToolError::ValidationFailed {
                        message,
                        remediation_hint,
                        ..
                    } => (message, remediation_hint),
                    other => (other.to_string(), None),
                };
                self.emit_event(
                    run_id,
                    EventType::ToolFailed,
                    serde_json::json!({
                        "tool_name": tool_name,
                        "error": "validation failed",
                        "message": &message,
                    }),
                )
                .await?;
                return Ok(Self::make_result(
                    tool_name,
                    ToolResultStatus::ValidationFailure,
                    serde_json::json!({"error": message}),
                    hint,
                    latency(),
                ));
            }
        }

        self.emit_event(
            run_id,
            EventType::ToolValidated,
            serde_json::json!({ "tool_name": tool_name }),
        )
        .await?;

        // --- Stage 3: Execute with retry ---
        let mut last_error_msg = String::new();
        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = delay_for_attempt(&self.config, attempt - 1);
                debug!(attempt, ?delay, tool_name, "retrying tool execution");
                tokio::time::sleep(delay).await;

                self.emit_event(
                    run_id,
                    EventType::ToolRetried,
                    serde_json::json!({
                        "tool_name": tool_name,
                        "attempt": attempt,
                    }),
                )
                .await?;
            }

            match self
                .executor
                .execute(tool_name, &invocation.parameters, self.run_trust_level)
                .await
            {
                Ok(output) => {
                    // --- Stage 4: Log success ---
                    let ms = latency();
                    self.emit_event(
                        run_id,
                        EventType::ToolExecuted,
                        serde_json::json!({
                            "tool_name": tool_name,
                            "latency_ms": ms,
                        }),
                    )
                    .await?;
                    return Ok(Self::make_result(
                        tool_name,
                        ToolResultStatus::Success,
                        output,
                        None,
                        ms,
                    ));
                }
                Err(exec_err) => {
                    last_error_msg = exec_err.message.clone();
                    if !exec_err.is_retryable {
                        self.emit_event(
                            run_id,
                            EventType::ToolFailed,
                            serde_json::json!({
                                "tool_name": tool_name,
                                "error": exec_err.message,
                                "retryable": false,
                            }),
                        )
                        .await?;
                        return Ok(Self::make_result(
                            tool_name,
                            ToolResultStatus::ExecutionError,
                            serde_json::json!({"error": exec_err.message}),
                            exec_err.remediation_hint,
                            latency(),
                        ));
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

        // All retries exhausted
        self.emit_event(
            run_id,
            EventType::ToolFailed,
            serde_json::json!({
                "tool_name": tool_name,
                "error": "retries exhausted",
                "max_retries": self.config.max_retries,
                "last_error": last_error_msg,
            }),
        )
        .await?;
        Ok(Self::make_result(
            tool_name,
            ToolResultStatus::ExecutionError,
            serde_json::json!({
                "error": format!(
                    "retries exhausted after {} attempts: {}",
                    self.config.max_retries, last_error_msg
                )
            }),
            Some("the tool failed repeatedly; check logs and retry later".to_string()),
            latency(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{NoOpExecutor, ToolExecutionError, ToolExecutor};
    use crate::registry::InMemoryToolRegistryBuilder;
    use stratum_core::ToolRegistryBuilder;
    use stratum_test_utils::mocks::MockTrajectoryStore;
    use uuid::Uuid;

    fn tool_def(name: &str, trust: TrustLevel) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
            trust_level_required: trust,
        }
    }

    fn build_gateway<E: ToolExecutor>(
        tools: Vec<ToolDefinition>,
        executor: E,
        trust_level: TrustLevel,
    ) -> (
        DefaultToolGateway<MockTrajectoryStore, E>,
        Arc<MockTrajectoryStore>,
    ) {
        let mut builder = InMemoryToolRegistryBuilder::default();
        for t in tools {
            builder.register(t).unwrap();
        }
        let registry = Arc::new(builder.build().unwrap());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let gateway = DefaultToolGateway::new(
            ToolGatewayConfig::default(),
            registry,
            Arc::clone(&trajectory),
            Arc::new(executor),
            trust_level,
            None,
        );
        (gateway, trajectory)
    }

    fn invocation(tool_name: &str) -> ToolInvocation {
        ToolInvocation {
            tool_name: tool_name.to_string(),
            parameters: serde_json::json!({"path": "/tmp/test"}),
            run_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn successful_call_emits_events() {
        let (gw, traj) = build_gateway(
            vec![tool_def("read_file", TrustLevel::Sandboxed)],
            NoOpExecutor,
            TrustLevel::Supervised,
        );
        let result = gw.call_tool(invocation("read_file")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.latency_ms < 1000);

        let events = traj.events.lock().unwrap();
        let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
        assert!(types.contains(&EventType::ToolCalled));
        assert!(types.contains(&EventType::ToolValidated));
        assert!(types.contains(&EventType::ToolExecuted));
    }

    #[tokio::test]
    async fn unknown_tool_returns_execution_error() {
        let (gw, _) = build_gateway(vec![], NoOpExecutor, TrustLevel::Autonomous);
        let result = gw.call_tool(invocation("nonexistent")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ExecutionError);
        assert!(result.remediation_hint.is_some());
    }

    #[tokio::test]
    async fn trust_denied_returns_policy_denied() {
        let (gw, _) = build_gateway(
            vec![tool_def("deploy", TrustLevel::Autonomous)],
            NoOpExecutor,
            TrustLevel::Sandboxed,
        );
        let result = gw.call_tool(invocation("deploy")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::PolicyDenied);
    }

    #[tokio::test]
    async fn invalid_params_returns_validation_failure() {
        let (gw, _) = build_gateway(
            vec![tool_def("read_file", TrustLevel::Sandboxed)],
            NoOpExecutor,
            TrustLevel::Supervised,
        );
        let inv = ToolInvocation {
            tool_name: "read_file".to_string(),
            parameters: serde_json::json!({}), // missing required "path"
            run_id: Uuid::new_v4(),
        };
        let result = gw.call_tool(inv).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ValidationFailure);
        assert!(result.remediation_hint.is_some());
    }

    #[tokio::test]
    async fn executor_failure_returns_execution_error() {
        struct FailExecutor;
        #[async_trait]
        impl ToolExecutor for FailExecutor {
            async fn execute(
                &self,
                _: &str,
                _: &serde_json::Value,
                _: TrustLevel,
            ) -> Result<serde_json::Value, ToolExecutionError> {
                Err(ToolExecutionError {
                    message: "disk full".to_string(),
                    remediation_hint: Some("free disk space".to_string()),
                    is_retryable: false,
                })
            }
        }

        let (gw, _) = build_gateway(
            vec![tool_def("write_file", TrustLevel::Sandboxed)],
            FailExecutor,
            TrustLevel::Supervised,
        );
        let result = gw.call_tool(invocation("write_file")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ExecutionError);
    }

    #[tokio::test]
    async fn retryable_failure_retries_then_succeeds() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct RetryExecutor {
            attempts: AtomicU32,
        }
        #[async_trait]
        impl ToolExecutor for RetryExecutor {
            async fn execute(
                &self,
                _: &str,
                _: &serde_json::Value,
                _: TrustLevel,
            ) -> Result<serde_json::Value, ToolExecutionError> {
                let n = self.attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(ToolExecutionError {
                        message: "transient".to_string(),
                        remediation_hint: None,
                        is_retryable: true,
                    })
                } else {
                    Ok(serde_json::json!({"result": "ok"}))
                }
            }
        }

        let executor = RetryExecutor {
            attempts: AtomicU32::new(0),
        };
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("cmd", TrustLevel::Sandboxed))
            .unwrap();
        let registry = Arc::new(builder.build().unwrap());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let config = ToolGatewayConfig {
            retry_base_delay: std::time::Duration::from_millis(1),
            retry_max_delay: std::time::Duration::from_millis(5),
            ..Default::default()
        };
        let gw = DefaultToolGateway::new(
            config,
            registry,
            Arc::clone(&trajectory),
            Arc::new(executor),
            TrustLevel::Supervised,
            None,
        );
        let result = gw.call_tool(invocation("cmd")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);

        let events = trajectory.events.lock().unwrap();
        let retry_count = events
            .iter()
            .filter(|e| e.event_type == EventType::ToolRetried)
            .count();
        assert_eq!(retry_count, 2);
    }

    #[tokio::test]
    async fn non_retryable_skips_retry() {
        struct NonRetryExecutor;
        #[async_trait]
        impl ToolExecutor for NonRetryExecutor {
            async fn execute(
                &self,
                _: &str,
                _: &serde_json::Value,
                _: TrustLevel,
            ) -> Result<serde_json::Value, ToolExecutionError> {
                Err(ToolExecutionError {
                    message: "permanent".to_string(),
                    remediation_hint: None,
                    is_retryable: false,
                })
            }
        }

        let (gw, traj) = build_gateway(
            vec![tool_def("cmd", TrustLevel::Sandboxed)],
            NonRetryExecutor,
            TrustLevel::Supervised,
        );
        let result = gw.call_tool(invocation("cmd")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ExecutionError);

        let events = traj.events.lock().unwrap();
        let retry_count = events
            .iter()
            .filter(|e| e.event_type == EventType::ToolRetried)
            .count();
        assert_eq!(retry_count, 0);
    }

    #[tokio::test]
    async fn max_retries_exhausted() {
        struct AlwaysFailExecutor;
        #[async_trait]
        impl ToolExecutor for AlwaysFailExecutor {
            async fn execute(
                &self,
                _: &str,
                _: &serde_json::Value,
                _: TrustLevel,
            ) -> Result<serde_json::Value, ToolExecutionError> {
                Err(ToolExecutionError {
                    message: "flaky".to_string(),
                    remediation_hint: None,
                    is_retryable: true,
                })
            }
        }

        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("cmd", TrustLevel::Sandboxed))
            .unwrap();
        let registry = Arc::new(builder.build().unwrap());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let config = ToolGatewayConfig {
            max_retries: 2,
            retry_base_delay: std::time::Duration::from_millis(1),
            retry_max_delay: std::time::Duration::from_millis(5),
            ..Default::default()
        };
        let gw = DefaultToolGateway::new(
            config,
            registry,
            Arc::clone(&trajectory),
            Arc::new(AlwaysFailExecutor),
            TrustLevel::Supervised,
            None,
        );
        let result = gw.call_tool(invocation("cmd")).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::ExecutionError);
        assert!(result.output.to_string().contains("retries exhausted"));
    }

    #[tokio::test]
    async fn latency_measured() {
        let (gw, _) = build_gateway(
            vec![tool_def("read_file", TrustLevel::Sandboxed)],
            NoOpExecutor,
            TrustLevel::Supervised,
        );
        let result = gw.call_tool(invocation("read_file")).await.unwrap();
        // latency should be a small positive number (< 1 second for a no-op)
        assert!(result.latency_ms < 1000);
    }

    #[tokio::test]
    async fn schema_validation_disabled() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("read_file", TrustLevel::Sandboxed))
            .unwrap();
        let registry = Arc::new(builder.build().unwrap());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let config = ToolGatewayConfig {
            validate_schema: false,
            ..Default::default()
        };
        let gw = DefaultToolGateway::new(
            config,
            registry,
            Arc::clone(&trajectory),
            Arc::new(NoOpExecutor),
            TrustLevel::Supervised,
            None,
        );
        // Invalid params but schema validation is disabled
        let inv = ToolInvocation {
            tool_name: "read_file".to_string(),
            parameters: serde_json::json!({}),
            run_id: Uuid::new_v4(),
        };
        let result = gw.call_tool(inv).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
    }
}
