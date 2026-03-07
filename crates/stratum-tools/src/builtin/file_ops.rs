//! File operation tools: read_file, write_file, list_directory, search_files.
//!
//! All operations use `tokio::fs` directly (no subprocess).

use serde_json::Value;

use crate::executor::ToolExecutionError;

fn err(msg: impl Into<String>) -> ToolExecutionError {
    ToolExecutionError::simple(msg)
}

/// Read the contents of a file with optional offset and limit.
pub async fn execute_read_file(parameters: &Value) -> Result<Value, ToolExecutionError> {
    let path = parameters
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: path"))?;

    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| err(format!("failed to read file `{path}`: {e}")))?;

    let offset = parameters
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let limit = parameters
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let bytes = content.as_bytes();
    let start = offset.min(bytes.len());
    let end = match limit {
        Some(l) => (start + l).min(bytes.len()),
        None => bytes.len(),
    };
    let slice = String::from_utf8_lossy(&bytes[start..end]);

    Ok(serde_json::json!({
        "content": slice,
        "bytes_read": end - start,
        "total_bytes": bytes.len(),
    }))
}

/// Write content to a file, creating parent directories if needed.
pub async fn execute_write_file(parameters: &Value) -> Result<Value, ToolExecutionError> {
    let path = parameters
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: path"))?;

    let content = parameters
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: content"))?;

    // Create parent directories
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| err(format!("failed to create parent dirs for `{path}`: {e}")))?;
    }

    tokio::fs::write(path, content)
        .await
        .map_err(|e| err(format!("failed to write file `{path}`: {e}")))?;

    Ok(serde_json::json!({
        "path": path,
        "bytes_written": content.len(),
    }))
}

/// List files and directories at a path with optional glob pattern.
pub async fn execute_list_directory(parameters: &Value) -> Result<Value, ToolExecutionError> {
    let path = parameters
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: path"))?;

    let pattern = parameters.get("pattern").and_then(|v| v.as_str());

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(path)
        .await
        .map_err(|e| err(format!("failed to list directory `{path}`: {e}")))?;

    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| err(format!("failed to read entry: {e}")))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();

        // Apply glob pattern filter if specified
        if let Some(pat) = pattern {
            if !glob_match(pat, &name) {
                continue;
            }
        }

        let file_type = entry.file_type().await.ok();
        let is_dir = file_type.as_ref().map(|ft| ft.is_dir()).unwrap_or(false);
        let metadata = entry.metadata().await.ok();
        let size = metadata.map(|m| m.len()).unwrap_or(0);

        entries.push(serde_json::json!({
            "name": name,
            "is_directory": is_dir,
            "size": size,
        }));
    }

    entries.sort_by(|a, b| {
        let a_name = a["name"].as_str().unwrap_or("");
        let b_name = b["name"].as_str().unwrap_or("");
        a_name.cmp(b_name)
    });

    Ok(serde_json::json!({
        "path": path,
        "entries": entries,
        "count": entries.len(),
    }))
}

/// Search for a text pattern in files under a directory.
pub async fn execute_search_files(parameters: &Value) -> Result<Value, ToolExecutionError> {
    let path = parameters
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: path"))?;

    let query = parameters
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err("missing required parameter: query"))?;

    let pattern = parameters.get("pattern").and_then(|v| v.as_str());

    let mut matches = Vec::new();
    let max_matches = 100;

    search_recursive(
        std::path::Path::new(path),
        query,
        pattern,
        &mut matches,
        max_matches,
    )
    .await;

    Ok(serde_json::json!({
        "query": query,
        "matches": matches,
        "count": matches.len(),
        "truncated": matches.len() >= max_matches,
    }))
}

async fn search_recursive(
    dir: &std::path::Path,
    query: &str,
    pattern: Option<&str>,
    matches: &mut Vec<Value>,
    max: usize,
) {
    if matches.len() >= max {
        return;
    }

    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return,
    };

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        if matches.len() >= max {
            return;
        }

        let path = entry.path();
        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            Box::pin(search_recursive(&path, query, pattern, matches, max)).await;
            continue;
        }

        if file_type.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(pat) = pattern {
                if !glob_match(pat, &name) {
                    continue;
                }
            }

            // Read file and search
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(_) => continue, // skip binary / unreadable files
            };

            for (line_num, line) in content.lines().enumerate() {
                if matches.len() >= max {
                    return;
                }
                if line.contains(query) {
                    matches.push(serde_json::json!({
                        "file": path.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line,
                    }));
                }
            }
        }
    }
}

/// Simple glob matching supporting `*` and `?` wildcards.
fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();

    glob_match_inner(&p, &n, 0, 0)
}

fn glob_match_inner(pattern: &[char], name: &[char], pi: usize, ni: usize) -> bool {
    if pi == pattern.len() && ni == name.len() {
        return true;
    }
    if pi == pattern.len() {
        return false;
    }

    match pattern[pi] {
        '*' => {
            // Try matching * with 0..n characters
            for skip in 0..=(name.len() - ni) {
                if glob_match_inner(pattern, name, pi + 1, ni + skip) {
                    return true;
                }
            }
            false
        }
        '?' => {
            if ni < name.len() {
                glob_match_inner(pattern, name, pi + 1, ni + 1)
            } else {
                false
            }
        }
        c => {
            if ni < name.len() && name[ni] == c {
                glob_match_inner(pattern, name, pi + 1, ni + 1)
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "lib.rs"));
        assert!(!glob_match("*.rs", "main.py"));
        assert!(glob_match("test_*", "test_foo"));
        assert!(glob_match("?oo", "foo"));
        assert!(!glob_match("?oo", "fooo"));
        assert!(glob_match("*", "anything"));
    }

    #[tokio::test]
    async fn read_file_basic() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world").await.unwrap();

        let params = serde_json::json!({"path": file_path.to_str().unwrap()});
        let result = execute_read_file(&params).await.unwrap();
        assert_eq!(result["content"].as_str().unwrap(), "hello world");
        assert_eq!(result["bytes_read"], 11);
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world").await.unwrap();

        let params =
            serde_json::json!({"path": file_path.to_str().unwrap(), "offset": 6, "limit": 5});
        let result = execute_read_file(&params).await.unwrap();
        assert_eq!(result["content"].as_str().unwrap(), "world");
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let params = serde_json::json!({"path": "/nonexistent/file.txt"});
        let result = execute_read_file(&params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_file_basic() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("out.txt");

        let params =
            serde_json::json!({"path": file_path.to_str().unwrap(), "content": "test content"});
        let result = execute_write_file(&params).await.unwrap();
        assert_eq!(result["bytes_written"], 12);

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "test content");
    }

    #[tokio::test]
    async fn write_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("a/b/c/out.txt");

        let params = serde_json::json!({"path": file_path.to_str().unwrap(), "content": "nested"});
        let result = execute_write_file(&params).await.unwrap();
        assert_eq!(result["bytes_written"], 6);
    }

    #[tokio::test]
    async fn list_directory_basic() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.rs"), "").await.unwrap();
        tokio::fs::create_dir(dir.path().join("subdir"))
            .await
            .unwrap();

        let params = serde_json::json!({"path": dir.path().to_str().unwrap()});
        let result = execute_list_directory(&params).await.unwrap();
        assert_eq!(result["count"], 3);
    }

    #[tokio::test]
    async fn list_directory_with_pattern() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.rs"), "").await.unwrap();

        let params = serde_json::json!({"path": dir.path().to_str().unwrap(), "pattern": "*.rs"});
        let result = execute_list_directory(&params).await.unwrap();
        assert_eq!(result["count"], 1);
    }

    #[tokio::test]
    async fn search_files_basic() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("a.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .await
        .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "no match here")
            .await
            .unwrap();

        let params = serde_json::json!({
            "path": dir.path().to_str().unwrap(),
            "query": "hello",
        });
        let result = execute_search_files(&params).await.unwrap();
        assert_eq!(result["count"], 2);
    }

    #[tokio::test]
    async fn search_files_with_pattern() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.rs"), "fn main() {}")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "fn main() {}")
            .await
            .unwrap();

        let params = serde_json::json!({
            "path": dir.path().to_str().unwrap(),
            "query": "fn main",
            "pattern": "*.rs",
        });
        let result = execute_search_files(&params).await.unwrap();
        assert_eq!(result["count"], 1);
    }
}
