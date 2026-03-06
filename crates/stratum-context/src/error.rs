//! Error types for the Context Engine.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("trajectory error: {0}")]
    Trajectory(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("compaction failed at stage {stage:?}: {message}")]
    Compaction {
        stage: stratum_types::CompactionStage,
        message: String,
    },
}
