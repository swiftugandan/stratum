//! `create_skill` built-in: agent creates a new skill as a Markdown file with YAML frontmatter.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::executor::ToolExecutionError;

fn err(msg: impl Into<String>) -> ToolExecutionError {
    ToolExecutionError::simple(msg)
}

/// Parsed parameters for the create_skill built-in.
#[derive(Debug, Clone)]
pub struct CreateSkillParams {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub content: String,
}

/// Validate and parse create_skill parameters.
pub fn validate_create_skill_params(
    parameters: &Value,
) -> Result<CreateSkillParams, ToolExecutionError> {
    let name = parameters
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: name"))?
        .to_string();

    let description = parameters
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: description"))?
        .to_string();

    let triggers: Vec<String> = parameters
        .get("triggers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let content = parameters
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: content"))?
        .to_string();

    Ok(CreateSkillParams {
        name,
        description,
        triggers,
        content,
    })
}

/// Write a skill as YAML-frontmatter Markdown to the skills directory.
pub async fn write_skill_file(
    skills_dir: &Path,
    params: &CreateSkillParams,
) -> Result<PathBuf, ToolExecutionError> {
    tokio::fs::create_dir_all(skills_dir)
        .await
        .map_err(|e| err(format!("failed to create skills dir: {e}")))?;

    let file_path = skills_dir.join(format!("{}.md", params.name));

    let triggers_yaml = if params.triggers.is_empty() {
        "triggers: []".to_string()
    } else {
        let items: Vec<String> = params.triggers.iter().map(|t| format!("  - {t}")).collect();
        format!("triggers:\n{}", items.join("\n"))
    };

    let markdown = format!(
        "---\nname: {}\ndescription: {}\n{}\n---\n{}",
        params.name, params.description, triggers_yaml, params.content
    );

    tokio::fs::write(&file_path, &markdown)
        .await
        .map_err(|e| err(format!("failed to write skill file: {e}")))?;

    Ok(file_path)
}

/// Schema definition for the create_skill built-in.
pub fn create_skill_definition() -> stratum_types::ToolDefinition {
    stratum_types::ToolDefinition {
        name: "create_skill".to_string(),
        description: "Create a new skill as a Markdown file. Skills are loaded on next scan and provide context for matching tasks.".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Unique skill name (used as filename)" },
                "description": { "type": "string", "description": "What the skill does" },
                "triggers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Keywords that trigger this skill (optional)"
                },
                "content": { "type": "string", "description": "Skill content (Markdown)" }
            },
            "required": ["name", "description", "content"]
        }),
        trust_level_required: stratum_types::TrustLevel::Autonomous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_valid_params() {
        let params = serde_json::json!({
            "name": "my-skill",
            "description": "A test skill",
            "triggers": ["test", "example"],
            "content": "# My Skill\nContent here.",
        });
        let result = validate_create_skill_params(&params).unwrap();
        assert_eq!(result.name, "my-skill");
        assert_eq!(result.triggers.len(), 2);
    }

    #[test]
    fn validate_no_triggers() {
        let params = serde_json::json!({
            "name": "my-skill",
            "description": "A test skill",
            "content": "Content here.",
        });
        let result = validate_create_skill_params(&params).unwrap();
        assert!(result.triggers.is_empty());
    }

    #[test]
    fn validate_missing_content() {
        let params = serde_json::json!({
            "name": "my-skill",
            "description": "A test skill",
        });
        assert!(validate_create_skill_params(&params).is_err());
    }

    #[tokio::test]
    async fn write_skill_file_creates_valid_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let params = CreateSkillParams {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            triggers: vec!["test".to_string(), "example".to_string()],
            content: "# Test Skill\nContent here.".to_string(),
        };

        let path = write_skill_file(dir.path(), &params).await.unwrap();
        assert!(path.exists());

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("name: test-skill"));
        assert!(content.contains("description: A test skill"));
        assert!(content.contains("  - test"));
        assert!(content.contains("# Test Skill"));
    }

    #[test]
    fn definition_has_correct_trust() {
        let def = create_skill_definition();
        assert_eq!(def.name, "create_skill");
        assert_eq!(
            def.trust_level_required,
            stratum_types::TrustLevel::Autonomous
        );
    }
}
