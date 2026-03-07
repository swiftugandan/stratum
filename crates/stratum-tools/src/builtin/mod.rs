//! Built-in tool executor for native tools (bash, file ops, memory, tool/skill creation).

pub mod bash;
pub mod create_skill;
pub mod create_tool;
pub mod file_ops;
pub mod memory;
pub mod schemas;

use async_trait::async_trait;
use serde_json::Value;
use stratum_types::TrustLevel;

use stratum_types::MemoryTier;

use crate::executor::{ToolExecutionError, ToolExecutor};

fn tier_str(tier: MemoryTier) -> &'static str {
    match tier {
        MemoryTier::Working => "working",
        MemoryTier::Episodic => "episodic",
        MemoryTier::Project => "project",
        MemoryTier::Global => "global",
    }
}

/// All built-in tool names. Used for routing and identification.
pub const BUILTIN_TOOL_NAMES: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "list_directory",
    "search_files",
    "create_tool",
    "create_skill",
    "memory_write",
    "memory_search",
    "memory_promote",
];

/// Built-in tool executor that handles native tools directly.
///
/// Routes by tool name to the appropriate handler function.
/// Tools that need external state (create_tool, create_skill, memory_*)
/// return a "pending" response indicating the wiring layer must complete the action.
#[derive(Debug, Default)]
pub struct BuiltinExecutor;

impl BuiltinExecutor {
    /// Returns true if this executor handles the given tool name.
    pub fn handles(&self, tool_name: &str) -> bool {
        BUILTIN_TOOL_NAMES.contains(&tool_name)
    }
}

#[async_trait]
impl ToolExecutor for BuiltinExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        parameters: &Value,
        trust_level: TrustLevel,
    ) -> Result<Value, ToolExecutionError> {
        match tool_name {
            "bash" => bash::execute_bash(parameters, trust_level).await,
            "read_file" => file_ops::execute_read_file(parameters).await,
            "write_file" => file_ops::execute_write_file(parameters).await,
            "list_directory" => file_ops::execute_list_directory(parameters).await,
            "search_files" => file_ops::execute_search_files(parameters).await,

            // Stateful tools: validate params, return marker for wiring layer
            "create_tool" => {
                let params = create_tool::validate_create_tool_params(parameters)?;
                Ok(serde_json::json!({
                    "_builtin_action": "create_tool",
                    "name": params.name,
                    "description": params.description,
                    "schema": params.schema,
                    "script_path": params.script_path,
                    "param_passing": params.param_passing,
                }))
            }
            "create_skill" => {
                let params = create_skill::validate_create_skill_params(parameters)?;
                Ok(serde_json::json!({
                    "_builtin_action": "create_skill",
                    "name": params.name,
                    "description": params.description,
                    "triggers": params.triggers,
                    "content": params.content,
                }))
            }
            "memory_write" => {
                let params = memory::validate_memory_write(parameters)?;
                Ok(serde_json::json!({
                    "_builtin_action": "memory_write",
                    "tier": tier_str(params.tier),
                    "id": params.id,
                    "content": params.content,
                }))
            }
            "memory_search" => {
                let params = memory::validate_memory_search(parameters)?;
                Ok(serde_json::json!({
                    "_builtin_action": "memory_search",
                    "tier": tier_str(params.tier),
                    "query": params.query,
                    "limit": params.limit,
                }))
            }
            "memory_promote" => {
                let params = memory::validate_memory_promote(parameters)?;
                Ok(serde_json::json!({
                    "_builtin_action": "memory_promote",
                    "entry_id": params.entry_id,
                    "from_tier": tier_str(params.from_tier),
                    "to_tier": tier_str(params.to_tier),
                }))
            }
            _ => Err(ToolExecutionError {
                message: format!("unknown built-in tool: {tool_name}"),
                remediation_hint: Some(format!(
                    "available built-in tools: {}",
                    BUILTIN_TOOL_NAMES.join(", ")
                )),
                is_retryable: false,
            }),
        }
    }
}

/// Return all built-in tool definitions (basic tools + create_tool + create_skill + memory tools).
pub fn all_builtin_definitions() -> Vec<stratum_types::ToolDefinition> {
    let mut defs = schemas::builtin_tool_definitions();
    defs.push(create_tool::create_tool_definition());
    defs.push(create_skill::create_skill_definition());
    defs.extend(memory::memory_tool_definitions());
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_all_builtin_tools() {
        let executor = BuiltinExecutor;
        for name in BUILTIN_TOOL_NAMES {
            assert!(executor.handles(name), "should handle {name}");
        }
        assert!(!executor.handles("unknown_tool"));
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let executor = BuiltinExecutor;
        let result = executor
            .execute("nope", &serde_json::json!({}), TrustLevel::Autonomous)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn routes_to_bash() {
        let executor = BuiltinExecutor;
        let result = executor
            .execute(
                "bash",
                &serde_json::json!({"command": "echo routed"}),
                TrustLevel::Autonomous,
            )
            .await
            .unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("routed"));
    }

    #[tokio::test]
    async fn create_tool_returns_marker() {
        let executor = BuiltinExecutor;
        let result = executor
            .execute(
                "create_tool",
                &serde_json::json!({
                    "name": "my_tool",
                    "description": "test",
                    "script_path": "/tmp/tool.sh",
                }),
                TrustLevel::Autonomous,
            )
            .await
            .unwrap();
        assert_eq!(result["_builtin_action"], "create_tool");
    }

    #[tokio::test]
    async fn memory_write_returns_marker() {
        let executor = BuiltinExecutor;
        let result = executor
            .execute(
                "memory_write",
                &serde_json::json!({"tier": "working", "id": "test", "content": "data"}),
                TrustLevel::Autonomous,
            )
            .await
            .unwrap();
        assert_eq!(result["_builtin_action"], "memory_write");
    }

    #[test]
    fn all_builtin_definitions_count() {
        let defs = all_builtin_definitions();
        // 5 basic + create_tool + create_skill + 3 memory = 10
        assert_eq!(defs.len(), 10);
    }
}
