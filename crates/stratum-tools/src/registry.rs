//! In-memory tool registry: builder + frozen (immutable) registry.

use std::collections::{HashMap, HashSet};

use stratum_core::{FrozenToolRegistry, ToolRegistryBuilder};
use stratum_types::{ToolDefinition, TrustLevel};

use crate::error::ToolError;

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Mutable builder for constructing a tool registry before freezing.
#[derive(Debug, Default)]
pub struct InMemoryToolRegistryBuilder {
    tools: Vec<ToolDefinition>,
    names: HashSet<String>,
}

impl ToolRegistryBuilder for InMemoryToolRegistryBuilder {
    type Frozen = InMemoryFrozenToolRegistry;
    type Error = ToolError;

    fn register(&mut self, definition: ToolDefinition) -> Result<(), Self::Error> {
        if !self.names.insert(definition.name.clone()) {
            return Err(ToolError::ValidationFailed {
                tool_name: definition.name,
                message: "duplicate tool name".to_string(),
                remediation_hint: Some("each tool name must be unique".to_string()),
            });
        }
        self.tools.push(definition);
        Ok(())
    }

    fn build(self) -> Result<Self::Frozen, Self::Error> {
        let mut validators = HashMap::with_capacity(self.tools.len());
        for tool in &self.tools {
            let validator = jsonschema::validator_for(&tool.schema).map_err(|e| {
                ToolError::SchemaCompilation(format!(
                    "failed to compile schema for `{}`: {e}",
                    tool.name
                ))
            })?;
            validators.insert(tool.name.clone(), validator);
        }

        let index = self
            .tools
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        Ok(InMemoryFrozenToolRegistry {
            tools: self.tools,
            index,
            validators,
        })
    }
}

// ---------------------------------------------------------------------------
// Frozen Registry
// ---------------------------------------------------------------------------

/// Immutable tool registry, frozen after run initialisation.
///
/// Schema validators are pre-compiled at build time and cached for reuse.
pub struct InMemoryFrozenToolRegistry {
    tools: Vec<ToolDefinition>,
    index: HashMap<String, usize>,
    validators: HashMap<String, jsonschema::Validator>,
}

impl InMemoryFrozenToolRegistry {
    /// Validate parameters against the pre-compiled schema for the named tool.
    /// Returns `Ok(())` if valid, or a `ToolError::ValidationFailed` if invalid.
    pub fn validate_params(
        &self,
        tool_name: &str,
        parameters: &serde_json::Value,
    ) -> Result<(), ToolError> {
        let validator = self
            .validators
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
}

impl std::fmt::Debug for InMemoryFrozenToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryFrozenToolRegistry")
            .field("tools", &self.tools)
            .field("index", &self.index)
            .finish()
    }
}

impl Clone for InMemoryFrozenToolRegistry {
    fn clone(&self) -> Self {
        // Re-compile validators from the cloned tool definitions.
        let mut validators = HashMap::with_capacity(self.tools.len());
        for tool in &self.tools {
            if let Ok(v) = jsonschema::validator_for(&tool.schema) {
                validators.insert(tool.name.clone(), v);
            }
        }
        Self {
            tools: self.tools.clone(),
            index: self.index.clone(),
            validators,
        }
    }
}

impl FrozenToolRegistry for InMemoryFrozenToolRegistry {
    fn get_manifest(&self) -> &[ToolDefinition] {
        &self.tools
    }

    fn is_permitted(&self, tool_name: &str, trust_level: TrustLevel) -> bool {
        self.get_tool(tool_name)
            .map(|t| trust_level >= t.trust_level_required)
            .unwrap_or(false)
    }

    fn get_tool(&self, name: &str) -> Option<&ToolDefinition> {
        self.index.get(name).map(|&i| &self.tools[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_def(name: &str, trust: TrustLevel) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            schema: serde_json::json!({"type": "object"}),
            trust_level_required: trust,
        }
    }

    #[test]
    fn register_and_build() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("read_file", TrustLevel::Sandboxed))
            .unwrap();
        builder
            .register(tool_def("write_file", TrustLevel::Supervised))
            .unwrap();
        let registry = builder.build().unwrap();
        assert_eq!(registry.get_manifest().len(), 2);
    }

    #[test]
    fn duplicate_rejected() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("read_file", TrustLevel::Sandboxed))
            .unwrap();
        let err = builder.register(tool_def("read_file", TrustLevel::Sandboxed));
        assert!(err.is_err());
    }

    #[test]
    fn get_tool_found() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("read_file", TrustLevel::Sandboxed))
            .unwrap();
        let registry = builder.build().unwrap();
        assert!(registry.get_tool("read_file").is_some());
        assert!(registry.get_tool("nonexistent").is_none());
    }

    #[test]
    fn is_permitted_matching_trust() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("read_file", TrustLevel::Sandboxed))
            .unwrap();
        let registry = builder.build().unwrap();
        assert!(registry.is_permitted("read_file", TrustLevel::Sandboxed));
        assert!(registry.is_permitted("read_file", TrustLevel::Supervised));
        assert!(registry.is_permitted("read_file", TrustLevel::Autonomous));
    }

    #[test]
    fn is_permitted_insufficient_trust() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("deploy", TrustLevel::Autonomous))
            .unwrap();
        let registry = builder.build().unwrap();
        assert!(!registry.is_permitted("deploy", TrustLevel::Sandboxed));
        assert!(!registry.is_permitted("deploy", TrustLevel::Supervised));
        assert!(registry.is_permitted("deploy", TrustLevel::Autonomous));
    }

    #[test]
    fn is_permitted_unknown_tool() {
        let builder = InMemoryToolRegistryBuilder::default();
        let registry = builder.build().unwrap();
        assert!(!registry.is_permitted("nonexistent", TrustLevel::Autonomous));
    }

    #[test]
    fn build_empty_registry() {
        let builder = InMemoryToolRegistryBuilder::default();
        let registry = builder.build().unwrap();
        assert!(registry.get_manifest().is_empty());
    }

    #[test]
    fn get_manifest_returns_all() {
        let mut builder = InMemoryToolRegistryBuilder::default();
        builder
            .register(tool_def("a", TrustLevel::Sandboxed))
            .unwrap();
        builder
            .register(tool_def("b", TrustLevel::Supervised))
            .unwrap();
        builder
            .register(tool_def("c", TrustLevel::Autonomous))
            .unwrap();
        let registry = builder.build().unwrap();
        let names: Vec<_> = registry.get_manifest().iter().map(|t| &t.name).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
