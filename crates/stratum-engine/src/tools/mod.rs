//! Built-in tool executor for native tools (bash, file ops, memory, tool/skill creation).

pub mod bash;
pub mod create_skill;
pub mod create_tool;
pub mod file_ops;
pub mod memory;
pub mod schemas;

use async_trait::async_trait;
use serde_json::Value;
use stratum_core::MemoryTier;

use crate::executor::{ToolExecutionError, ToolExecutor};

fn tier_str(tier: MemoryTier) -> &'static str {
    match tier {
        MemoryTier::Working => "working",
        MemoryTier::Persistent => "persistent",
    }
}

/// All built-in tool names.
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
];

/// Built-in tool executor that handles native tools directly.
#[derive(Debug, Default)]
pub struct BuiltinExecutor;

impl BuiltinExecutor {
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
    ) -> Result<Value, ToolExecutionError> {
        match tool_name {
            "bash" => bash::execute_bash(parameters).await,
            "read_file" => file_ops::execute_read_file(parameters).await,
            "write_file" => file_ops::execute_write_file(parameters).await,
            "list_directory" => file_ops::execute_list_directory(parameters).await,
            "search_files" => file_ops::execute_search_files(parameters).await,

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

/// Return all built-in tool definitions.
pub fn all_builtin_definitions() -> Vec<stratum_core::ToolDefinition> {
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
        let result = executor.execute("nope", &serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn routes_to_bash() {
        let executor = BuiltinExecutor;
        let result = executor
            .execute("bash", &serde_json::json!({"command": "echo routed"}))
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
            )
            .await
            .unwrap();
        assert_eq!(result["_builtin_action"], "memory_write");
    }

    #[test]
    fn all_builtin_definitions_count() {
        let defs = all_builtin_definitions();
        // 5 basic + create_tool + create_skill + 2 memory = 9
        assert_eq!(defs.len(), 9);
    }
}
