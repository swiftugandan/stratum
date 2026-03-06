//! Security audit tests for the Tool Execution Gateway.
//!
//! Verifies trust level enforcement, no sandbox escapes, and no tool manifest bypasses.

use std::sync::Arc;

use async_trait::async_trait;
use stratum_core::{FrozenToolRegistry, ToolGateway, ToolRegistryBuilder};
use stratum_test_utils::mocks::MockTrajectoryStore;
use stratum_tools::config::ToolGatewayConfig;
use stratum_tools::executor::{NoOpExecutor, ToolExecutionError, ToolExecutor};
use stratum_tools::gateway::DefaultToolGateway;
use stratum_tools::registry::InMemoryToolRegistryBuilder;
use stratum_types::*;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tool_def(name: &str, trust: TrustLevel, schema: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} tool"),
        schema,
        trust_level_required: trust,
    }
}

fn invocation(tool_name: &str, params: serde_json::Value, run_id: RunId) -> ToolInvocation {
    ToolInvocation {
        tool_name: tool_name.to_string(),
        parameters: params,
        run_id,
    }
}

fn simple_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"]
    })
}

fn build_gateway_with_executor<E: ToolExecutor>(
    tools: Vec<ToolDefinition>,
    executor: E,
    trust_level: TrustLevel,
    config: ToolGatewayConfig,
) -> DefaultToolGateway<MockTrajectoryStore, E> {
    let mut builder = InMemoryToolRegistryBuilder::default();
    for t in tools {
        builder.register(t).unwrap();
    }
    let registry = Arc::new(builder.build().unwrap());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    DefaultToolGateway::new(
        config,
        registry,
        trajectory,
        Arc::new(executor),
        trust_level,
        None,
    )
}

fn build_gateway(
    tools: Vec<ToolDefinition>,
    trust_level: TrustLevel,
) -> DefaultToolGateway<MockTrajectoryStore, NoOpExecutor> {
    build_gateway_with_executor(
        tools,
        NoOpExecutor,
        trust_level,
        ToolGatewayConfig::default(),
    )
}

// ===========================================================================
// Trust Level Enforcement
// ===========================================================================

#[tokio::test]
async fn test_sandboxed_cannot_call_supervised_tool() {
    let gw = build_gateway(
        vec![tool_def(
            "edit_file",
            TrustLevel::Supervised,
            simple_schema(),
        )],
        TrustLevel::Sandboxed,
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "edit_file",
            serde_json::json!({"path": "/tmp/x"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::PolicyDenied);
    assert!(result.output.to_string().contains("insufficient"));
}

#[tokio::test]
async fn test_sandboxed_cannot_call_autonomous_tool() {
    let gw = build_gateway(
        vec![tool_def("deploy", TrustLevel::Autonomous, simple_schema())],
        TrustLevel::Sandboxed,
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "deploy",
            serde_json::json!({"path": "/prod"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::PolicyDenied);
    assert!(result.output.to_string().contains("insufficient"));
}

#[tokio::test]
async fn test_supervised_cannot_call_autonomous_tool() {
    let gw = build_gateway(
        vec![tool_def("deploy", TrustLevel::Autonomous, simple_schema())],
        TrustLevel::Supervised,
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "deploy",
            serde_json::json!({"path": "/prod"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::PolicyDenied);
    assert!(result.output.to_string().contains("Supervised"));
    assert!(result.output.to_string().contains("Autonomous"));
}

#[tokio::test]
async fn test_autonomous_can_call_all_trust_levels() {
    let gw = build_gateway(
        vec![
            tool_def("read", TrustLevel::Sandboxed, simple_schema()),
            tool_def("write", TrustLevel::Supervised, simple_schema()),
            tool_def("deploy", TrustLevel::Autonomous, simple_schema()),
        ],
        TrustLevel::Autonomous,
    );
    let run_id = Uuid::new_v4();
    let params = serde_json::json!({"path": "/tmp"});

    for name in &["read", "write", "deploy"] {
        let result = gw
            .call_tool(invocation(name, params.clone(), run_id))
            .await
            .unwrap();
        assert_eq!(
            result.status,
            ToolResultStatus::Success,
            "Autonomous gateway should be able to call {name}"
        );
    }
}

#[tokio::test]
async fn test_supervised_can_call_sandboxed_and_supervised() {
    let gw = build_gateway(
        vec![
            tool_def("read", TrustLevel::Sandboxed, simple_schema()),
            tool_def("write", TrustLevel::Supervised, simple_schema()),
        ],
        TrustLevel::Supervised,
    );
    let run_id = Uuid::new_v4();
    let params = serde_json::json!({"path": "/tmp"});

    for name in &["read", "write"] {
        let result = gw
            .call_tool(invocation(name, params.clone(), run_id))
            .await
            .unwrap();
        assert_eq!(
            result.status,
            ToolResultStatus::Success,
            "Supervised gateway should be able to call {name}"
        );
    }
}

#[tokio::test]
async fn test_exact_trust_level_match_allowed() {
    let gw = build_gateway(
        vec![tool_def("edit", TrustLevel::Supervised, simple_schema())],
        TrustLevel::Supervised,
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "edit",
            serde_json::json!({"path": "/tmp"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::Success);
}

// ===========================================================================
// No Sandbox Escapes
// ===========================================================================

#[tokio::test]
async fn test_tool_not_in_registry_rejected() {
    let gw = build_gateway(
        vec![tool_def("read", TrustLevel::Sandboxed, simple_schema())],
        TrustLevel::Autonomous,
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "not_registered",
            serde_json::json!({"path": "/etc/shadow"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::ExecutionError);
    assert!(result.output.to_string().contains("not found"));
}

#[test]
fn test_frozen_registry_immutable() {
    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(tool_def("tool_a", TrustLevel::Sandboxed, simple_schema()))
        .unwrap();
    builder
        .register(tool_def("tool_b", TrustLevel::Supervised, simple_schema()))
        .unwrap();
    let registry = builder.build().unwrap();

    // After freezing, get_manifest returns exactly the registered tools.
    let manifest = registry.get_manifest();
    assert_eq!(manifest.len(), 2);
    let names: Vec<&str> = manifest.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"tool_a"));
    assert!(names.contains(&"tool_b"));

    // InMemoryFrozenToolRegistry has no public method to add tools after building.
    // The only way to get tools into a registry is through the builder, which is
    // consumed by build(). This is enforced by Rust's type system.
    assert!(registry.get_tool("tool_a").is_some());
    assert!(registry.get_tool("tool_b").is_some());
    assert!(registry.get_tool("injected_tool").is_none());
}

#[test]
fn test_registry_duplicate_name_rejected() {
    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(tool_def("read", TrustLevel::Sandboxed, simple_schema()))
        .unwrap();
    let result = builder.register(tool_def("read", TrustLevel::Autonomous, simple_schema()));
    assert!(result.is_err(), "duplicate tool name must be rejected");
}

// ===========================================================================
// No Tool Manifest Bypasses
// ===========================================================================

#[tokio::test]
async fn test_schema_validation_rejects_invalid_params() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "mode": { "type": "string" }
        },
        "required": ["path", "mode"]
    });
    let gw = build_gateway(
        vec![tool_def("strict_tool", TrustLevel::Sandboxed, schema)],
        TrustLevel::Autonomous,
    );
    let run_id = Uuid::new_v4();
    // Missing required "mode" field.
    let result = gw
        .call_tool(invocation(
            "strict_tool",
            serde_json::json!({"path": "/tmp"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::ValidationFailure);
}

#[tokio::test]
async fn test_schema_validation_rejects_wrong_type() {
    let gw = build_gateway(
        vec![tool_def(
            "typed_tool",
            TrustLevel::Sandboxed,
            simple_schema(),
        )],
        TrustLevel::Autonomous,
    );
    let run_id = Uuid::new_v4();
    // Schema expects "path" to be a string, pass an integer instead.
    let result = gw
        .call_tool(invocation(
            "typed_tool",
            serde_json::json!({"path": 42}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::ValidationFailure);
}

#[tokio::test]
async fn test_schema_validation_allows_valid_params() {
    let gw = build_gateway(
        vec![tool_def(
            "valid_tool",
            TrustLevel::Sandboxed,
            simple_schema(),
        )],
        TrustLevel::Autonomous,
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "valid_tool",
            serde_json::json!({"path": "/tmp/ok"}),
            run_id,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::Success);
}

#[tokio::test]
async fn test_schema_validation_can_be_disabled() {
    let config = ToolGatewayConfig {
        validate_schema: false,
        ..Default::default()
    };
    let gw = build_gateway_with_executor(
        vec![tool_def("lax_tool", TrustLevel::Sandboxed, simple_schema())],
        NoOpExecutor,
        TrustLevel::Autonomous,
        config,
    );
    let run_id = Uuid::new_v4();
    // Missing required "path" field, but schema validation is disabled.
    let result = gw
        .call_tool(invocation("lax_tool", serde_json::json!({}), run_id))
        .await
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::Success);
}

// ===========================================================================
// Trust Level Ordering
// ===========================================================================

#[test]
fn test_trust_level_ordering() {
    assert!(TrustLevel::Sandboxed < TrustLevel::Supervised);
    assert!(TrustLevel::Supervised < TrustLevel::Autonomous);
    assert!(TrustLevel::Sandboxed < TrustLevel::Autonomous);

    // Equality
    assert!(TrustLevel::Sandboxed == TrustLevel::Sandboxed);
    assert!(TrustLevel::Supervised == TrustLevel::Supervised);
    assert!(TrustLevel::Autonomous == TrustLevel::Autonomous);

    // Greater-than (reverse)
    assert!(TrustLevel::Autonomous > TrustLevel::Supervised);
    assert!(TrustLevel::Supervised > TrustLevel::Sandboxed);
}

#[test]
fn test_is_permitted_respects_trust_hierarchy() {
    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(tool_def(
            "sandbox_tool",
            TrustLevel::Sandboxed,
            simple_schema(),
        ))
        .unwrap();
    builder
        .register(tool_def(
            "supervised_tool",
            TrustLevel::Supervised,
            simple_schema(),
        ))
        .unwrap();
    builder
        .register(tool_def(
            "autonomous_tool",
            TrustLevel::Autonomous,
            simple_schema(),
        ))
        .unwrap();
    let registry = builder.build().unwrap();

    // Sandboxed can only access Sandboxed tools.
    assert!(registry.is_permitted("sandbox_tool", TrustLevel::Sandboxed));
    assert!(!registry.is_permitted("supervised_tool", TrustLevel::Sandboxed));
    assert!(!registry.is_permitted("autonomous_tool", TrustLevel::Sandboxed));

    // Supervised can access Sandboxed and Supervised tools.
    assert!(registry.is_permitted("sandbox_tool", TrustLevel::Supervised));
    assert!(registry.is_permitted("supervised_tool", TrustLevel::Supervised));
    assert!(!registry.is_permitted("autonomous_tool", TrustLevel::Supervised));

    // Autonomous can access all tools.
    assert!(registry.is_permitted("sandbox_tool", TrustLevel::Autonomous));
    assert!(registry.is_permitted("supervised_tool", TrustLevel::Autonomous));
    assert!(registry.is_permitted("autonomous_tool", TrustLevel::Autonomous));

    // Unknown tool is never permitted, even at Autonomous.
    assert!(!registry.is_permitted("nonexistent", TrustLevel::Autonomous));
}

// ===========================================================================
// Executor Isolation
// ===========================================================================

#[tokio::test]
async fn test_executor_error_doesnt_leak_internals() {
    struct LeakyExecutor;
    #[async_trait]
    impl ToolExecutor for LeakyExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _parameters: &serde_json::Value,
            _trust_level: TrustLevel,
        ) -> Result<serde_json::Value, ToolExecutionError> {
            Err(ToolExecutionError {
                message: "connection refused to db-host-internal:5432".to_string(),
                remediation_hint: Some("check database connectivity".to_string()),
                is_retryable: false,
            })
        }
    }

    let gw = build_gateway_with_executor(
        vec![tool_def("db_query", TrustLevel::Sandboxed, simple_schema())],
        LeakyExecutor,
        TrustLevel::Autonomous,
        ToolGatewayConfig::default(),
    );
    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "db_query",
            serde_json::json!({"path": "/query"}),
            run_id,
        ))
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::ExecutionError);

    // The output contains the error message from the executor, but should not
    // contain stack traces or Rust-internal paths (e.g. /rustc/, .rs:).
    let output_str = result.output.to_string();
    assert!(
        !output_str.contains("/rustc/"),
        "output must not leak rustc internal paths"
    );
    assert!(
        !output_str.contains("stack backtrace"),
        "output must not leak stack traces"
    );
    assert!(
        !output_str.contains(".rs:"),
        "output must not leak source file paths"
    );
    // The actual error message is present.
    assert!(output_str.contains("connection refused"));
}

#[tokio::test]
async fn test_retryable_error_doesnt_bypass_trust() {
    // A tool that requires Autonomous trust, called from a Supervised gateway,
    // should be denied on the first attempt and never reach the executor at all
    // (no retries either, since trust check happens before execution).
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingExecutor {
        call_count: AtomicU32,
    }
    #[async_trait]
    impl ToolExecutor for CountingExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _parameters: &serde_json::Value,
            _trust_level: TrustLevel,
        ) -> Result<serde_json::Value, ToolExecutionError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(ToolExecutionError {
                message: "should never be called".to_string(),
                remediation_hint: None,
                is_retryable: true,
            })
        }
    }

    let executor = CountingExecutor {
        call_count: AtomicU32::new(0),
    };
    let executor = Arc::new(executor);
    let executor_ref = Arc::clone(&executor);

    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(tool_def(
            "admin_tool",
            TrustLevel::Autonomous,
            simple_schema(),
        ))
        .unwrap();
    let registry = Arc::new(builder.build().unwrap());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = ToolGatewayConfig {
        max_retries: 5,
        retry_base_delay: std::time::Duration::from_millis(1),
        retry_max_delay: std::time::Duration::from_millis(5),
        ..Default::default()
    };
    let gw = DefaultToolGateway::new(
        config,
        registry,
        trajectory,
        executor,
        TrustLevel::Supervised, // Insufficient for Autonomous tool
        None,
    );

    let run_id = Uuid::new_v4();
    let result = gw
        .call_tool(invocation(
            "admin_tool",
            serde_json::json!({"path": "/admin"}),
            run_id,
        ))
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::PolicyDenied);
    // The executor should never have been called because trust check happens first.
    assert_eq!(
        executor_ref.call_count.load(Ordering::SeqCst),
        0,
        "executor must not be called when trust level is insufficient"
    );
}
