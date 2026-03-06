//! stratum-orchestrator: Sub-Agent Orchestrator implementation (Stratum 5).
//!
//! Owns sub-agent spawning, rfbmq-backed task dispatch, spawn patterns,
//! and depth bounds.

pub mod config;
pub mod dispatch;
pub mod error;
pub mod orchestrator;

// Re-export port traits from stratum-core.
pub use stratum_core::{Orchestrator, TaskDispatch};

// Re-export concrete types.
pub use config::OrchestratorConfig;
pub use dispatch::RfbmqDispatcher;
pub use error::OrchestratorError;
pub use orchestrator::DefaultOrchestrator;
