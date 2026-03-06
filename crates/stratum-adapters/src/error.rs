use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),
}
