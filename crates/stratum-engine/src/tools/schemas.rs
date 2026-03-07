//! JSON Schema definitions for basic built-in tools.

use stratum_core::ToolDefinition;

pub fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "bash".to_string(),
            description: "Execute a shell command via /bin/sh -c".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "working_dir": { "type": "string", "description": "Working directory (optional)" },
                    "timeout_ms": { "type": "number", "description": "Timeout in milliseconds (optional, default 30000)" }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to read" },
                    "offset": { "type": "number", "description": "Byte offset to start reading from (optional)" },
                    "limit": { "type": "number", "description": "Maximum number of bytes to read (optional)" }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write content to a file (creates or overwrites)".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to write" },
                    "content": { "type": "string", "description": "Content to write to the file" }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "list_directory".to_string(),
            description: "List files and directories at a path".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path to list" },
                    "pattern": { "type": "string", "description": "Glob pattern to filter entries (optional)" }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "search_files".to_string(),
            description: "Search for a text pattern in files under a directory".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to search in" },
                    "query": { "type": "string", "description": "Text pattern to search for" },
                    "pattern": { "type": "string", "description": "Glob pattern to filter files (optional)" }
                },
                "required": ["path", "query"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_definitions_have_valid_schemas() {
        let defs = builtin_tool_definitions();
        assert_eq!(defs.len(), 5);
        for def in &defs {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
            assert!(
                jsonschema::validator_for(&def.schema).is_ok(),
                "schema for {} failed to compile",
                def.name
            );
        }
    }
}
