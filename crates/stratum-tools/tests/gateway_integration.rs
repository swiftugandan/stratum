//! Integration tests for the Tool Execution Gateway.

use std::sync::Arc;

use uuid::Uuid;

use stratum_core::{FrozenToolRegistry, ToolGateway, ToolRegistryBuilder};
use stratum_test_utils::mocks::MockTrajectoryStore;
use stratum_types::*;

use stratum_tools::{
    DefaultConstraintEnforcer, DefaultToolGateway, InMemoryToolRegistryBuilder, NoOpExecutor,
    ToolGatewayConfig,
};

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

fn build_gateway(
    tools: Vec<ToolDefinition>,
    trust_level: TrustLevel,
) -> (
    DefaultToolGateway<MockTrajectoryStore, NoOpExecutor>,
    Arc<MockTrajectoryStore>,
) {
    let mut builder = InMemoryToolRegistryBuilder::default();
    for t in tools {
        builder.register(t).unwrap();
    }
    let registry = Arc::new(builder.build().unwrap());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = ToolGatewayConfig::default();
    let gw = DefaultToolGateway::new(
        config,
        registry,
        Arc::clone(&trajectory),
        Arc::new(NoOpExecutor),
        trust_level,
        None,
    );
    (gw, trajectory)
}

#[tokio::test]
async fn full_pipeline_success() {
    let (gw, traj) = build_gateway(
        vec![tool_def("read_file", TrustLevel::Sandboxed)],
        TrustLevel::Supervised,
    );

    let result = gw
        .call_tool(ToolInvocation {
            tool_name: "read_file".to_string(),
            parameters: serde_json::json!({"path": "/tmp/test"}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(result.output, serde_json::json!({"result": "ok"}));
    assert!(result.latency_ms < 1000);

    // Verify event sequence: ToolCalled → ToolValidated → ToolExecuted
    let events = traj.events.lock().unwrap();
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![
            EventType::ToolCalled,
            EventType::ToolValidated,
            EventType::ToolExecuted
        ]
    );
    // All events should be on ToolGateway layer
    assert!(events
        .iter()
        .all(|e| e.stratum_layer == StratumLayer::ToolGateway));
}

#[tokio::test]
async fn builder_freeze_gateway_flow() {
    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(tool_def("read_file", TrustLevel::Sandboxed))
        .unwrap();
    builder
        .register(tool_def("write_file", TrustLevel::Supervised))
        .unwrap();
    builder
        .register(tool_def("deploy", TrustLevel::Autonomous))
        .unwrap();

    let registry = Arc::new(builder.build().unwrap());
    assert_eq!(registry.get_manifest().len(), 3);
    assert!(registry.is_permitted("read_file", TrustLevel::Sandboxed));
    assert!(!registry.is_permitted("deploy", TrustLevel::Supervised));

    let trajectory = Arc::new(MockTrajectoryStore::default());
    let gw = DefaultToolGateway::new(
        ToolGatewayConfig::default(),
        registry,
        Arc::clone(&trajectory),
        Arc::new(NoOpExecutor),
        TrustLevel::Supervised,
        None,
    );

    // Supervised can use read_file and write_file, but not deploy
    let r1 = gw
        .call_tool(ToolInvocation {
            tool_name: "read_file".to_string(),
            parameters: serde_json::json!({"path": "/a"}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();
    assert_eq!(r1.status, ToolResultStatus::Success);

    let r2 = gw
        .call_tool(ToolInvocation {
            tool_name: "write_file".to_string(),
            parameters: serde_json::json!({"path": "/b"}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();
    assert_eq!(r2.status, ToolResultStatus::Success);

    let r3 = gw
        .call_tool(ToolInvocation {
            tool_name: "deploy".to_string(),
            parameters: serde_json::json!({"path": "/c"}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();
    assert_eq!(r3.status, ToolResultStatus::PolicyDenied);
}

#[tokio::test]
async fn multiple_tools_in_sequence() {
    let (gw, _) = build_gateway(
        vec![
            tool_def("read_file", TrustLevel::Sandboxed),
            tool_def("write_file", TrustLevel::Sandboxed),
        ],
        TrustLevel::Supervised,
    );

    for name in &["read_file", "write_file"] {
        let result = gw
            .call_tool(ToolInvocation {
                tool_name: name.to_string(),
                parameters: serde_json::json!({"path": "/tmp/test"}),
                run_id: Uuid::new_v4(),
            })
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
    }
}

#[tokio::test]
async fn constraint_enforcer_via_gateway() {
    // Build a gateway that has a "run_lint" tool which returns a constraint result
    use async_trait::async_trait;
    use stratum_core::ConstraintEnforcer;
    use stratum_tools::executor::{ToolExecutionError, ToolExecutor};

    struct LintExecutor;
    #[async_trait]
    impl ToolExecutor for LintExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _parameters: &serde_json::Value,
            _trust_level: TrustLevel,
        ) -> Result<serde_json::Value, ToolExecutionError> {
            Ok(serde_json::json!({
                "constraint_name": "lint",
                "passed": true,
                "violations": []
            }))
        }
    }

    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(ToolDefinition {
            name: "run_lint".to_string(),
            description: "Run linter".to_string(),
            schema: serde_json::json!({"type": "object"}),
            trust_level_required: TrustLevel::Sandboxed,
        })
        .unwrap();
    let registry = Arc::new(builder.build().unwrap());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let gw = Arc::new(DefaultToolGateway::new(
        ToolGatewayConfig::default(),
        registry,
        trajectory,
        Arc::new(LintExecutor),
        TrustLevel::Supervised,
        None,
    ));

    let enforcer = DefaultConstraintEnforcer::new(
        gw,
        vec![ConstraintDefinition {
            name: "lint".to_string(),
            description: "Run linter".to_string(),
            check_tool: "run_lint".to_string(),
            severity: ConstraintSeverity::Error,
        }],
    );

    let results = enforcer.check_all(Uuid::new_v4()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].passed);
}

#[tokio::test]
async fn event_sequence_for_failure() {
    let (gw, traj) = build_gateway(vec![], TrustLevel::Supervised);

    let result = gw
        .call_tool(ToolInvocation {
            tool_name: "nonexistent".to_string(),
            parameters: serde_json::json!({}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::ExecutionError);

    let events = traj.events.lock().unwrap();
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EventType::ToolCalled, EventType::ToolFailed]);
}

#[tokio::test]
async fn event_sequence_for_policy_denied() {
    let (gw, traj) = build_gateway(
        vec![tool_def("deploy", TrustLevel::Autonomous)],
        TrustLevel::Sandboxed,
    );

    let result = gw
        .call_tool(ToolInvocation {
            tool_name: "deploy".to_string(),
            parameters: serde_json::json!({"path": "/prod"}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::PolicyDenied);

    let events = traj.events.lock().unwrap();
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EventType::ToolCalled, EventType::ToolFailed]);
}

#[tokio::test]
async fn parent_run_id_propagated() {
    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(tool_def("read_file", TrustLevel::Sandboxed))
        .unwrap();
    let registry = Arc::new(builder.build().unwrap());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let parent_id = Uuid::new_v4();
    let gw = DefaultToolGateway::new(
        ToolGatewayConfig::default(),
        registry,
        Arc::clone(&trajectory),
        Arc::new(NoOpExecutor),
        TrustLevel::Supervised,
        Some(parent_id),
    );

    gw.call_tool(ToolInvocation {
        tool_name: "read_file".to_string(),
        parameters: serde_json::json!({"path": "/tmp"}),
        run_id: Uuid::new_v4(),
    })
    .await
    .unwrap();

    let events = trajectory.events.lock().unwrap();
    assert!(events.iter().all(|e| e.parent_run_id == Some(parent_id)));
}
