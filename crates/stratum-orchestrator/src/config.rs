//! Configuration for the Sub-Agent Orchestrator.

use std::path::PathBuf;
use std::time::Duration;

/// Configuration for `DefaultOrchestrator`.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Root directory for queue topology.
    pub queue_root: PathBuf,
    /// Default spawn depth limit for new runs.
    pub default_depth_limit: u32,
    /// Hard maximum depth (overrides per-run limits).
    pub hard_max_depth: u32,
    /// Maximum pending messages per queue.
    pub max_pending_per_queue: i64,
    /// Timeout for `await_result` polling.
    pub await_timeout: Duration,
    /// Interval between polls in `await_result`.
    pub poll_interval: Duration,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            queue_root: PathBuf::from(".stratum/queues"),
            default_depth_limit: 2,
            hard_max_depth: 5,
            max_pending_per_queue: 1000,
            await_timeout: Duration::from_secs(3600),
            poll_interval: Duration::from_secs(1),
        }
    }
}
