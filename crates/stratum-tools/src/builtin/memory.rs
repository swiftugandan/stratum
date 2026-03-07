//! Memory tools: write, search, promote across the 4-tier memory hierarchy.
//!
//! These tools are handled by the wiring layer since they need access to the MemoryStore.
//! This module provides parameter validation, schema definitions, and the tool definitions.

use serde_json::Value;
use stratum_types::MemoryTier;

use crate::executor::ToolExecutionError;

fn err(msg: impl Into<String>) -> ToolExecutionError {
    ToolExecutionError::simple(msg)
}

/// Parse a tier string into a `MemoryTier`.
pub fn parse_tier(s: &str) -> Result<MemoryTier, ToolExecutionError> {
    match s {
        "working" => Ok(MemoryTier::Working),
        "episodic" => Ok(MemoryTier::Episodic),
        "project" => Ok(MemoryTier::Project),
        "global" => Ok(MemoryTier::Global),
        _ => Err(err(format!(
            "invalid tier: {s}. Must be one of: working, episodic, project, global"
        ))),
    }
}

/// Parsed parameters for memory_write.
#[derive(Debug, Clone)]
pub struct MemoryWriteParams {
    pub tier: MemoryTier,
    pub id: String,
    pub content: String,
}

/// Parsed parameters for memory_search.
#[derive(Debug, Clone)]
pub struct MemorySearchParams {
    pub tier: MemoryTier,
    pub query: String,
    pub limit: usize,
}

/// Parsed parameters for memory_promote.
#[derive(Debug, Clone)]
pub struct MemoryPromoteParams {
    pub entry_id: String,
    pub from_tier: MemoryTier,
    pub to_tier: MemoryTier,
}

pub fn validate_memory_write(params: &Value) -> Result<MemoryWriteParams, ToolExecutionError> {
    let tier = params
        .get("tier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: tier"))?;
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: id"))?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: content"))?;

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
        .ok_or_else(|| err("missing required parameter: tier"))?;
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: query"))?;
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    Ok(MemorySearchParams {
        tier: parse_tier(tier)?,
        query: query.to_string(),
        limit,
    })
}

pub fn validate_memory_promote(params: &Value) -> Result<MemoryPromoteParams, ToolExecutionError> {
    let entry_id = params
        .get("entry_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: entry_id"))?;
    let from = params
        .get("from_tier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: from_tier"))?;
    let to = params
        .get("to_tier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: to_tier"))?;

    Ok(MemoryPromoteParams {
        entry_id: entry_id.to_string(),
        from_tier: parse_tier(from)?,
        to_tier: parse_tier(to)?,
    })
}

/// Return all memory tool definitions.
pub fn memory_tool_definitions() -> Vec<stratum_types::ToolDefinition> {
    vec![
        stratum_types::ToolDefinition {
            name: "memory_write".to_string(),
            description: "Write an entry to a memory tier".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tier": { "type": "string", "enum": ["working", "episodic", "project", "global"], "description": "Memory tier to write to" },
                    "id": { "type": "string", "description": "Unique entry ID" },
                    "content": { "type": "string", "description": "Memory content" }
                },
                "required": ["tier", "id", "content"]
            }),
            trust_level_required: stratum_types::TrustLevel::Supervised,
        },
        stratum_types::ToolDefinition {
            name: "memory_search".to_string(),
            description: "Search for entries in a memory tier using BM25 full-text search"
                .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tier": { "type": "string", "enum": ["working", "episodic", "project", "global"], "description": "Memory tier to search" },
                    "query": { "type": "string", "description": "Search query" },
                    "limit": { "type": "number", "description": "Maximum number of results (default: 10)" }
                },
                "required": ["tier", "query"]
            }),
            trust_level_required: stratum_types::TrustLevel::Sandboxed,
        },
        stratum_types::ToolDefinition {
            name: "memory_promote".to_string(),
            description:
                "Promote a memory entry from one tier to the next (Working→Episodic→Project→Global)"
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "entry_id": { "type": "string", "description": "ID of the entry to promote" },
                    "from_tier": { "type": "string", "enum": ["working", "episodic", "project"], "description": "Current tier" },
                    "to_tier": { "type": "string", "enum": ["episodic", "project", "global"], "description": "Target tier (must be one step up)" }
                },
                "required": ["entry_id", "from_tier", "to_tier"]
            }),
            trust_level_required: stratum_types::TrustLevel::Supervised,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_tiers() {
        assert_eq!(parse_tier("working").unwrap(), MemoryTier::Working);
        assert_eq!(parse_tier("episodic").unwrap(), MemoryTier::Episodic);
        assert_eq!(parse_tier("project").unwrap(), MemoryTier::Project);
        assert_eq!(parse_tier("global").unwrap(), MemoryTier::Global);
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
        let params = serde_json::json!({"tier": "episodic", "query": "rust"});
        let result = validate_memory_search(&params).unwrap();
        assert_eq!(result.tier, MemoryTier::Episodic);
        assert_eq!(result.limit, 10); // default
    }

    #[test]
    fn validate_search_with_limit() {
        let params = serde_json::json!({"tier": "project", "query": "test", "limit": 5});
        let result = validate_memory_search(&params).unwrap();
        assert_eq!(result.limit, 5);
    }

    #[test]
    fn validate_promote() {
        let params =
            serde_json::json!({"entry_id": "e1", "from_tier": "working", "to_tier": "episodic"});
        let result = validate_memory_promote(&params).unwrap();
        assert_eq!(result.from_tier, MemoryTier::Working);
        assert_eq!(result.to_tier, MemoryTier::Episodic);
    }

    #[test]
    fn memory_definitions_count() {
        let defs = memory_tool_definitions();
        assert_eq!(defs.len(), 3);
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"memory_write"));
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"memory_promote"));
    }
}
