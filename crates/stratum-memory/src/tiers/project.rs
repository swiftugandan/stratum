//! Project memory tier: filesystem Markdown + sidecar SQLite FTS5 index.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::{params, Connection};
use stratum_types::{MemoryEntry, MemorySearchResult, MemoryTier};

use super::{build_entry, parse_datetime, parse_metadata, TierBackend};
use crate::error::MemoryError;
use crate::search::{build_fts5_query, normalize_bm25_score};

pub(crate) struct ProjectTier {
    dir: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl ProjectTier {
    /// Create a new tier, initializing the directory, schema, and FTS5 index.
    pub(crate) fn new(dir: PathBuf) -> Result<Self, MemoryError> {
        std::fs::create_dir_all(&dir)?;

        let db_path = dir.join(".index.db");
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS project_index (
                id         TEXT PRIMARY KEY,
                content    TEXT NOT NULL,
                metadata   TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS project_fts USING fts5(
                content,
                content_rowid='rowid'
            );",
        )?;

        let tier = Self {
            dir,
            conn: Arc::new(Mutex::new(conn)),
        };
        tier.init_index()?;
        Ok(tier)
    }

    /// Wrap an existing directory + connection whose schema is already initialized.
    pub(crate) fn wrap(dir: PathBuf, conn: Arc<Mutex<Connection>>) -> Self {
        Self { dir, conn }
    }

    /// Scan directory and rebuild FTS5 index from Markdown files.
    fn init_index(&self) -> Result<(), MemoryError> {
        let conn = self.conn.lock().unwrap();

        // Clear existing FTS entries and rebuild
        conn.execute_batch("DELETE FROM project_fts;")?;

        let mut stmt = conn.prepare("SELECT id, content FROM project_index")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        // Re-check files on disk; remove stale index entries, add missing ones
        drop(stmt);

        // Read all .md files from disk
        let entries = std::fs::read_dir(&self.dir)?;
        let mut on_disk: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let content = std::fs::read_to_string(&path)?;
                    on_disk.insert(stem.to_string(), content);
                }
            }
        }

        // Remove stale rows
        for (id, _) in &rows {
            if !on_disk.contains_key(id) {
                conn.execute("DELETE FROM project_index WHERE id = ?1", params![id])?;
            }
        }

        // Upsert from disk
        for (id, content) in &on_disk {
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT OR REPLACE INTO project_index (id, content, metadata, created_at, updated_at)
                 VALUES (?1, ?2, '{}', ?3, ?3)",
                params![id, content, now],
            )?;
            conn.execute(
                "INSERT INTO project_fts (content) VALUES (?1)",
                params![content],
            )?;
        }

        Ok(())
    }

    fn file_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    pub(crate) fn connection(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }

    pub(crate) fn dir(&self) -> &PathBuf {
        &self.dir
    }
}

impl TierBackend for ProjectTier {
    fn read(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let path = self.file_path(id);
        // Read directly and handle NotFound — avoids TOCTOU race.
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        // Get metadata from index
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT metadata, created_at, updated_at FROM project_index WHERE id = ?1",
            params![id],
            |row| {
                let metadata_str: String = row.get(0)?;
                let created_str: String = row.get(1)?;
                let updated_str: String = row.get(2)?;
                Ok((metadata_str, created_str, updated_str))
            },
        );

        let (metadata, created_at, updated_at) = match result {
            Ok((m, c, u)) => (parse_metadata(&m), parse_datetime(&c), parse_datetime(&u)),
            Err(_) => (serde_json::json!({}), Utc::now(), Utc::now()),
        };

        Ok(Some(MemoryEntry {
            id: id.to_string(),
            tier: MemoryTier::Project,
            content,
            metadata,
            created_at,
            updated_at,
        }))
    }

    fn write(&self, entry: &MemoryEntry) -> Result<(), MemoryError> {
        // Atomic write: write to temp file, then rename
        let target = self.file_path(&entry.id);
        let tmp = self.dir.join(format!(".{}.tmp", entry.id));
        std::fs::write(&tmp, &entry.content)?;
        std::fs::rename(&tmp, &target)?;

        // Update index
        let conn = self.conn.lock().unwrap();
        let metadata = serde_json::to_string(&entry.metadata)?;
        let created = entry.created_at.to_rfc3339();
        let updated = entry.updated_at.to_rfc3339();

        // Remove old FTS entry if exists
        let old_content: Option<String> = conn
            .query_row(
                "SELECT content FROM project_index WHERE id = ?1",
                params![entry.id],
                |row| row.get(0),
            )
            .ok();
        if let Some(old) = old_content {
            conn.execute("DELETE FROM project_fts WHERE content = ?1", params![old])?;
        }

        conn.execute(
            "INSERT OR REPLACE INTO project_index (id, content, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![entry.id, entry.content, metadata, created, updated],
        )?;

        conn.execute(
            "INSERT INTO project_fts (content) VALUES (?1)",
            params![entry.content],
        )?;

        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<MemorySearchResult>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let fts_query = build_fts5_query(query);

        let mut stmt = conn.prepare(
            "SELECT p.id, p.content, p.metadata, p.created_at, p.updated_at, bm25(project_fts) as score
             FROM project_fts f
             JOIN project_index p ON p.content = f.content
             WHERE project_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;

        let results = stmt
            .query_map(params![fts_query, limit as i64], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata_str: String = row.get(2)?;
                let created_str: String = row.get(3)?;
                let updated_str: String = row.get(4)?;
                let raw_score: f64 = row.get(5)?;

                Ok(MemorySearchResult {
                    entry: build_entry(
                        id,
                        MemoryTier::Project,
                        content,
                        &metadata_str,
                        &created_str,
                        &updated_str,
                    ),
                    relevance_score: normalize_bm25_score(raw_score),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(results)
    }

    fn delete(&self, id: &str) -> Result<(), MemoryError> {
        let path = self.file_path(id);
        // Remove directly and handle NotFound — avoids TOCTOU race.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        let conn = self.conn.lock().unwrap();
        let old_content: Option<String> = conn
            .query_row(
                "SELECT content FROM project_index WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        if let Some(content) = old_content {
            conn.execute(
                "DELETE FROM project_fts WHERE content = ?1",
                params![content],
            )?;
        }
        conn.execute("DELETE FROM project_index WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let tier = ProjectTier::new(dir.path().to_path_buf()).unwrap();

        let entry = MemoryEntry {
            id: "test-doc".to_string(),
            tier: MemoryTier::Project,
            content: "# Project Memory\n\nSome content here.".to_string(),
            metadata: serde_json::json!({"tag": "docs"}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        tier.write(&entry).unwrap();

        // Verify file exists on disk
        assert!(dir.path().join("test-doc.md").exists());

        let read = tier.read("test-doc").unwrap().unwrap();
        assert_eq!(read.content, entry.content);
        assert_eq!(read.metadata["tag"], "docs");
    }

    #[test]
    fn test_project_fts5_search() {
        let dir = tempfile::tempdir().unwrap();
        let tier = ProjectTier::new(dir.path().to_path_buf()).unwrap();

        tier.write(&MemoryEntry {
            id: "arch".to_string(),
            tier: MemoryTier::Project,
            content: "architecture decisions for the API layer".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

        tier.write(&MemoryEntry {
            id: "deploy".to_string(),
            tier: MemoryTier::Project,
            content: "deployment configuration for production".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

        let results = tier.search("architecture", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, "arch");
    }

    #[test]
    fn test_project_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let tier = ProjectTier::new(dir.path().to_path_buf()).unwrap();
        assert!(tier.read("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_project_delete() {
        let dir = tempfile::tempdir().unwrap();
        let tier = ProjectTier::new(dir.path().to_path_buf()).unwrap();

        let entry = MemoryEntry {
            id: "to-delete".to_string(),
            tier: MemoryTier::Project,
            content: "temporary content".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        tier.write(&entry).unwrap();
        tier.delete("to-delete").unwrap();

        assert!(tier.read("to-delete").unwrap().is_none());
        assert!(!dir.path().join("to-delete.md").exists());
    }
}
