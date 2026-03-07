//! `create_tool` built-in: agent creates a new tool by writing a script and registering it.

use serde_json::Value;

use crate::executor::ToolExecutionError;

#[derive(Debug, Clone)]
pub struct CreateToolParams {
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub script_path: String,
    pub param_passing: String,
}

pub fn validate_create_tool_params(
    parameters: &Value,
) -> Result<CreateToolParams, ToolExecutionError> {
    let name = parameters
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: name"))?
        .to_string();

    let description = parameters
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: description"))?
        .to_string();

    let schema = parameters
        .get("schema")
        .cloned()
        .unwrap_or(serde_json::json!({"type": "object"}));

    let script_path = parameters
        .get("script_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: script_path"))?
        .to_string();

    let param_passing = parameters
        .get("param_passing")
        .and_then(|v| v.as_str())
        .unwrap_or("stdin")
        .to_string();

    if !["stdin", "json_arg", "cli_flags"].contains(&param_passing.as_str()) {
        return Err(ToolExecutionError::simple(format!(
            "invalid param_passing: {param_passing}. Must be one of: stdin, json_arg, cli_flags"
        )));
    }

    Ok(CreateToolParams {
        name,
        description,
        schema,
        script_path,
        param_passing,
    })
}

pub fn create_tool_definition() -> stratum_core::ToolDefinition {
    stratum_core::ToolDefinition {
        name: "create_tool".to_string(),
        description: "Create a new tool by registering a script. The tool will be available in subsequent turns.".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Unique tool name" },
                "description": { "type": "string", "description": "What the tool does" },
                "schema": { "type": "object", "description": "JSON Schema for tool parameters (optional)" },
                "script_path": { "type": "string", "description": "Path to the executable script" },
                "param_passing": { "type": "string", "description": "How to pass params: stdin (default), json_arg, cli_flags", "enum": ["stdin", "json_arg", "cli_flags"] }
            },
            "required": ["name", "description", "script_path"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_valid_params() {
        let params = serde_json::json!({
            "name": "my_tool",
            "description": "does stuff",
            "script_path": "/usr/local/bin/my_tool",
        });
        let result = validate_create_tool_params(&params).unwrap();
        assert_eq!(result.name, "my_tool");
        assert_eq!(result.param_passing, "stdin");
    }

    #[test]
    fn validate_missing_name() {
        let params = serde_json::json!({"description": "x", "script_path": "/tmp/t"});
        assert!(validate_create_tool_params(&params).is_err());
    }

    #[test]
    fn validate_invalid_param_passing() {
        let params = serde_json::json!({
            "name": "my_tool",
            "description": "does stuff",
            "script_path": "/tmp/t",
            "param_passing": "invalid",
        });
        assert!(validate_create_tool_params(&params).is_err());
    }
}
