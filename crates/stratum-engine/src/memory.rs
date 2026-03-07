//! 2-tier memory store: in-memory HashMap (Working) + SQLite/FTS5 (Persistent).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use stratum_core::ports::MemoryStore;
use stratum_core::*;

use crate::error::EngineError;

pub struct TwoTierMemoryStore {
    working: RwLock<HashMap<String, MemoryEntry>>,
    conn: Arc<Mutex<Connection>>,
}

impl TwoTierMemoryStore {
    pub fn new(path: &str) -> Result<Self, EngineError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, EngineError> {
        let store = Self {
            working: RwLock::new(HashMap::new()),
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), EngineError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS persistent_memory USING fts5(
                id,
                content,
                metadata
            );

            CREATE TABLE IF NOT EXISTS persistent_memory_meta (
                id         TEXT PRIMARY KEY,
                content    TEXT NOT NULL,
                metadata   TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;
        Ok(())
    }
}

impl std::fmt::Debug for TwoTierMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let wc = self.working.read().unwrap().len();
        f.debug_struct("TwoTierMemoryStore")
            .field("working_count", &wc)
            .finish()
    }
}

#[async_trait]
impl MemoryStore for TwoTierMemoryStore {
    type Error = EngineError;

    async fn write(&self, tier: MemoryTier, entry: &MemoryEntry) -> Result<(), Self::Error> {
        match tier {
            MemoryTier::Working => {
                let mut working = self.working.write().unwrap();
                working.insert(entry.id.clone(), entry.clone());
                Ok(())
            }
            MemoryTier::Persistent => {
                let conn = Arc::clone(&self.conn);
                let entry = entry.clone();
                tokio::task::spawn_blocking(move || {
                    let conn = conn.lock().unwrap();
                    let now = Utc::now().to_rfc3339();
                    let metadata =
                        serde_json::to_string(&entry.metadata).unwrap_or_else(|_| "{}".to_string());

                    let tx = conn.unchecked_transaction()?;

                    // Upsert into meta table
                    tx.execute(
                        "INSERT OR REPLACE INTO persistent_memory_meta
                            (id, content, metadata, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![entry.id, entry.content, metadata, now, now],
                    )?;

                    // Delete old FTS entry if exists, then insert new
                    tx.execute(
                        "DELETE FROM persistent_memory WHERE id = ?1",
                        params![entry.id],
                    )?;
                    tx.execute(
                        "INSERT INTO persistent_memory (id, content, metadata)
                         VALUES (?1, ?2, ?3)",
                        params![entry.id, entry.content, metadata],
                    )?;

                    tx.commit()?;

                    Ok::<(), EngineError>(())
                })
                .await??;
                Ok(())
            }
        }
    }

    async fn search(
        &self,
        tier: MemoryTier,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchResult>, Self::Error> {
        match tier {
            MemoryTier::Working => {
                let working = self.working.read().unwrap();
                let query_lower = query.to_lowercase();
                let mut results: Vec<MemorySearchResult> = working
                    .values()
                    .filter(|e| e.content.to_lowercase().contains(&query_lower))
                    .map(|e| MemorySearchResult {
                        entry: e.clone(),
                        relevance_score: 1.0,
                    })
                    .collect();
                results.truncate(limit);
                Ok(results)
            }
            MemoryTier::Persistent => {
                let conn = Arc::clone(&self.conn);
                let query = query.to_string();
                tokio::task::spawn_blocking(move || {
                    let conn = conn.lock().unwrap();

                    // FTS5 query with BM25 ranking
                    let fts_query = query
                        .split_whitespace()
                        .map(|w| format!("\"{w}\""))
                        .collect::<Vec<_>>()
                        .join(" OR ");

                    let mut stmt = conn.prepare(
                        "SELECT m.id, m.content, m.metadata, m.created_at, m.updated_at,
                                rank
                         FROM persistent_memory f
                         JOIN persistent_memory_meta m ON f.id = m.id
                         WHERE persistent_memory MATCH ?1
                         ORDER BY rank
                         LIMIT ?2",
                    )?;

                    let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, f64>(5)?,
                        ))
                    })?;

                    let mut results = Vec::new();
                    for row in rows {
                        let (id, content, metadata_str, created_at, updated_at, rank) = row?;
                        let metadata: serde_json::Value =
                            serde_json::from_str(&metadata_str).unwrap_or_default();
                        let created = chrono::DateTime::parse_from_rfc3339(&created_at)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now());
                        let updated = chrono::DateTime::parse_from_rfc3339(&updated_at)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now());

                        results.push(MemorySearchResult {
                            entry: MemoryEntry {
                                id,
                                tier: MemoryTier::Persistent,
                                content,
                                metadata,
                                created_at: created,
                                updated_at: updated,
                            },
                            relevance_score: (-rank) as f32, // FTS5 rank is negative
                        });
                    }
                    Ok(results)
                })
                .await?
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            tier: MemoryTier::Working,
            content: content.to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn working_tier_write_and_search() {
        let store = TwoTierMemoryStore::in_memory().unwrap();
        store
            .write(MemoryTier::Working, &make_entry("e1", "rust programming"))
            .await
            .unwrap();
        store
            .write(MemoryTier::Working, &make_entry("e2", "python scripting"))
            .await
            .unwrap();

        let results = store.search(MemoryTier::Working, "rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, "e1");
    }

    #[tokio::test]
    async fn persistent_tier_write_and_search() {
        let store = TwoTierMemoryStore::in_memory().unwrap();
        let entry = MemoryEntry {
            tier: MemoryTier::Persistent,
            ..make_entry("p1", "database optimization techniques")
        };
        store.write(MemoryTier::Persistent, &entry).await.unwrap();

        let results = store
            .search(MemoryTier::Persistent, "database", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, "p1");
    }

    #[tokio::test]
    async fn persistent_tier_upsert() {
        let store = TwoTierMemoryStore::in_memory().unwrap();
        let entry1 = MemoryEntry {
            tier: MemoryTier::Persistent,
            ..make_entry("p1", "old content")
        };
        store.write(MemoryTier::Persistent, &entry1).await.unwrap();

        let entry2 = MemoryEntry {
            tier: MemoryTier::Persistent,
            ..make_entry("p1", "new content about databases")
        };
        store.write(MemoryTier::Persistent, &entry2).await.unwrap();

        let results = store
            .search(MemoryTier::Persistent, "databases", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.content, "new content about databases");
    }

    #[tokio::test]
    async fn working_empty_search() {
        let store = TwoTierMemoryStore::in_memory().unwrap();
        let results = store
            .search(MemoryTier::Working, "nothing", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn persistent_empty_search() {
        let store = TwoTierMemoryStore::in_memory().unwrap();
        let results = store
            .search(MemoryTier::Persistent, "nothing", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn working_search_case_insensitive() {
        let store = TwoTierMemoryStore::in_memory().unwrap();
        store
            .write(MemoryTier::Working, &make_entry("e1", "Rust Programming"))
            .await
            .unwrap();

        let results = store.search(MemoryTier::Working, "rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }
}
