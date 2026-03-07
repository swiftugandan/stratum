//! Memory tools: write and search across 2-tier memory hierarchy.

use serde_json::Value;
use stratum_core::MemoryTier;

use crate::executor::ToolExecutionError;

pub fn parse_tier(s: &str) -> Result<MemoryTier, ToolExecutionError> {
    match s {
        "working" => Ok(MemoryTier::Working),
        "persistent" => Ok(MemoryTier::Persistent),
        _ => Err(ToolExecutionError::simple(format!(
            "invalid tier: {s}. Must be one of: working, persistent"
        ))),
    }
}

#[derive(Debug, Clone)]
pub struct MemoryWriteParams {
    pub tier: MemoryTier,
    pub id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct MemorySearchParams {
    pub tier: MemoryTier,
    pub query: String,
    pub limit: usize,
}

pub fn validate_memory_write(params: &Value) -> Result<MemoryWriteParams, ToolExecutionError> {
    let tier = params
        .get("tier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: tier"))?;
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: id"))?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: content"))?;

    Ok(MemoryWriteParams {
        tier: parse_tier(tier)?,
        id: id.to_string(),
        content: content.to_string(),
    })
}

pub fn validate_memory_search(params: &Value) -> Result<MemorySearchParams, ToolExecutionError> {
    let tier = params
        .get("tier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: tier"))?;
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: query"))?;
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    Ok(MemorySearchParams {
        tier: parse_tier(tier)?,
        query: query.to_string(),
        limit,
    })
}

pub fn memory_tool_definitions() -> Vec<stratum_core::ToolDefinition> {
    vec![
        stratum_core::ToolDefinition {
            name: "memory_write".to_string(),
            description: "Write an entry to a memory tier".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tier": { "type": "string", "enum": ["working", "persistent"], "description": "Memory tier to write to" },
                    "id": { "type": "string", "description": "Unique entry ID" },
                    "content": { "type": "string", "description": "Memory content" }
                },
                "required": ["tier", "id", "content"]
            }),
        },
        stratum_core::ToolDefinition {
            name: "memory_search".to_string(),
            description: "Search for entries in a memory tier using full-text search".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tier": { "type": "string", "enum": ["working", "persistent"], "description": "Memory tier to search" },
                    "query": { "type": "string", "description": "Search query" },
                    "limit": { "type": "number", "description": "Maximum number of results (default: 10)" }
                },
                "required": ["tier", "query"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_tiers() {
        assert_eq!(parse_tier("working").unwrap(), MemoryTier::Working);
        assert_eq!(parse_tier("persistent").unwrap(), MemoryTier::Persistent);
        assert!(parse_tier("invalid").is_err());
    }

    #[test]
    fn validate_write() {
        let params = serde_json::json!({"tier": "working", "id": "test-1", "content": "hello"});
        let result = validate_memory_write(&params).unwrap();
        assert_eq!(result.tier, MemoryTier::Working);
        assert_eq!(result.id, "test-1");
    }

    #[test]
    fn validate_write_missing_tier() {
        let params = serde_json::json!({"id": "test-1", "content": "hello"});
        assert!(validate_memory_write(&params).is_err());
    }

    #[test]
    fn validate_search() {
        let params = serde_json::json!({"tier": "persistent", "query": "rust"});
        let result = validate_memory_search(&params).unwrap();
        assert_eq!(result.tier, MemoryTier::Persistent);
        assert_eq!(result.limit, 10);
    }

    #[test]
    fn memory_definitions_count() {
        let defs = memory_tool_definitions();
        assert_eq!(defs.len(), 2);
    }
}
