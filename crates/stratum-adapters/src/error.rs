use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrajectoryStoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Failed to deserialize row data: {0}")]
    RowDeserialization(String),
}
