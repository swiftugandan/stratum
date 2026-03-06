//! `FilesystemSkillLoader` — loads skills from `.stratum/skills/*.md` with YAML frontmatter.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use async_trait::async_trait;
use serde::Deserialize;
use stratum_core::SkillLoader;
use stratum_types::Skill;

use crate::error::MemoryError;

/// YAML frontmatter parsed from skill Markdown files.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    triggers: Vec<String>,
}

/// Loads skills from Markdown files with YAML frontmatter.
///
/// File format:
/// ```text
/// ---
/// name: my-skill
/// description: Does something useful
/// triggers:
///   - keyword1
///   - keyword2
/// ---
/// # Skill content here
/// ```
pub struct FilesystemSkillLoader {
    skills_dir: PathBuf,
    /// Cached manifest: (name, description) pairs.
    manifest: RwLock<Vec<(String, String)>>,
    /// Cached parsed skills.
    cache: RwLock<HashMap<String, Skill>>,
}

impl FilesystemSkillLoader {
    /// Create a new skill loader for the given directory.
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            manifest: RwLock::new(Vec::new()),
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Scan the skills directory and populate the manifest.
    pub fn scan(&self) -> Result<(), MemoryError> {
        let mut manifest = self.manifest.write().unwrap();
        let mut cache = self.cache.write().unwrap();

        manifest.clear();
        cache.clear();

        if !self.skills_dir.exists() {
            return Ok(());
        }

        let entries = std::fs::read_dir(&self.skills_dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                match self.parse_skill_file(&path) {
                    Ok(skill) => {
                        manifest.push((skill.name.clone(), skill.description.clone()));
                        cache.insert(skill.name.clone(), skill);
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "failed to parse skill file");
                    }
                }
            }
        }

        Ok(())
    }

    fn parse_skill_file(&self, path: &std::path::Path) -> Result<Skill, MemoryError> {
        let content = std::fs::read_to_string(path)?;
        Self::parse_skill_content(&content)
    }

    fn parse_skill_content(content: &str) -> Result<Skill, MemoryError> {
        // Parse YAML frontmatter between --- delimiters
        let trimmed = content.trim();
        if !trimmed.starts_with("---") {
            return Err(MemoryError::Yaml(
                "skill file must start with YAML frontmatter (---)".to_string(),
            ));
        }

        let after_first = &trimmed[3..];
        let end_idx = after_first.find("---").ok_or_else(|| {
            MemoryError::Yaml("missing closing --- for YAML frontmatter".to_string())
        })?;

        let yaml_str = &after_first[..end_idx];
        let body = after_first[end_idx + 3..].trim();

        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(yaml_str).map_err(|e| MemoryError::Yaml(e.to_string()))?;

        Ok(Skill {
            name: frontmatter.name,
            description: frontmatter.description,
            trigger_conditions: frontmatter.triggers,
            content: body.to_string(),
        })
    }
}

impl std::fmt::Debug for FilesystemSkillLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesystemSkillLoader")
            .field("skills_dir", &self.skills_dir)
            .finish()
    }
}

#[async_trait]
impl SkillLoader for FilesystemSkillLoader {
    type Error = MemoryError;

    async fn list_skills(&self) -> Result<Vec<(String, String)>, Self::Error> {
        let manifest = self.manifest.read().unwrap();
        Ok(manifest.clone())
    }

    async fn load_skill(&self, name: &str) -> Result<Option<Skill>, Self::Error> {
        let cache = self.cache.read().unwrap();
        Ok(cache.get(name).cloned())
    }

    async fn match_skills(&self, task_context: &str) -> Result<Vec<String>, Self::Error> {
        let cache = self.cache.read().unwrap();
        let context_lower = task_context.to_lowercase();

        let mut matched: Vec<String> = cache
            .values()
            .filter(|skill| {
                skill
                    .trigger_conditions
                    .iter()
                    .any(|trigger| context_lower.contains(&trigger.to_lowercase()))
            })
            .map(|skill| skill.name.clone())
            .collect();

        matched.sort();
        Ok(matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_content() {
        let content = r#"---
name: test-skill
description: A test skill
triggers:
  - test
  - testing
---
# Test Skill

This is the skill content.
"#;
        let skill = FilesystemSkillLoader::parse_skill_content(content).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.trigger_conditions, vec!["test", "testing"]);
        assert!(skill.content.contains("# Test Skill"));
    }

    #[test]
    fn test_parse_skill_no_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here.";
        let result = FilesystemSkillLoader::parse_skill_content(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_skill_incomplete_frontmatter() {
        let content = "---\nname: broken\n";
        let result = FilesystemSkillLoader::parse_skill_content(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_skill_no_triggers() {
        let content = r#"---
name: no-triggers
description: Skill without triggers
---
Content here.
"#;
        let skill = FilesystemSkillLoader::parse_skill_content(content).unwrap();
        assert!(skill.trigger_conditions.is_empty());
    }

    #[test]
    fn test_scan_missing_dir() {
        let loader = FilesystemSkillLoader::new(PathBuf::from("/nonexistent/path/skills"));
        // scan should succeed silently when dir doesn't exist
        assert!(loader.scan().is_ok());
    }

    #[test]
    fn test_scan_with_files() {
        let dir = tempfile::tempdir().unwrap();
        let skill_content = r#"---
name: file-skill
description: Loaded from file
triggers:
  - file
---
File skill content.
"#;
        std::fs::write(dir.path().join("file-skill.md"), skill_content).unwrap();

        let loader = FilesystemSkillLoader::new(dir.path().to_path_buf());
        loader.scan().unwrap();

        let manifest = loader.manifest.read().unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].0, "file-skill");
    }

    #[tokio::test]
    async fn test_match_skills() {
        let dir = tempfile::tempdir().unwrap();

        let skill1 = r#"---
name: rust-skill
description: Rust related
triggers:
  - rust
  - cargo
---
Rust content.
"#;
        let skill2 = r#"---
name: python-skill
description: Python related
triggers:
  - python
  - pip
---
Python content.
"#;
        std::fs::write(dir.path().join("rust.md"), skill1).unwrap();
        std::fs::write(dir.path().join("python.md"), skill2).unwrap();

        let loader = FilesystemSkillLoader::new(dir.path().to_path_buf());
        loader.scan().unwrap();

        let matches = loader
            .match_skills("I need to build with Rust")
            .await
            .unwrap();
        assert_eq!(matches, vec!["rust-skill"]);

        let matches = loader.match_skills("use pip to install").await.unwrap();
        assert_eq!(matches, vec!["python-skill"]);

        let matches = loader.match_skills("something unrelated").await.unwrap();
        assert!(matches.is_empty());
    }
}
