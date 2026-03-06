//! Error types for the Sub-Agent Orchestrator.

use stratum_types::RunId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("depth limit exceeded: current depth {current}, limit {limit}")]
    DepthLimitExceeded { current: u32, limit: u32 },

    #[error("run not found: {0}")]
    RunNotFound(RunId),

    #[error("sub-agent failed: run {run_id}: {reason}")]
    SubAgentFailed { run_id: RunId, reason: String },

    #[error("dispatch error: {0}")]
    Dispatch(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("trajectory error: {0}")]
    Trajectory(String),

    #[error("timeout awaiting result for run {0}")]
    Timeout(RunId),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
