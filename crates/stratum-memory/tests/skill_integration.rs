//! Integration tests for FilesystemSkillLoader.

use std::path::PathBuf;

use stratum_core::SkillLoader;
use stratum_memory::FilesystemSkillLoader;

fn write_skill(dir: &std::path::Path, filename: &str, content: &str) {
    std::fs::write(dir.join(filename), content).unwrap();
}

fn make_loader_with_skills() -> (tempfile::TempDir, FilesystemSkillLoader) {
    let dir = tempfile::tempdir().unwrap();

    write_skill(
        dir.path(),
        "code-review.md",
        r#"---
name: code-review
description: Performs code review analysis
triggers:
  - review
  - code quality
---
# Code Review Skill

Analyze code for quality issues.
"#,
    );

    write_skill(
        dir.path(),
        "testing.md",
        r#"---
name: testing
description: Generates unit tests
triggers:
  - test
  - testing
  - unit test
---
# Testing Skill

Generate comprehensive tests.
"#,
    );

    let loader = FilesystemSkillLoader::new(dir.path().to_path_buf());
    loader.scan().unwrap();
    (dir, loader)
}

#[tokio::test]
async fn test_scan_and_list_skills() {
    let (_dir, loader) = make_loader_with_skills();

    let skills = loader.list_skills().await.unwrap();
    assert_eq!(skills.len(), 2);

    let names: Vec<&str> = skills.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"code-review"));
    assert!(names.contains(&"testing"));
}

#[tokio::test]
async fn test_load_skill() {
    let (_dir, loader) = make_loader_with_skills();

    let skill = loader.load_skill("code-review").await.unwrap().unwrap();
    assert_eq!(skill.name, "code-review");
    assert_eq!(skill.description, "Performs code review analysis");
    assert!(skill.content.contains("# Code Review Skill"));
    assert_eq!(skill.trigger_conditions, vec!["review", "code quality"]);
}

#[tokio::test]
async fn test_load_nonexistent_skill() {
    let (_dir, loader) = make_loader_with_skills();

    let skill = loader.load_skill("nonexistent").await.unwrap();
    assert!(skill.is_none());
}

#[tokio::test]
async fn test_match_skills() {
    let (_dir, loader) = make_loader_with_skills();

    let matches = loader.match_skills("please review the code").await.unwrap();
    assert_eq!(matches, vec!["code-review"]);

    let matches = loader
        .match_skills("write a unit test for this")
        .await
        .unwrap();
    assert_eq!(matches, vec!["testing"]);
}

#[tokio::test]
async fn test_match_skills_case_insensitive() {
    let (_dir, loader) = make_loader_with_skills();

    let matches = loader.match_skills("REVIEW this code").await.unwrap();
    assert_eq!(matches, vec!["code-review"]);
}

#[tokio::test]
async fn test_match_skills_no_match() {
    let (_dir, loader) = make_loader_with_skills();

    let matches = loader.match_skills("deploy to production").await.unwrap();
    assert!(matches.is_empty());
}

#[tokio::test]
async fn test_malformed_yaml() {
    let dir = tempfile::tempdir().unwrap();
    write_skill(
        dir.path(),
        "bad.md",
        r#"---
name: [invalid yaml
---
Content.
"#,
    );

    let loader = FilesystemSkillLoader::new(dir.path().to_path_buf());
    // scan should succeed, skipping the bad file
    loader.scan().unwrap();

    let skills = loader.list_skills().await.unwrap();
    assert!(skills.is_empty());
}

#[tokio::test]
async fn test_missing_dir() {
    let loader = FilesystemSkillLoader::new(PathBuf::from("/tmp/nonexistent_skills_dir_12345"));
    loader.scan().unwrap();

    let skills = loader.list_skills().await.unwrap();
    assert!(skills.is_empty());
}

#[tokio::test]
async fn test_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let loader = FilesystemSkillLoader::new(dir.path().to_path_buf());
    loader.scan().unwrap();

    let skills = loader.list_skills().await.unwrap();
    assert!(skills.is_empty());
}

#[tokio::test]
async fn test_non_md_files_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a skill").unwrap();
    std::fs::write(dir.path().join("data.json"), "{}").unwrap();

    let loader = FilesystemSkillLoader::new(dir.path().to_path_buf());
    loader.scan().unwrap();

    let skills = loader.list_skills().await.unwrap();
    assert!(skills.is_empty());
}
