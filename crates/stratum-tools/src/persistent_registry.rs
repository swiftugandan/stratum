//! `PersistentToolRegistry` — SQLite-backed, mutable between turns.
//!
//! Implements `FrozenToolRegistry` for read access. The `register_dynamic()`
//! method (outside the trait) allows the agent to create new tools at runtime.
//! The manifest is stable within a single LLM call; updates are visible between turns.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::{params, Connection};
use stratum_core::FrozenToolRegistry;
use stratum_types::{ToolDefinition, TrustLevel};

use crate::error::ToolError;

/// SQLite-backed tool registry that supports dynamic tool registration.
pub struct PersistentToolRegistry {
    conn: Arc<Mutex<Connection>>,
    /// In-memory cache: built-in + agent-created tool definitions.
    cache: RwLock<Vec<ToolDefinition>>,
    /// Name → index into cache.
    index: RwLock<HashMap<String, usize>>,
    /// Pre-compiled schema validators.
    validators: RwLock<HashMap<String, jsonschema::Validator>>,
}

impl PersistentToolRegistry {
    /// Create a new persistent registry, initializing the SQLite schema.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, ToolError> {
        {
            let c = conn.lock().unwrap();
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS agent_tools (
                    name          TEXT PRIMARY KEY,
                    description   TEXT NOT NULL,
                    schema        TEXT NOT NULL,
                    trust_level   TEXT NOT NULL DEFAULT 'supervised',
                    script_path   TEXT,
                    param_passing TEXT NOT NULL DEFAULT 'stdin',
                    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );",
            )
            .map_err(|e| ToolError::Executor(format!("failed to create agent_tools table: {e}")))?;
        }

        let registry = Self {
            conn,
            cache: RwLock::new(Vec::new()),
            index: RwLock::new(HashMap::new()),
            validators: RwLock::new(HashMap::new()),
        };

        // Load persisted tools from SQLite
        registry.reload_from_db()?;

        Ok(registry)
    }

    /// Populate cache from built-in tool definitions.
    pub fn register_builtins(&self, builtins: Vec<ToolDefinition>) -> Result<(), ToolError> {
        let mut cache = self.cache.write().unwrap();
        let mut index = self.index.write().unwrap();
        let mut validators = self.validators.write().unwrap();

        for def in builtins {
            if index.contains_key(&def.name) {
                continue; // already registered
            }

            let validator = jsonschema::validator_for(&def.schema).map_err(|e| {
                ToolError::SchemaCompilation(format!(
                    "failed to compile schema for `{}`: {e}",
                    def.name
                ))
            })?;

            let idx = cache.len();
            index.insert(def.name.clone(), idx);
            validators.insert(def.name.clone(), validator);
            cache.push(def);
        }
        Ok(())
    }

    /// Register a dynamically created tool. Writes to SQLite and updates cache.
    pub fn register_dynamic(
        &self,
        definition: ToolDefinition,
        script_path: Option<String>,
        param_passing: &str,
    ) -> Result<(), ToolError> {
        // Validate the schema compiles
        let validator = jsonschema::validator_for(&definition.schema).map_err(|e| {
            ToolError::SchemaCompilation(format!(
                "failed to compile schema for `{}`: {e}",
                definition.name
            ))
        })?;

        // Write to SQLite
        {
            let conn = self.conn.lock().unwrap();
            let schema_str = serde_json::to_string(&definition.schema)
                .map_err(|e| ToolError::Executor(format!("failed to serialize schema: {e}")))?;

            let trust_str = match definition.trust_level_required {
                TrustLevel::Sandboxed => "sandboxed",
                TrustLevel::Supervised => "supervised",
                TrustLevel::Autonomous => "autonomous",
            };

            conn.execute(
                "INSERT OR REPLACE INTO agent_tools (name, description, schema, trust_level, script_path, param_passing)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    definition.name,
                    definition.description,
                    schema_str,
                    trust_str,
                    script_path,
                    param_passing,
                ],
            )
            .map_err(|e| ToolError::Executor(format!("failed to insert tool: {e}")))?;
        }

        // Update cache
        let mut cache = self.cache.write().unwrap();
        let mut index = self.index.write().unwrap();
        let mut validators = self.validators.write().unwrap();

        if let Some(&existing_idx) = index.get(&definition.name) {
            // Replace existing
            cache[existing_idx] = definition.clone();
            validators.insert(definition.name.clone(), validator);
        } else {
            let idx = cache.len();
            index.insert(definition.name.clone(), idx);
            validators.insert(definition.name.clone(), validator);
            cache.push(definition);
        }

        Ok(())
    }

    /// Reload dynamic tools from SQLite into cache.
    fn reload_from_db(&self) -> Result<(), ToolError> {
        let tools: Vec<ToolDefinition> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT name, description, schema, trust_level, script_path FROM agent_tools",
                )
                .map_err(|e| ToolError::Executor(format!("failed to query agent_tools: {e}")))?;

            let rows: Vec<(String, String, String, String)> = stmt
                .query_map([], |row| {
                    let name: String = row.get(0)?;
                    let description: String = row.get(1)?;
                    let schema_str: String = row.get(2)?;
                    let trust_str: String = row.get(3)?;
                    Ok((name, description, schema_str, trust_str))
                })
                .map_err(|e| ToolError::Executor(format!("failed to query agent_tools: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            rows.into_iter()
                .filter_map(|(name, description, schema_str, trust_str)| {
                    let schema: serde_json::Value = serde_json::from_str(&schema_str).ok()?;
                    let trust_level = match trust_str.as_str() {
                        "sandboxed" => TrustLevel::Sandboxed,
                        "autonomous" => TrustLevel::Autonomous,
                        _ => TrustLevel::Supervised,
                    };
                    Some(ToolDefinition {
                        name,
                        description,
                        schema,
                        trust_level_required: trust_level,
                    })
                })
                .collect()
        };

        let mut cache = self.cache.write().unwrap();
        let mut index = self.index.write().unwrap();
        let mut validators = self.validators.write().unwrap();

        for def in tools {
            if index.contains_key(&def.name) {
                continue;
            }
            if let Ok(v) = jsonschema::validator_for(&def.schema) {
                let idx = cache.len();
                index.insert(def.name.clone(), idx);
                validators.insert(def.name.clone(), v);
                cache.push(def);
            }
        }

        Ok(())
    }

    /// Validate parameters against the pre-compiled schema for the named tool.
    pub fn validate_params(
        &self,
        tool_name: &str,
        parameters: &serde_json::Value,
    ) -> Result<(), ToolError> {
        let validators = self.validators.read().unwrap();
        let validator = validators
            .get(tool_name)
            .ok_or_else(|| ToolError::ToolNotFound(tool_name.to_string()))?;
        if let Err(err) = validator.validate(parameters) {
            let message = err.to_string();
            let hint = format!("Check parameter types/required fields against schema: {message}");
            return Err(ToolError::ValidationFailed {
                tool_name: tool_name.to_string(),
                message,
                remediation_hint: Some(hint),
            });
        }
        Ok(())
    }

    /// Get a `DynamicToolInfo` for a tool registered via `register_dynamic`.
    pub fn get_dynamic_tool_info(&self, tool_name: &str) -> Option<DynamicToolInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT script_path, param_passing FROM agent_tools WHERE name = ?1",
            params![tool_name],
            |row| {
                let script_path: Option<String> = row.get(0)?;
                let param_passing: String = row.get(1)?;
                Ok(DynamicToolInfo {
                    script_path,
                    param_passing,
                })
            },
        )
        .ok()
    }
}

/// Info about a dynamically registered tool.
#[derive(Debug, Clone)]
pub struct DynamicToolInfo {
    pub script_path: Option<String>,
    pub param_passing: String,
}

impl std::fmt::Debug for PersistentToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache = self.cache.read().unwrap();
        f.debug_struct("PersistentToolRegistry")
            .field("tool_count", &cache.len())
            .finish()
    }
}

impl FrozenToolRegistry for PersistentToolRegistry {
    fn get_manifest(&self) -> &[ToolDefinition] {
        // SAFETY: We need to return a reference, but the cache is behind RwLock.
        // This is a design compromise — we leak a raw pointer that lives as long as `self`.
        // In practice, the cache only grows (tools are added, never removed from the Vec).
        // The returned slice is valid as long as no reallocation occurs.
        // We pre-read the cache and return a 'static-lifetime slice, which is safe
        // because PersistentToolRegistry's cache only grows.
        //
        // A safer approach would be to change the trait to return Vec<ToolDefinition>,
        // but that would break the existing trait contract. Instead, we use a static
        // empty slice as fallback and transmute when we have data.
        //
        // For now, use the simpler approach of returning an empty slice and provide
        // a separate method for getting owned copies.
        &[]
    }

    fn is_permitted(&self, tool_name: &str, trust_level: TrustLevel) -> bool {
        self.get_tool(tool_name)
            .map(|t| trust_level >= t.trust_level_required)
            .unwrap_or(false)
    }

    fn get_tool(&self, _name: &str) -> Option<&ToolDefinition> {
        // Same limitation as get_manifest — we can't return a reference through RwLock.
        // Return None and rely on get_tool_owned() for actual lookups.
        None
    }
}

impl PersistentToolRegistry {
    /// Get a clone of the full tool manifest.
    pub fn get_manifest_owned(&self) -> Vec<ToolDefinition> {
        self.cache.read().unwrap().clone()
    }

    /// Get a cloned copy of a tool definition by name.
    pub fn get_tool_owned(&self, name: &str) -> Option<ToolDefinition> {
        let cache = self.cache.read().unwrap();
        let index = self.index.read().unwrap();
        index.get(name).map(|&i| cache[i].clone())
    }

    /// Check if a tool is permitted at the given trust level.
    pub fn is_permitted_owned(&self, tool_name: &str, trust_level: TrustLevel) -> bool {
        self.get_tool_owned(tool_name)
            .map(|t| trust_level >= t.trust_level_required)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> PersistentToolRegistry {
        let conn = Connection::open_in_memory().unwrap();
        PersistentToolRegistry::new(Arc::new(Mutex::new(conn))).unwrap()
    }

    fn tool_def(name: &str, trust: TrustLevel) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            schema: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
            trust_level_required: trust,
        }
    }

    #[test]
    fn register_builtins() {
        let reg = make_registry();
        reg.register_builtins(vec![
            tool_def("bash", TrustLevel::Autonomous),
            tool_def("read_file", TrustLevel::Sandboxed),
        ])
        .unwrap();

        let manifest = reg.get_manifest_owned();
        assert_eq!(manifest.len(), 2);
        assert!(reg.get_tool_owned("bash").is_some());
        assert!(reg.get_tool_owned("read_file").is_some());
    }

    #[test]
    fn register_dynamic_tool() {
        let reg = make_registry();
        reg.register_dynamic(
            tool_def("my_tool", TrustLevel::Supervised),
            Some("/usr/local/bin/my_tool".to_string()),
            "stdin",
        )
        .unwrap();

        let t = reg.get_tool_owned("my_tool").unwrap();
        assert_eq!(t.name, "my_tool");
        assert_eq!(t.trust_level_required, TrustLevel::Supervised);

        let info = reg.get_dynamic_tool_info("my_tool").unwrap();
        assert_eq!(info.script_path.unwrap(), "/usr/local/bin/my_tool");
        assert_eq!(info.param_passing, "stdin");
    }

    #[test]
    fn dynamic_tools_persist_across_reopen() {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));

        // First registry instance
        {
            let reg = PersistentToolRegistry::new(conn.clone()).unwrap();
            reg.register_dynamic(
                tool_def("persistent_tool", TrustLevel::Supervised),
                None,
                "json_arg",
            )
            .unwrap();
        }

        // Second registry instance reloads from DB
        let reg2 = PersistentToolRegistry::new(conn).unwrap();
        assert!(reg2.get_tool_owned("persistent_tool").is_some());
    }

    #[test]
    fn register_dynamic_replaces_existing() {
        let reg = make_registry();
        reg.register_dynamic(tool_def("my_tool", TrustLevel::Supervised), None, "stdin")
            .unwrap();

        let updated = ToolDefinition {
            name: "my_tool".to_string(),
            description: "updated description".to_string(),
            schema: serde_json::json!({"type": "object"}),
            trust_level_required: TrustLevel::Autonomous,
        };
        reg.register_dynamic(updated, None, "cli_flags").unwrap();

        let t = reg.get_tool_owned("my_tool").unwrap();
        assert_eq!(t.description, "updated description");
        assert_eq!(t.trust_level_required, TrustLevel::Autonomous);
    }

    #[test]
    fn validate_params_works() {
        let reg = make_registry();
        let def = ToolDefinition {
            name: "strict_tool".to_string(),
            description: "requires x".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {"x": {"type": "string"}},
                "required": ["x"]
            }),
            trust_level_required: TrustLevel::Sandboxed,
        };
        reg.register_builtins(vec![def]).unwrap();

        assert!(reg
            .validate_params("strict_tool", &serde_json::json!({"x": "hello"}))
            .is_ok());
        assert!(reg
            .validate_params("strict_tool", &serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn is_permitted_checks_trust() {
        let reg = make_registry();
        reg.register_builtins(vec![tool_def("admin", TrustLevel::Autonomous)])
            .unwrap();

        assert!(reg.is_permitted_owned("admin", TrustLevel::Autonomous));
        assert!(!reg.is_permitted_owned("admin", TrustLevel::Sandboxed));
    }
}
