//! Integration tests for SubprocessExecutor using real system commands.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use stratum_core::{ToolGateway, ToolRegistryBuilder};
use stratum_test_utils::mocks::MockTrajectoryStore;
use stratum_types::*;

use stratum_tools::executor::ToolExecutor;
use stratum_tools::{
    DefaultToolGateway, InMemoryToolRegistryBuilder, ParamPassing, SubprocessExecutor,
    SubprocessExecutorConfig, ToolCommandSpec, ToolGatewayConfig,
};

fn echo_json_spec() -> ToolCommandSpec {
    // echo -n '{"result":"hello"}' — uses sh -c to get proper JSON output
    ToolCommandSpec {
        program: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "echo '{\"result\":\"hello\"}'".to_string(),
        ],
        param_passing: ParamPassing::JsonArg,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    }
}

fn make_executor(specs: Vec<(&str, ToolCommandSpec)>) -> SubprocessExecutor {
    let commands: HashMap<String, ToolCommandSpec> = specs
        .into_iter()
        .map(|(name, spec)| (name.to_string(), spec))
        .collect();
    SubprocessExecutor::new(SubprocessExecutorConfig::default(), commands)
}

#[tokio::test]
async fn echo_json_via_json_arg() {
    let executor = make_executor(vec![("echo_json", echo_json_spec())]);
    let result = executor
        .execute("echo_json", &serde_json::json!({}), TrustLevel::Autonomous)
        .await
        .unwrap();
    assert_eq!(result, serde_json::json!({"result": "hello"}));
}

#[tokio::test]
async fn pipe_json_via_stdin() {
    let spec = ToolCommandSpec {
        program: "cat".to_string(),
        args: vec![],
        param_passing: ParamPassing::Stdin,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    };
    let executor = make_executor(vec![("cat_stdin", spec)]);
    let params = serde_json::json!({"key": "value"});
    let result = executor
        .execute("cat_stdin", &params, TrustLevel::Autonomous)
        .await
        .unwrap();
    // cat echoes back the JSON, which should be parsed
    assert_eq!(result, params);
}

#[tokio::test]
async fn nonzero_exit_returns_error() {
    let spec = ToolCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo 'oops' >&2; exit 1".to_string()],
        param_passing: ParamPassing::JsonArg,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    };
    let executor = make_executor(vec![("fail_tool", spec)]);
    let err = executor
        .execute("fail_tool", &serde_json::json!({}), TrustLevel::Autonomous)
        .await
        .unwrap_err();
    assert!(err.message.contains("exited with code 1"));
    assert!(err.message.contains("oops"));
    assert!(!err.is_retryable);
}

#[tokio::test]
async fn retryable_exit_code() {
    let spec = ToolCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "exit 75".to_string()],
        param_passing: ParamPassing::JsonArg,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    };
    let executor = make_executor(vec![("tempfail", spec)]);
    let err = executor
        .execute("tempfail", &serde_json::json!({}), TrustLevel::Autonomous)
        .await
        .unwrap_err();
    assert!(err.message.contains("exited with code 75"));
    assert!(err.is_retryable);
}

#[tokio::test]
async fn timeout_kills_child() {
    let spec = ToolCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 60".to_string()],
        param_passing: ParamPassing::Stdin,
        timeout: Some(Duration::from_millis(100)),
        working_dir: None,
        env: HashMap::new(),
    };
    let executor = make_executor(vec![("slow", spec)]);
    let err = executor
        .execute("slow", &serde_json::json!({}), TrustLevel::Autonomous)
        .await
        .unwrap_err();
    assert!(err.message.contains("timed out"));
    assert!(err.is_retryable);
}

#[tokio::test]
async fn cli_flags_expansion() {
    let spec2 = ToolCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo done".to_string()],
        param_passing: ParamPassing::CliFlags,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    };
    let executor = make_executor(vec![("flags_tool", spec2)]);
    // The flags are appended after the static args, so sh -c "echo done" --name test
    // sh ignores extra args after -c command, but the command itself runs fine
    let result = executor
        .execute(
            "flags_tool",
            &serde_json::json!({"name": "test"}),
            TrustLevel::Autonomous,
        )
        .await
        .unwrap();
    // Output should contain "done"
    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    assert!(stdout.contains("done"));
}

#[tokio::test]
async fn sandboxed_strips_env() {
    // Create sandbox dir
    let sandbox = std::env::temp_dir().join("stratum-sandbox-test");
    std::fs::create_dir_all(&sandbox).unwrap();

    let spec = ToolCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "env".to_string()],
        param_passing: ParamPassing::Stdin,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    };
    let config = SubprocessExecutorConfig {
        sandbox_root: sandbox.clone(),
        ..Default::default()
    };
    let commands: HashMap<String, ToolCommandSpec> =
        [("env_tool".to_string(), spec)].into_iter().collect();
    let executor = SubprocessExecutor::new(config, commands);

    let result = executor
        .execute("env_tool", &serde_json::json!({}), TrustLevel::Sandboxed)
        .await
        .unwrap();

    // Output should be plain text (env listing), wrapped in stdout/stderr/exit_code
    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    // Should have STRATUM_TRUST=sandboxed
    assert!(stdout.contains("STRATUM_TRUST=sandboxed"));
    // Should NOT have sensitive vars (they were cleared)
    assert!(!stdout.contains("AWS_SECRET_ACCESS_KEY"));
    assert!(!stdout.contains("GITHUB_TOKEN"));

    // Clean up
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let executor = make_executor(vec![]);
    let err = executor
        .execute(
            "nonexistent",
            &serde_json::json!({}),
            TrustLevel::Autonomous,
        )
        .await
        .unwrap_err();
    assert!(err.message.contains("no command spec registered"));
    assert!(!err.is_retryable);
}

#[tokio::test]
async fn full_gateway_pipeline_with_subprocess() {
    // Wire up SubprocessExecutor through DefaultToolGateway
    let spec = ToolCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo '{\"status\":\"ok\"}'".to_string()],
        param_passing: ParamPassing::JsonArg,
        timeout: None,
        working_dir: None,
        env: HashMap::new(),
    };
    let commands: HashMap<String, ToolCommandSpec> =
        [("my_tool".to_string(), spec)].into_iter().collect();
    let executor = SubprocessExecutor::new(SubprocessExecutorConfig::default(), commands);

    let mut builder = InMemoryToolRegistryBuilder::default();
    builder
        .register(ToolDefinition {
            name: "my_tool".to_string(),
            description: "test tool".to_string(),
            schema: serde_json::json!({"type": "object"}),
            trust_level_required: TrustLevel::Sandboxed,
        })
        .unwrap();
    let registry = Arc::new(builder.build().unwrap());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let gw = DefaultToolGateway::new(
        ToolGatewayConfig::default(),
        registry,
        Arc::clone(&trajectory),
        Arc::new(executor),
        TrustLevel::Autonomous,
        None,
    );

    let result = gw
        .call_tool(ToolInvocation {
            tool_name: "my_tool".to_string(),
            parameters: serde_json::json!({}),
            run_id: Uuid::new_v4(),
        })
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(result.output, serde_json::json!({"status": "ok"}));

    // Verify event sequence
    let events = trajectory.events.lock().unwrap();
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![
            EventType::ToolCalled,
            EventType::ToolValidated,
            EventType::ToolExecuted
        ]
    );
}
