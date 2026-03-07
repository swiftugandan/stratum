//! `create_skill` built-in: agent creates a new skill as a Markdown file with YAML frontmatter.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::executor::ToolExecutionError;

#[derive(Debug, Clone)]
pub struct CreateSkillParams {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub content: String,
}

/// Extract triggers array from a JSON value containing a "triggers" field.
pub fn parse_triggers(val: &Value) -> Vec<String> {
    val.get("triggers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub fn validate_create_skill_params(
    parameters: &Value,
) -> Result<CreateSkillParams, ToolExecutionError> {
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

    let triggers = parse_triggers(parameters);

    let content = parameters
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolExecutionError::simple("missing required parameter: content"))?
        .to_string();

    Ok(CreateSkillParams {
        name,
        description,
        triggers,
        content,
    })
}

pub async fn write_skill_file(
    skills_dir: &Path,
    params: &CreateSkillParams,
) -> Result<PathBuf, ToolExecutionError> {
    tokio::fs::create_dir_all(skills_dir)
        .await
        .map_err(|e| ToolExecutionError::simple(format!("failed to create skills dir: {e}")))?;

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
        .map_err(|e| ToolExecutionError::simple(format!("failed to write skill file: {e}")))?;

    Ok(file_path)
}

pub fn create_skill_definition() -> stratum_core::ToolDefinition {
    stratum_core::ToolDefinition {
        name: "create_skill".to_string(),
        description:
            "Create a new skill as a Markdown file. Skills provide context for matching tasks."
                .to_string(),
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
            triggers: vec!["test".to_string()],
            content: "# Test Skill\nContent here.".to_string(),
        };

        let path = write_skill_file(dir.path(), &params).await.unwrap();
        assert!(path.exists());

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("name: test-skill"));
    }
}
