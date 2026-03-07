//! Subprocess-based tool executor.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, warn};

use crate::executor::{ToolExecutionError, ToolExecutor};
use crate::util::read_limited;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamPassing {
    JsonArg,
    Stdin,
    CliFlags,
}

#[derive(Debug, Clone)]
pub struct ToolCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub param_passing: ParamPassing,
    pub timeout: Option<Duration>,
    pub working_dir: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SubprocessExecutorConfig {
    pub default_timeout: Duration,
    pub retryable_exit_codes: HashSet<i32>,
    pub max_output_bytes: usize,
}

impl Default for SubprocessExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            retryable_exit_codes: [69, 75].into_iter().collect(),
            max_output_bytes: 1024 * 1024,
        }
    }
}

pub struct SubprocessExecutor {
    config: SubprocessExecutorConfig,
    commands: HashMap<String, ToolCommandSpec>,
}

impl std::fmt::Debug for SubprocessExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubprocessExecutor")
            .field("commands", &self.commands.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SubprocessExecutor {
    pub fn new(
        config: SubprocessExecutorConfig,
        commands: HashMap<String, ToolCommandSpec>,
    ) -> Self {
        Self { config, commands }
    }

    fn build_command(&self, spec: &ToolCommandSpec, parameters: &Value) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args);

        match &spec.param_passing {
            ParamPassing::JsonArg => {
                cmd.arg(parameters.to_string());
            }
            ParamPassing::Stdin => {
                cmd.stdin(std::process::Stdio::piped());
            }
            ParamPassing::CliFlags => {
                if let Value::Object(map) = parameters {
                    for (key, val) in map {
                        cmd.arg(format!("--{key}"));
                        match val {
                            Value::String(s) => {
                                cmd.arg(s);
                            }
                            other => {
                                cmd.arg(other.to_string());
                            }
                        }
                    }
                }
            }
        }

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.env("STRATUM_TRUST", "autonomous");

        if let Some(dir) = &spec.working_dir {
            cmd.current_dir(dir);
        }

        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        cmd
    }

    fn parse_output(stdout: &str, stderr: &str, exit_code: i32) -> Value {
        if let Ok(json) = serde_json::from_str::<Value>(stdout) {
            return json;
        }
        serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
        })
    }

}

#[async_trait]
impl ToolExecutor for SubprocessExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        parameters: &Value,
    ) -> Result<Value, ToolExecutionError> {
        let spec = self
            .commands
            .get(tool_name)
            .ok_or_else(|| ToolExecutionError {
                message: format!("no command spec registered for tool `{tool_name}`"),
                remediation_hint: Some("register a ToolCommandSpec for this tool".to_string()),
                is_retryable: false,
            })?;

        let timeout = spec.timeout.unwrap_or(self.config.default_timeout);
        let mut cmd = self.build_command(spec, parameters);

        debug!(tool_name, ?timeout, "spawning subprocess");

        let mut child = cmd.spawn().map_err(|e| ToolExecutionError {
            message: format!("failed to spawn `{}`: {e}", spec.program),
            remediation_hint: Some("check that the program exists and is executable".to_string()),
            is_retryable: false,
        })?;

        let stdin_data = if spec.param_passing == ParamPassing::Stdin {
            Some(parameters.to_string())
        } else {
            None
        };

        let max_bytes = self.config.max_output_bytes;
        let result = tokio::time::timeout(timeout, async {
            let stdin_fut = async {
                if let Some(data) = stdin_data {
                    if let Some(mut stdin) = child.stdin.take() {
                        use tokio::io::AsyncWriteExt;
                        let _ = stdin.write_all(data.as_bytes()).await;
                        drop(stdin);
                    }
                }
            };

            let mut stdout_reader = child.stdout.take().expect("stdout piped");
            let mut stderr_reader = child.stderr.take().expect("stderr piped");

            let ((), stdout, stderr) = tokio::join!(
                stdin_fut,
                read_limited(&mut stdout_reader, max_bytes),
                read_limited(&mut stderr_reader, max_bytes),
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
                if exit_code == 0 {
                    Ok(Self::parse_output(&stdout, &stderr, exit_code))
                } else {
                    let retryable = self.config.retryable_exit_codes.contains(&exit_code);
                    warn!(
                        tool_name,
                        exit_code, retryable, "subprocess exited with non-zero code"
                    );
                    Err(ToolExecutionError {
                        message: format!(
                            "process exited with code {exit_code}: {}",
                            if stderr.is_empty() {
                                stdout.trim().to_string()
                            } else {
                                stderr.trim().to_string()
                            }
                        ),
                        remediation_hint: None,
                        is_retryable: retryable,
                    })
                }
            }
            Ok((_, _, Err(e))) => Err(ToolExecutionError {
                message: format!("failed to wait on child process: {e}"),
                remediation_hint: None,
                is_retryable: false,
            }),
            Err(_) => {
                warn!(tool_name, ?timeout, "subprocess timed out, killing");
                let _ = child.kill().await;
                Err(ToolExecutionError {
                    message: format!("tool `{tool_name}` timed out after {timeout:?}"),
                    remediation_hint: Some("increase timeout or optimize the tool".to_string()),
                    is_retryable: true,
                })
            }
        }
    }
}

/// Composite executor that routes between built-in and subprocess executors.
pub struct CompositeExecutor {
    pub builtin: crate::tools::BuiltinExecutor,
    pub subprocess: SubprocessExecutor,
}

impl CompositeExecutor {
    pub fn new(builtin: crate::tools::BuiltinExecutor, subprocess: SubprocessExecutor) -> Self {
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
    ) -> Result<Value, ToolExecutionError> {
        if self.builtin.handles(tool_name) {
            self.builtin.execute(tool_name, parameters).await
        } else {
            self.subprocess.execute(tool_name, parameters).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_valid_json() {
        let result = SubprocessExecutor::parse_output(r#"{"result": "hello"}"#, "", 0);
        assert_eq!(result, serde_json::json!({"result": "hello"}));
    }

    #[test]
    fn parse_output_plain_text() {
        let result = SubprocessExecutor::parse_output("hello world", "some warn", 0);
        assert_eq!(result["stdout"], "hello world");
    }

    #[test]
    fn config_defaults() {
        let config = SubprocessExecutorConfig::default();
        assert_eq!(config.default_timeout, Duration::from_secs(30));
        assert!(config.retryable_exit_codes.contains(&69));
    }

    #[tokio::test]
    async fn composite_routes_builtin() {
        let executor = CompositeExecutor::new(
            crate::tools::BuiltinExecutor,
            SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new()),
        );
        let result = executor
            .execute("bash", &serde_json::json!({"command": "echo composite"}))
            .await
            .unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("composite"));
    }

    #[tokio::test]
    async fn composite_falls_back_to_subprocess() {
        let executor = CompositeExecutor::new(
            crate::tools::BuiltinExecutor,
            SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new()),
        );
        let result = executor
            .execute("unknown_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("no command spec"));
    }
}
