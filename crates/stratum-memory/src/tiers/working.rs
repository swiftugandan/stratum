//! Working memory tier: in-memory HashMap with substring search.

use std::collections::HashMap;
use std::sync::RwLock;

use stratum_types::{MemoryEntry, MemorySearchResult};

use super::TierBackend;
use crate::error::MemoryError;

pub(crate) struct WorkingTier {
    entries: RwLock<HashMap<String, MemoryEntry>>,
}

impl WorkingTier {
    pub(crate) fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Clear all working memory entries (between-turn cleanup).
    pub(crate) fn clear(&self) {
        self.entries.write().unwrap().clear();
    }
}

impl TierBackend for WorkingTier {
    fn read(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let entries = self.entries.read().unwrap();
        Ok(entries.get(id).cloned())
    }

    fn write(&self, entry: &MemoryEntry) -> Result<(), MemoryError> {
        let mut entries = self.entries.write().unwrap();
        entries.insert(entry.id.clone(), entry.clone());
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<MemorySearchResult>, MemoryError> {
        let entries = self.entries.read().unwrap();
        let query_lower = query.to_lowercase();

        let mut results: Vec<MemorySearchResult> = entries
            .values()
            .filter(|e| e.content.to_lowercase().contains(&query_lower))
            .map(|e| MemorySearchResult {
                entry: e.clone(),
                relevance_score: 1.0, // Simple substring match — no ranking
            })
            .collect();

        results.truncate(limit);
        Ok(results)
    }

    fn delete(&self, id: &str) -> Result<(), MemoryError> {
        let mut entries = self.entries.write().unwrap();
        entries.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use stratum_types::MemoryTier;

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

    #[test]
    fn test_working_crud() {
        let tier = WorkingTier::new();
        let entry = make_entry("w1", "hello world");

        tier.write(&entry).unwrap();
        let read = tier.read("w1").unwrap().unwrap();
        assert_eq!(read.content, "hello world");

        tier.delete("w1").unwrap();
        assert!(tier.read("w1").unwrap().is_none());
    }

    #[test]
    fn test_working_search() {
        let tier = WorkingTier::new();
        tier.write(&make_entry("w1", "rust programming")).unwrap();
        tier.write(&make_entry("w2", "python scripting")).unwrap();
        tier.write(&make_entry("w3", "Rust is great")).unwrap();

        let results = tier.search("rust", 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_working_clear() {
        let tier = WorkingTier::new();
        tier.write(&make_entry("w1", "data")).unwrap();
        tier.write(&make_entry("w2", "data")).unwrap();

        tier.clear();
        assert!(tier.read("w1").unwrap().is_none());
        assert!(tier.read("w2").unwrap().is_none());
    }

    #[test]
    fn test_working_overwrite() {
        let tier = WorkingTier::new();
        tier.write(&make_entry("w1", "original")).unwrap();
        tier.write(&make_entry("w1", "updated")).unwrap();

        let read = tier.read("w1").unwrap().unwrap();
        assert_eq!(read.content, "updated");
    }

    #[test]
    fn test_working_search_limit() {
        let tier = WorkingTier::new();
        for i in 0..10 {
            tier.write(&make_entry(&format!("w{i}"), "same content"))
                .unwrap();
        }
        let results = tier.search("same", 3).unwrap();
        assert_eq!(results.len(), 3);
    }
}
