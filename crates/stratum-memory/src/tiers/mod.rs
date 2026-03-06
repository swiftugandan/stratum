//! Internal tier backend implementations.

use chrono::{DateTime, Utc};
use stratum_types::{MemoryEntry, MemorySearchResult, MemoryTier};

use crate::error::MemoryError;

pub(crate) mod episodic;
pub(crate) mod global;
pub(crate) mod project;
pub(crate) mod working;

/// Internal trait for tier-specific storage backends.
///
/// All methods are synchronous; the outer `DefaultMemoryStore` wraps them
/// in `spawn_blocking` where needed.
pub(crate) trait TierBackend: Send + Sync {
    fn read(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    fn write(&self, entry: &MemoryEntry) -> Result<(), MemoryError>;
    fn search(&self, query: &str, limit: usize) -> Result<Vec<MemorySearchResult>, MemoryError>;
    #[allow(dead_code)]
    fn delete(&self, id: &str) -> Result<(), MemoryError>;
}

/// Parse an RFC 3339 datetime string, falling back to `Utc::now()` on failure.
pub(crate) fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Parse a JSON metadata string, falling back to `{}` on failure.
pub(crate) fn parse_metadata(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap_or(serde_json::json!({}))
}

/// Build a `MemoryEntry` from raw SQLite row fields.
pub(crate) fn build_entry(
    id: String,
    tier: MemoryTier,
    content: String,
    metadata_str: &str,
    created_str: &str,
    updated_str: &str,
) -> MemoryEntry {
    MemoryEntry {
        id,
        tier,
        content,
        metadata: parse_metadata(metadata_str),
        created_at: parse_datetime(created_str),
        updated_at: parse_datetime(updated_str),
    }
}
