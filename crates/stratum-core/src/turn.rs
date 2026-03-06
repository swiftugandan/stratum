//! TurnExecutor: the application service that orchestrates a single agent turn.
//!
//! Review 2.3: The SAD described each stratum's internals but never specified
//! the orchestration that ties a single agent turn together. This is that
//! missing piece -- the use case that coordinates the ports.

use async_trait::async_trait;
use stratum_types::*;

/// The outcome of executing a single turn.
#[derive(Debug, Clone)]
pub enum TurnOutcome {
    /// Model produced a text response (no tool calls).
    Response { content: String },
    /// Model requested tool calls; results are returned for the next turn.
    ToolCalls { results: Vec<ToolResult> },
    /// The run should be paused (HITL gate triggered).
    Paused { gate: HitlRecord },
    /// The run is complete (model signalled completion).
    Completed,
    /// An error occurred during the turn.
    Error { message: String },
}

/// Orchestrates a single agent turn by wiring strata together.
///
/// The turn lifecycle:
/// 1. Context Engine assembles context for the model
/// 2. Check budget, trigger compaction if needed
/// 3. Optionally inject todo recitation
/// 4. Call the LLM
/// 5. If tool calls: route through Tool Gateway
/// 6. If HITL gate triggered: pause via HITL Controller
/// 7. Emit trajectory events throughout
/// 8. Checkpoint if needed
#[async_trait]
pub trait TurnExecutor: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Execute a single turn for the given run.
    async fn execute_turn(&self, run_id: RunId) -> Result<TurnOutcome, Self::Error>;
}
