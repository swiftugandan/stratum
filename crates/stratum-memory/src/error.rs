//! Error types for the Memory Hierarchy.

use stratum_types::MemoryTier;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("entry not found in {tier:?}: {id}")]
    EntryNotFound { tier: MemoryTier, id: String },

    #[error("invalid promotion: {0}")]
    InvalidPromotion(String),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("YAML parse error: {0}")]
    Yaml(String),

    #[error("trajectory error: {0}")]
    Trajectory(String),

    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("vector search not implemented — use Bm25 strategy")]
    VectorNotImplemented,
}
