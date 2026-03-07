//! Bash tool: shell execution via `tokio::process::Command`.
//! Always autonomous — no trust-level isolation.

use std::time::Duration;

use serde_json::Value;
use tracing::{debug, warn};

use crate::executor::ToolExecutionError;
use crate::util::read_limited;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

pub async fn execute_bash(parameters: &Value) -> Result<Value, ToolExecutionError> {
    let command = parameters
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError {
            message: "missing required parameter: command".to_string(),
            remediation_hint: Some("provide a 'command' string".to_string()),
            is_retryable: false,
        })?;

    let working_dir = parameters.get("working_dir").and_then(|v| v.as_str());
    let timeout_ms = parameters
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    let timeout = Duration::from_millis(timeout_ms);

    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", command]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.env("STRATUM_TRUST", "autonomous");

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    debug!(command, ?timeout, "executing bash command");

    let mut child = cmd.spawn().map_err(|e| ToolExecutionError {
        message: format!("failed to spawn shell: {e}"),
        remediation_hint: Some("check that /bin/sh is available".to_string()),
        is_retryable: false,
    })?;

    let result = tokio::time::timeout(timeout, async {
        let mut stdout_reader = child.stdout.take().expect("stdout piped");
        let mut stderr_reader = child.stderr.take().expect("stderr piped");

        let (stdout, stderr) = tokio::join!(
            read_limited(&mut stdout_reader, MAX_OUTPUT_BYTES),
            read_limited(&mut stderr_reader, MAX_OUTPUT_BYTES),
        );

        let status = child.wait().await;
        (
            stdout.unwrap_or_default(),
            stderr.unwrap_or_default(),
            status,
        )
    })
    .await;

    match result {
        Ok((stdout, stderr, Ok(status))) => {
            let exit_code = status.code().unwrap_or(-1);
            Ok(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
            }))
        }
        Ok((_, _, Err(e))) => Err(ToolExecutionError {
            message: format!("failed to wait on child process: {e}"),
            remediation_hint: None,
            is_retryable: false,
        }),
        Err(_) => {
            warn!(command, ?timeout, "bash command timed out, killing");
            let _ = child.kill().await;
            Err(ToolExecutionError {
                message: format!("bash command timed out after {timeout_ms}ms"),
                remediation_hint: Some("increase timeout_ms or simplify the command".to_string()),
                is_retryable: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bash_echo() {
        let params = serde_json::json!({"command": "echo hello"});
        let result = execute_bash(&params).await.unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().trim(), "hello");
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn bash_exit_code() {
        let params = serde_json::json!({"command": "exit 42"});
        let result = execute_bash(&params).await.unwrap();
        assert_eq!(result["exit_code"], 42);
    }

    #[tokio::test]
    async fn bash_stderr() {
        let params = serde_json::json!({"command": "echo err >&2"});
        let result = execute_bash(&params).await.unwrap();
        assert!(result["stderr"].as_str().unwrap().contains("err"));
    }

    #[tokio::test]
    async fn bash_timeout() {
        let params = serde_json::json!({"command": "sleep 60", "timeout_ms": 100});
        let result = execute_bash(&params).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_retryable);
    }

    #[tokio::test]
    async fn bash_missing_command() {
        let params = serde_json::json!({});
        let result = execute_bash(&params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn bash_working_dir() {
        let params = serde_json::json!({"command": "pwd", "working_dir": "/tmp"});
        let result = execute_bash(&params).await.unwrap();
        let stdout = result["stdout"].as_str().unwrap().trim();
        assert!(stdout == "/tmp" || stdout == "/private/tmp");
    }
}
