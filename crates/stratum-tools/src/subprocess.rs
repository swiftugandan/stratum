//! Subprocess-based tool executor.
//!
//! Spawns tools as child processes via `tokio::process::Command` with
//! trust-level-appropriate isolation (env stripping, working dir scoping).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use stratum_types::TrustLevel;
use tokio::io::AsyncReadExt;
use tracing::{debug, warn};

use crate::executor::{ToolExecutionError, ToolExecutor};

/// How to pass JSON parameters to the subprocess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamPassing {
    /// Serialize JSON as a single CLI argument.
    JsonArg,
    /// Pipe JSON to stdin.
    Stdin,
    /// Expand top-level object keys to `--key value` flags.
    CliFlags,
}

/// Describes how to invoke a specific tool as a subprocess.
#[derive(Debug, Clone)]
pub struct ToolCommandSpec {
    /// Binary path or PATH-resolvable name.
    pub program: String,
    /// Static args prepended before parameters.
    pub args: Vec<String>,
    /// How to pass parameters to the process.
    pub param_passing: ParamPassing,
    /// Per-tool timeout override.
    pub timeout: Option<Duration>,
    /// Working directory override.
    pub working_dir: Option<PathBuf>,
    /// Additional env vars injected for this tool.
    pub env: HashMap<String, String>,
}

/// Configuration for [`SubprocessExecutor`].
#[derive(Debug, Clone)]
pub struct SubprocessExecutorConfig {
    /// Default timeout for tool execution.
    pub default_timeout: Duration,
    /// Base directory for Sandboxed execution.
    pub sandbox_root: PathBuf,
    /// Scoped directory for Supervised execution.
    pub project_root: PathBuf,
    /// Exit codes treated as retryable (e.g. EX_UNAVAILABLE=69, EX_TEMPFAIL=75).
    pub retryable_exit_codes: HashSet<i32>,
    /// Env vars stripped in Sandboxed mode.
    pub sandboxed_env_denylist: Vec<String>,
    /// Network allowlist hint exposed as env var for Supervised mode.
    pub supervised_allowed_hosts: Vec<String>,
    /// Max bytes to capture per stream (stdout and stderr independently).
    pub max_output_bytes: usize,
}

impl Default for SubprocessExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            sandbox_root: std::env::temp_dir().join("stratum-sandbox"),
            project_root: PathBuf::from("."),
            retryable_exit_codes: [69, 75].into_iter().collect(),
            sandboxed_env_denylist: vec![
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "AWS_ACCESS_KEY_ID".to_string(),
                "GITHUB_TOKEN".to_string(),
                "HOME".to_string(),
                "SSH_AUTH_SOCK".to_string(),
            ],
            supervised_allowed_hosts: Vec::new(),
            max_output_bytes: 1024 * 1024, // 1 MiB
        }
    }
}

/// A [`ToolExecutor`] that spawns tools as subprocesses.
///
/// Each registered tool maps to a [`ToolCommandSpec`] describing the binary,
/// arguments, and parameter-passing strategy. Trust level determines the
/// isolation policy applied to the child process environment.
pub struct SubprocessExecutor {
    config: SubprocessExecutorConfig,
    commands: HashMap<String, ToolCommandSpec>,
}

impl std::fmt::Debug for SubprocessExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubprocessExecutor")
            .field("config", &self.config)
            .field("commands", &self.commands.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SubprocessExecutor {
    /// Create a new subprocess executor with the given config and tool command specs.
    pub fn new(
        config: SubprocessExecutorConfig,
        commands: HashMap<String, ToolCommandSpec>,
    ) -> Self {
        Self { config, commands }
    }

    /// Build a `tokio::process::Command` from a tool spec, trust level, and parameters.
    fn build_command(
        &self,
        spec: &ToolCommandSpec,
        parameters: &Value,
        trust_level: TrustLevel,
    ) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&spec.program);

        // Static args
        cmd.args(&spec.args);

        // Parameter passing
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

        // Stdout/stderr always captured
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Apply trust-level isolation
        self.apply_trust_policy(&mut cmd, spec, trust_level);

        cmd
    }

    /// Apply environment and directory restrictions based on trust level.
    fn apply_trust_policy(
        &self,
        cmd: &mut tokio::process::Command,
        spec: &ToolCommandSpec,
        trust_level: TrustLevel,
    ) {
        match trust_level {
            TrustLevel::Sandboxed => {
                // Clear all env, set minimal PATH and trust marker
                cmd.env_clear();
                cmd.env("PATH", "/usr/bin:/bin");
                cmd.env("STRATUM_TRUST", "sandboxed");
                // Use sandbox root as working dir (spec override not honored in sandboxed)
                cmd.current_dir(&self.config.sandbox_root);
            }
            TrustLevel::Supervised => {
                // Inherit env but strip denylist entries
                for key in &self.config.sandboxed_env_denylist {
                    cmd.env_remove(key);
                }
                cmd.env("STRATUM_TRUST", "supervised");
                if !self.config.supervised_allowed_hosts.is_empty() {
                    cmd.env(
                        "STRATUM_ALLOWED_HOSTS",
                        self.config.supervised_allowed_hosts.join(","),
                    );
                }
                // Working dir: spec override or project root
                let dir = spec
                    .working_dir
                    .as_ref()
                    .unwrap_or(&self.config.project_root);
                cmd.current_dir(dir);
            }
            TrustLevel::Autonomous => {
                // Full env inheritance
                cmd.env("STRATUM_TRUST", "autonomous");
                if let Some(dir) = &spec.working_dir {
                    cmd.current_dir(dir);
                }
            }
        }

        // Inject tool-specific env vars (after trust policy so they take precedence)
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
    }

    /// Parse subprocess output into a JSON value.
    ///
    /// If stdout is valid JSON, returns it directly.
    /// Otherwise wraps stdout, stderr, and exit code in a JSON object.
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

    /// Read up to `max_bytes` from an async reader into a String.
    async fn read_limited(
        reader: &mut (impl tokio::io::AsyncRead + Unpin),
        max_bytes: usize,
    ) -> std::io::Result<String> {
        let mut buf = Vec::with_capacity(std::cmp::min(8192, max_bytes));
        let mut chunk = [0u8; 8192];
        loop {
            if buf.len() >= max_bytes {
                break;
            }
            let to_read = std::cmp::min(chunk.len(), max_bytes - buf.len());
            let n = reader.read(&mut chunk[..to_read]).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[async_trait]
impl ToolExecutor for SubprocessExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        parameters: &Value,
        trust_level: TrustLevel,
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
        let mut cmd = self.build_command(spec, parameters, trust_level);

        debug!(tool_name, ?timeout, "spawning subprocess");

        let mut child = cmd.spawn().map_err(|e| ToolExecutionError {
            message: format!("failed to spawn `{}`: {e}", spec.program),
            remediation_hint: Some("check that the program exists and is executable".to_string()),
            is_retryable: false,
        })?;

        // Serialize params once for Stdin mode (before moving into async block)
        let stdin_data = if spec.param_passing == ParamPassing::Stdin {
            Some(parameters.to_string())
        } else {
            None
        };

        // Run stdin write + stdout/stderr reads concurrently under timeout
        let max_bytes = self.config.max_output_bytes;
        let result = tokio::time::timeout(timeout, async {
            // Write stdin concurrently with reading stdout/stderr to avoid deadlocks
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
                Self::read_limited(&mut stdout_reader, max_bytes),
                Self::read_limited(&mut stderr_reader, max_bytes),
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
                // Timeout — kill the child
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract env vars from a std::process::Command for test assertions.
    fn cmd_envs(cmd: &std::process::Command) -> HashMap<String, String> {
        cmd.get_envs()
            .filter_map(|(k, v)| {
                v.map(|val| {
                    (
                        k.to_string_lossy().into_owned(),
                        val.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect()
    }

    #[test]
    fn parse_output_valid_json() {
        let result = SubprocessExecutor::parse_output(r#"{"result": "hello"}"#, "", 0);
        assert_eq!(result, serde_json::json!({"result": "hello"}));
    }

    #[test]
    fn parse_output_plain_text() {
        let result = SubprocessExecutor::parse_output("hello world", "some warn", 0);
        assert_eq!(
            result,
            serde_json::json!({
                "stdout": "hello world",
                "stderr": "some warn",
                "exit_code": 0,
            })
        );
    }

    #[test]
    fn parse_output_empty() {
        let result = SubprocessExecutor::parse_output("", "", 0);
        assert_eq!(
            result,
            serde_json::json!({
                "stdout": "",
                "stderr": "",
                "exit_code": 0,
            })
        );
    }

    #[test]
    fn config_defaults() {
        let config = SubprocessExecutorConfig::default();
        assert_eq!(config.default_timeout, Duration::from_secs(30));
        assert_eq!(config.max_output_bytes, 1024 * 1024);
        assert!(config.retryable_exit_codes.contains(&69));
        assert!(config.retryable_exit_codes.contains(&75));
        assert!(!config.sandboxed_env_denylist.is_empty());
    }

    #[test]
    fn build_command_json_arg() {
        let executor = SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new());
        let spec = ToolCommandSpec {
            program: "echo".to_string(),
            args: vec!["-n".to_string()],
            param_passing: ParamPassing::JsonArg,
            timeout: None,
            working_dir: None,
            env: HashMap::new(),
        };
        let params = serde_json::json!({"key": "value"});
        let cmd = executor.build_command(&spec, &params, TrustLevel::Autonomous);
        let cmd_ref = cmd.as_std();
        let args: Vec<_> = cmd_ref.get_args().collect();
        // Should have: -n, then the JSON string
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-n");
        assert_eq!(args[1].to_string_lossy(), params.to_string());
    }

    #[test]
    fn build_command_cli_flags() {
        let executor = SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new());
        let spec = ToolCommandSpec {
            program: "mytool".to_string(),
            args: vec![],
            param_passing: ParamPassing::CliFlags,
            timeout: None,
            working_dir: None,
            env: HashMap::new(),
        };
        let params = serde_json::json!({"name": "test", "count": 5});
        let cmd = executor.build_command(&spec, &params, TrustLevel::Autonomous);
        let cmd_ref = cmd.as_std();
        let args: Vec<String> = cmd_ref
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // Should contain --name test --count 5 (order may vary with JSON map)
        assert!(args.contains(&"--name".to_string()));
        assert!(args.contains(&"test".to_string()));
        assert!(args.contains(&"--count".to_string()));
        assert!(args.contains(&"5".to_string()));
    }

    #[test]
    fn build_command_stdin_sets_piped_stdin() {
        let executor = SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new());
        let spec = ToolCommandSpec {
            program: "cat".to_string(),
            args: vec![],
            param_passing: ParamPassing::Stdin,
            timeout: None,
            working_dir: None,
            env: HashMap::new(),
        };
        let params = serde_json::json!({"key": "value"});
        // Just verify it doesn't add params as args
        let cmd = executor.build_command(&spec, &params, TrustLevel::Autonomous);
        let cmd_ref = cmd.as_std();
        let args: Vec<_> = cmd_ref.get_args().collect();
        assert!(args.is_empty());
    }

    #[test]
    fn apply_trust_policy_sandboxed_clears_env() {
        let config = SubprocessExecutorConfig {
            sandbox_root: PathBuf::from("/tmp/sandbox"),
            ..Default::default()
        };
        let executor = SubprocessExecutor::new(config, HashMap::new());
        let spec = ToolCommandSpec {
            program: "test".to_string(),
            args: vec![],
            param_passing: ParamPassing::JsonArg,
            timeout: None,
            working_dir: None,
            env: HashMap::new(),
        };
        let cmd = executor.build_command(&spec, &serde_json::json!({}), TrustLevel::Sandboxed);
        let cmd_ref = cmd.as_std();

        // In sandboxed mode, env is cleared and only PATH + STRATUM_TRUST set
        let envs = cmd_envs(cmd_ref);
        assert_eq!(envs.get("STRATUM_TRUST").unwrap(), "sandboxed");
        assert_eq!(envs.get("PATH").unwrap(), "/usr/bin:/bin");
        assert_eq!(
            cmd_ref.get_current_dir().unwrap(),
            PathBuf::from("/tmp/sandbox")
        );
    }

    #[test]
    fn apply_trust_policy_autonomous_sets_trust_marker() {
        let executor = SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new());
        let spec = ToolCommandSpec {
            program: "test".to_string(),
            args: vec![],
            param_passing: ParamPassing::JsonArg,
            timeout: None,
            working_dir: Some(PathBuf::from("/custom")),
            env: HashMap::new(),
        };
        let cmd = executor.build_command(&spec, &serde_json::json!({}), TrustLevel::Autonomous);
        let cmd_ref = cmd.as_std();

        let envs = cmd_envs(cmd_ref);
        assert_eq!(envs.get("STRATUM_TRUST").unwrap(), "autonomous");
        assert_eq!(cmd_ref.get_current_dir().unwrap(), PathBuf::from("/custom"));
    }

    #[test]
    fn tool_specific_env_injected() {
        let executor = SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new());
        let mut tool_env = HashMap::new();
        tool_env.insert("MY_VAR".to_string(), "my_value".to_string());
        let spec = ToolCommandSpec {
            program: "test".to_string(),
            args: vec![],
            param_passing: ParamPassing::JsonArg,
            timeout: None,
            working_dir: None,
            env: tool_env,
        };
        let cmd = executor.build_command(&spec, &serde_json::json!({}), TrustLevel::Autonomous);
        let cmd_ref = cmd.as_std();

        let envs = cmd_envs(cmd_ref);
        assert_eq!(envs.get("MY_VAR").unwrap(), "my_value");
    }
}
