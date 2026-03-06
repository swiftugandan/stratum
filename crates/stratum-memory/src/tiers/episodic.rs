//! Episodic memory tier: SQLite + FTS5, run-scoped.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use stratum_types::{MemoryEntry, MemorySearchResult, MemoryTier};

use super::{build_entry, TierBackend};
use crate::error::MemoryError;
use crate::search::{build_fts5_query, normalize_bm25_score};

pub(crate) struct EpisodicTier {
    conn: Arc<Mutex<Connection>>,
}

impl EpisodicTier {
    /// Create a new tier, initializing schema (WAL, tables, FTS5).
    pub(crate) fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, MemoryError> {
        {
            let c = conn.lock().unwrap();
            c.execute_batch("PRAGMA journal_mode=WAL;")?;
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS episodic_entries (
                    id         TEXT PRIMARY KEY,
                    content    TEXT NOT NULL,
                    metadata   TEXT NOT NULL DEFAULT '{}',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS episodic_fts USING fts5(
                    content,
                    content_rowid='rowid'
                );",
            )?;
        }
        Ok(Self { conn })
    }

    /// Wrap an existing connection whose schema is already initialized.
    pub(crate) fn wrap(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    pub(crate) fn connection(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }
}

impl TierBackend for EpisodicTier {
    fn read(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, content, metadata, created_at, updated_at FROM episodic_entries WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata_str: String = row.get(2)?;
            let created_str: String = row.get(3)?;
            let updated_str: String = row.get(4)?;
            Ok(Some(build_entry(
                id,
                MemoryTier::Episodic,
                content,
                &metadata_str,
                &created_str,
                &updated_str,
            )))
        } else {
            Ok(None)
        }
    }

    fn write(&self, entry: &MemoryEntry) -> Result<(), MemoryError> {
        let conn = self.conn.lock().unwrap();
        let metadata = serde_json::to_string(&entry.metadata)?;
        let created = entry.created_at.to_rfc3339();
        let updated = entry.updated_at.to_rfc3339();

        // Check if entry exists for FTS update
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM episodic_entries WHERE id = ?1",
                params![entry.id],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)?;

        if exists {
            // Delete old FTS entry
            let old_content: String = conn.query_row(
                "SELECT content FROM episodic_entries WHERE id = ?1",
                params![entry.id],
                |row| row.get(0),
            )?;
            conn.execute(
                "DELETE FROM episodic_fts WHERE content = ?1",
                params![old_content],
            )?;
        }

        conn.execute(
            "INSERT OR REPLACE INTO episodic_entries (id, content, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![entry.id, entry.content, metadata, created, updated],
        )?;

        conn.execute(
            "INSERT INTO episodic_fts (content) VALUES (?1)",
            params![entry.content],
        )?;

        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<MemorySearchResult>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let fts_query = build_fts5_query(query);

        let mut stmt = conn.prepare(
            "SELECT e.id, e.content, e.metadata, e.created_at, e.updated_at, bm25(episodic_fts) as score
             FROM episodic_fts f
             JOIN episodic_entries e ON e.content = f.content
             WHERE episodic_fts MATCH ?1
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
                        MemoryTier::Episodic,
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
        let conn = self.conn.lock().unwrap();
        // Delete FTS entry first
        let old_content: Option<String> = conn
            .query_row(
                "SELECT content FROM episodic_entries WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        if let Some(content) = old_content {
            conn.execute(
                "DELETE FROM episodic_fts WHERE content = ?1",
                params![content],
            )?;
        }
        conn.execute("DELETE FROM episodic_entries WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn make_tier() -> EpisodicTier {
        let conn = Connection::open_in_memory().unwrap();
        EpisodicTier::new(Arc::new(Mutex::new(conn))).unwrap()
    }

    fn make_entry(id: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            tier: MemoryTier::Episodic,
            content: content.to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_episodic_crud() {
        let tier = make_tier();
        let entry = make_entry("e1", "episodic memory content");

        tier.write(&entry).unwrap();
        let read = tier.read("e1").unwrap().unwrap();
        assert_eq!(read.content, "episodic memory content");

        tier.delete("e1").unwrap();
        assert!(tier.read("e1").unwrap().is_none());
    }

    #[test]
    fn test_episodic_fts5_search() {
        let tier = make_tier();
        tier.write(&make_entry("e1", "rust programming language"))
            .unwrap();
        tier.write(&make_entry("e2", "python programming language"))
            .unwrap();
        tier.write(&make_entry("e3", "javascript framework"))
            .unwrap();

        let results = tier.search("programming", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].relevance_score > 0.0);
    }

    #[test]
    fn test_episodic_fts5_ranking() {
        let tier = make_tier();
        tier.write(&make_entry("e1", "rust rust rust programming"))
            .unwrap();
        tier.write(&make_entry("e2", "rust programming")).unwrap();

        let results = tier.search("rust", 10).unwrap();
        assert_eq!(results.len(), 2);
        // Entry with more occurrences should score higher
        assert!(results[0].relevance_score >= results[1].relevance_score);
    }

    #[test]
    fn test_episodic_empty_db() {
        let tier = make_tier();
        let results = tier.search("anything", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_episodic_overwrite() {
        let tier = make_tier();
        tier.write(&make_entry("e1", "original content")).unwrap();
        tier.write(&make_entry("e1", "updated content")).unwrap();

        let read = tier.read("e1").unwrap().unwrap();
        assert_eq!(read.content, "updated content");

        // Search should find updated content
        let results = tier.search("updated", 10).unwrap();
        assert_eq!(results.len(), 1);

        // Search should not find original content
        let results = tier.search("original", 10).unwrap();
        assert!(results.is_empty());
    }
}
