//! Global memory tier: SQLite + FTS5 with promotion queue.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use stratum_types::{MemoryEntry, MemorySearchResult, MemoryTier};

use super::{build_entry, parse_datetime, parse_metadata, TierBackend};
use crate::error::MemoryError;
use crate::search::{build_fts5_query, normalize_bm25_score};

/// Status of a promotion request in the global promotion queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionStatus {
    Pending,
    Approved,
    Rejected,
}

impl std::fmt::Display for PromotionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Approved => write!(f, "approved"),
            Self::Rejected => write!(f, "rejected"),
        }
    }
}

/// A promotion request in the queue.
#[derive(Debug, Clone)]
pub struct PromotionRequest {
    pub id: String,
    pub entry_id: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub requested_at: DateTime<Utc>,
    pub status: PromotionStatus,
}

pub(crate) struct GlobalTier {
    conn: Arc<Mutex<Connection>>,
}

impl GlobalTier {
    /// Create a new tier, initializing schema (WAL, tables, FTS5, promotion queue).
    pub(crate) fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, MemoryError> {
        {
            let c = conn.lock().unwrap();
            c.execute_batch("PRAGMA journal_mode=WAL;")?;
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS global_entries (
                    id         TEXT PRIMARY KEY,
                    content    TEXT NOT NULL,
                    metadata   TEXT NOT NULL DEFAULT '{}',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS global_fts USING fts5(
                    content,
                    content_rowid='rowid'
                );
                CREATE TABLE IF NOT EXISTS promotion_queue (
                    id           TEXT PRIMARY KEY,
                    entry_id     TEXT NOT NULL,
                    content      TEXT NOT NULL,
                    metadata     TEXT NOT NULL DEFAULT '{}',
                    requested_at TEXT NOT NULL,
                    status       TEXT NOT NULL DEFAULT 'pending'
                );",
            )?;
        }
        Ok(Self { conn })
    }

    /// Wrap an existing connection whose schema is already initialized.
    pub(crate) fn wrap(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Queue a memory entry for global promotion (requires approval).
    pub(crate) fn queue_promotion(&self, entry: &MemoryEntry) -> Result<String, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let queue_id = uuid::Uuid::new_v4().to_string();
        let metadata = serde_json::to_string(&entry.metadata)?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO promotion_queue (id, entry_id, content, metadata, requested_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            params![queue_id, entry.id, entry.content, metadata, now],
        )?;

        Ok(queue_id)
    }

    /// Approve a pending promotion, writing the entry to global storage.
    pub(crate) fn approve_promotion(&self, queue_id: &str) -> Result<(), MemoryError> {
        let conn = self.conn.lock().unwrap();

        let (entry_id, content, metadata_str): (String, String, String) = conn
            .query_row(
                "SELECT entry_id, content, metadata FROM promotion_queue WHERE id = ?1 AND status = 'pending'",
                params![queue_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| MemoryError::EntryNotFound {
                tier: MemoryTier::Global,
                id: queue_id.to_string(),
            })?;

        conn.execute(
            "UPDATE promotion_queue SET status = 'approved' WHERE id = ?1",
            params![queue_id],
        )?;

        // Write to global storage
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO global_entries (id, content, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![entry_id, content, metadata_str, now],
        )?;

        conn.execute(
            "INSERT INTO global_fts (content) VALUES (?1)",
            params![content],
        )?;

        Ok(())
    }

    /// Reject a pending promotion.
    pub(crate) fn reject_promotion(&self, queue_id: &str) -> Result<(), MemoryError> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE promotion_queue SET status = 'rejected' WHERE id = ?1 AND status = 'pending'",
            params![queue_id],
        )?;
        if affected == 0 {
            return Err(MemoryError::EntryNotFound {
                tier: MemoryTier::Global,
                id: queue_id.to_string(),
            });
        }
        Ok(())
    }

    /// List all pending promotion requests.
    pub(crate) fn pending_promotions(&self) -> Result<Vec<PromotionRequest>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, entry_id, content, metadata, requested_at, status
             FROM promotion_queue WHERE status = 'pending'",
        )?;

        let results = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let entry_id: String = row.get(1)?;
                let content: String = row.get(2)?;
                let metadata_str: String = row.get(3)?;
                let requested_str: String = row.get(4)?;

                Ok(PromotionRequest {
                    id,
                    entry_id,
                    content,
                    metadata: parse_metadata(&metadata_str),
                    requested_at: parse_datetime(&requested_str),
                    status: PromotionStatus::Pending,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(results)
    }

    pub(crate) fn connection(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }
}

impl TierBackend for GlobalTier {
    fn read(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, content, metadata, created_at, updated_at FROM global_entries WHERE id = ?1",
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
                MemoryTier::Global,
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

        // Remove old FTS entry if exists
        let old_content: Option<String> = conn
            .query_row(
                "SELECT content FROM global_entries WHERE id = ?1",
                params![entry.id],
                |row| row.get(0),
            )
            .ok();
        if let Some(old) = old_content {
            conn.execute("DELETE FROM global_fts WHERE content = ?1", params![old])?;
        }

        conn.execute(
            "INSERT OR REPLACE INTO global_entries (id, content, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![entry.id, entry.content, metadata, created, updated],
        )?;

        conn.execute(
            "INSERT INTO global_fts (content) VALUES (?1)",
            params![entry.content],
        )?;

        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<MemorySearchResult>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let fts_query = build_fts5_query(query);

        let mut stmt = conn.prepare(
            "SELECT g.id, g.content, g.metadata, g.created_at, g.updated_at, bm25(global_fts) as score
             FROM global_fts f
             JOIN global_entries g ON g.content = f.content
             WHERE global_fts MATCH ?1
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
                        MemoryTier::Global,
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
        let old_content: Option<String> = conn
            .query_row(
                "SELECT content FROM global_entries WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        if let Some(content) = old_content {
            conn.execute(
                "DELETE FROM global_fts WHERE content = ?1",
                params![content],
            )?;
        }
        conn.execute("DELETE FROM global_entries WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tier() -> GlobalTier {
        let conn = Connection::open_in_memory().unwrap();
        GlobalTier::new(Arc::new(Mutex::new(conn))).unwrap()
    }

    fn make_entry(id: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            tier: MemoryTier::Global,
            content: content.to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_global_crud() {
        let tier = make_tier();
        let entry = make_entry("g1", "global knowledge");

        tier.write(&entry).unwrap();
        let read = tier.read("g1").unwrap().unwrap();
        assert_eq!(read.content, "global knowledge");
        assert_eq!(read.tier, MemoryTier::Global);

        tier.delete("g1").unwrap();
        assert!(tier.read("g1").unwrap().is_none());
    }

    #[test]
    fn test_global_fts5_search() {
        let tier = make_tier();
        tier.write(&make_entry("g1", "machine learning algorithms"))
            .unwrap();
        tier.write(&make_entry("g2", "database optimization"))
            .unwrap();

        let results = tier.search("machine learning", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, "g1");
    }

    #[test]
    fn test_promotion_queue_lifecycle() {
        let tier = make_tier();
        let entry = make_entry("p1", "promote this content");

        // Queue
        let queue_id = tier.queue_promotion(&entry).unwrap();

        // Verify pending
        let pending = tier.pending_promotions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entry_id, "p1");
        assert_eq!(pending[0].status, PromotionStatus::Pending);

        // Approve
        tier.approve_promotion(&queue_id).unwrap();

        // Now it should be in global storage
        let read = tier.read("p1").unwrap().unwrap();
        assert_eq!(read.content, "promote this content");

        // Pending should be empty
        let pending = tier.pending_promotions().unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_promotion_queue_reject() {
        let tier = make_tier();
        let entry = make_entry("p2", "reject this");

        let queue_id = tier.queue_promotion(&entry).unwrap();
        tier.reject_promotion(&queue_id).unwrap();

        // Should not be in global storage
        assert!(tier.read("p2").unwrap().is_none());

        // Pending should be empty
        let pending = tier.pending_promotions().unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_promotion_approve_nonexistent() {
        let tier = make_tier();
        let result = tier.approve_promotion("nonexistent");
        assert!(result.is_err());
    }
}
