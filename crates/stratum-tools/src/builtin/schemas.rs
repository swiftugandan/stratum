//! JSON Schema constants for all built-in tools.

use stratum_types::{ToolDefinition, TrustLevel};

/// Return all built-in tool definitions with their schemas.
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
            trust_level_required: TrustLevel::Autonomous,
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
            trust_level_required: TrustLevel::Sandboxed,
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
            trust_level_required: TrustLevel::Autonomous,
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
            trust_level_required: TrustLevel::Sandboxed,
        },
        ToolDefinition {
            name: "search_files".to_string(),
            description: "Search for a text pattern in files under a directory".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to search in" },
                    "query": { "type": "string", "description": "Text pattern to search for" },
                    "pattern": { "type": "string", "description": "Glob pattern to filter files (optional, e.g. '*.rs')" }
                },
                "required": ["path", "query"]
            }),
            trust_level_required: TrustLevel::Sandboxed,
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
            // Schema should compile
            assert!(
                jsonschema::validator_for(&def.schema).is_ok(),
                "schema for {} failed to compile",
                def.name
            );
        }
    }

    #[test]
    fn bash_requires_autonomous() {
        let defs = builtin_tool_definitions();
        let bash = defs.iter().find(|d| d.name == "bash").unwrap();
        assert_eq!(bash.trust_level_required, TrustLevel::Autonomous);
    }

    #[test]
    fn read_file_allows_sandboxed() {
        let defs = builtin_tool_definitions();
        let rf = defs.iter().find(|d| d.name == "read_file").unwrap();
        assert_eq!(rf.trust_level_required, TrustLevel::Sandboxed);
    }

    #[test]
    fn write_file_requires_autonomous() {
        let defs = builtin_tool_definitions();
        let wf = defs.iter().find(|d| d.name == "write_file").unwrap();
        assert_eq!(wf.trust_level_required, TrustLevel::Autonomous);
    }
}
