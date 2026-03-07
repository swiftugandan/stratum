//! PersistentToolRegistry: SQLite-backed, mutable between turns.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::{params, Connection};
use stratum_core::ToolDefinition;

use crate::error::EngineError;

/// SQLite-backed tool registry that supports dynamic tool registration.
pub struct PersistentToolRegistry {
    conn: Arc<Mutex<Connection>>,
    cache: RwLock<Vec<ToolDefinition>>,
    index: RwLock<HashMap<String, usize>>,
    validators: RwLock<HashMap<String, jsonschema::Validator>>,
}

impl PersistentToolRegistry {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, EngineError> {
        {
            let c = conn.lock().unwrap();
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS agent_tools (
                    name          TEXT PRIMARY KEY,
                    description   TEXT NOT NULL,
                    schema        TEXT NOT NULL,
                    script_path   TEXT,
                    param_passing TEXT NOT NULL DEFAULT 'stdin',
                    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );",
            )?;
        }

        let registry = Self {
            conn,
            cache: RwLock::new(Vec::new()),
            index: RwLock::new(HashMap::new()),
            validators: RwLock::new(HashMap::new()),
        };

        registry.reload_from_db()?;
        Ok(registry)
    }

    pub fn register_builtins(&self, builtins: Vec<ToolDefinition>) -> Result<(), EngineError> {
        let mut cache = self.cache.write().unwrap();
        let mut index = self.index.write().unwrap();
        let mut validators = self.validators.write().unwrap();

        for def in builtins {
            if index.contains_key(&def.name) {
                continue;
            }

            let validator = jsonschema::validator_for(&def.schema).map_err(|e| {
                EngineError::SchemaCompilation(format!(
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

    pub fn register_dynamic(
        &self,
        definition: ToolDefinition,
        script_path: Option<String>,
        param_passing: &str,
    ) -> Result<(), EngineError> {
        let validator = jsonschema::validator_for(&definition.schema).map_err(|e| {
            EngineError::SchemaCompilation(format!(
                "failed to compile schema for `{}`: {e}",
                definition.name
            ))
        })?;

        {
            let conn = self.conn.lock().unwrap();
            let schema_str = serde_json::to_string(&definition.schema)?;

            conn.execute(
                "INSERT OR REPLACE INTO agent_tools (name, description, schema, script_path, param_passing)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    definition.name,
                    definition.description,
                    schema_str,
                    script_path,
                    param_passing,
                ],
            )?;
        }

        let mut cache = self.cache.write().unwrap();
        let mut index = self.index.write().unwrap();
        let mut validators = self.validators.write().unwrap();

        if let Some(&existing_idx) = index.get(&definition.name) {
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

    fn reload_from_db(&self) -> Result<(), EngineError> {
        let tools: Vec<ToolDefinition> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT name, description, schema FROM agent_tools")?;

            let rows: Vec<(String, String, String)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();

            rows.into_iter()
                .filter_map(|(name, description, schema_str)| {
                    let schema: serde_json::Value = serde_json::from_str(&schema_str).ok()?;
                    Some(ToolDefinition {
                        name,
                        description,
                        schema,
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

    pub fn validate_params(
        &self,
        tool_name: &str,
        parameters: &serde_json::Value,
    ) -> Result<(), EngineError> {
        let validators = self.validators.read().unwrap();
        let validator = validators
            .get(tool_name)
            .ok_or_else(|| EngineError::ToolNotFound(tool_name.to_string()))?;
        if let Err(err) = validator.validate(parameters) {
            let message = err.to_string();
            return Err(EngineError::ValidationFailed {
                tool_name: tool_name.to_string(),
                message,
                remediation_hint: None,
            });
        }
        Ok(())
    }

    pub fn get_manifest_owned(&self) -> Vec<ToolDefinition> {
        self.cache.read().unwrap().clone()
    }

    pub fn get_tool_owned(&self, name: &str) -> Option<ToolDefinition> {
        let cache = self.cache.read().unwrap();
        let index = self.index.read().unwrap();
        index.get(name).map(|&i| cache[i].clone())
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> PersistentToolRegistry {
        let conn = Connection::open_in_memory().unwrap();
        PersistentToolRegistry::new(Arc::new(Mutex::new(conn))).unwrap()
    }

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            schema: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        }
    }

    #[test]
    fn register_builtins() {
        let reg = make_registry();
        reg.register_builtins(vec![tool_def("bash"), tool_def("read_file")])
            .unwrap();
        assert_eq!(reg.get_manifest_owned().len(), 2);
        assert!(reg.get_tool_owned("bash").is_some());
    }

    #[test]
    fn register_dynamic_tool() {
        let reg = make_registry();
        reg.register_dynamic(
            tool_def("my_tool"),
            Some("/tmp/tool.sh".to_string()),
            "stdin",
        )
        .unwrap();

        let t = reg.get_tool_owned("my_tool").unwrap();
        assert_eq!(t.name, "my_tool");

        let info = reg.get_dynamic_tool_info("my_tool").unwrap();
        assert_eq!(info.script_path.unwrap(), "/tmp/tool.sh");
    }

    #[test]
    fn dynamic_tools_persist_across_reopen() {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));

        {
            let reg = PersistentToolRegistry::new(conn.clone()).unwrap();
            reg.register_dynamic(tool_def("persistent_tool"), None, "json_arg")
                .unwrap();
        }

        let reg2 = PersistentToolRegistry::new(conn).unwrap();
        assert!(reg2.get_tool_owned("persistent_tool").is_some());
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
        };
        reg.register_builtins(vec![def]).unwrap();

        assert!(reg
            .validate_params("strict_tool", &serde_json::json!({"x": "hello"}))
            .is_ok());
        assert!(reg
            .validate_params("strict_tool", &serde_json::json!({}))
            .is_err());
    }
}
