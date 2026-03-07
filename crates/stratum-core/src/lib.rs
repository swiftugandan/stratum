//! stratum-core: Domain types and port traits for Stratum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Run Identity
// ---------------------------------------------------------------------------

pub type RunId = Uuid;
pub type ModelRef = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunState {
    Running,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StratumRun {
    pub id: RunId,
    pub parent_run_id: Option<RunId>,
    pub model_ref: ModelRef,
    pub tool_manifest: Vec<String>,
    pub context_budget: ContextBudget,
    pub spawn_depth_limit: u32,
    pub state: RunState,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Context Budget (simplified)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub max_tokens: u32,
    pub max_history_messages: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_tokens: 128_000,
            max_history_messages: 50,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub parameters: serde_json::Value,
    pub run_id: RunId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_name: String,
    pub status: ToolResultStatus,
    pub output: serde_json::Value,
    pub remediation_hint: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolResultStatus {
    Success,
    ValidationFailure,
    ExecutionError,
}

// ---------------------------------------------------------------------------
// Trajectory Event
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEvent {
    pub event_id: Uuid,
    pub run_id: RunId,
    pub parent_run_id: Option<RunId>,
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub stratum_layer: StratumLayer,
    pub payload: serde_json::Value,
}

impl TrajectoryEvent {
    pub fn new(
        run_id: RunId,
        parent_run_id: Option<RunId>,
        event_type: EventType,
        stratum_layer: StratumLayer,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            run_id,
            parent_run_id,
            timestamp: Utc::now(),
            event_type,
            stratum_layer,
            payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StratumLayer {
    Session,
    Context,
    Memory,
    Tools,
    Orchestrator,
    Trajectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    // Session
    RunCreated,
    RunStarted,
    CheckpointWritten,
    RunCompleted,
    RunFailed,
    RunAborted,

    // Core loop
    LlmCompleted,

    // Memory
    MemoryWritten,
    MemorySearched,

    // Tools
    ToolCalled,
    ToolValidated,
    ToolExecuted,
    ToolFailed,
    ToolRetried,

    // Sub-agents
    SubagentSpawned,
    SubagentCompleted,
    SubagentFailed,
    JanitorRunStarted,

    // Daemon
    DaemonStarted,
    DaemonStopped,
    TaskDequeued,
    TaskCompleted,
    ToolCreated,
    SkillCreated,
}

// ---------------------------------------------------------------------------
// Memory types (2-tier: Working + Persistent)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryTier {
    Working,
    Persistent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub tier: MemoryTier,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResult {
    pub entry: MemoryEntry,
    pub relevance_score: f32,
}

// ---------------------------------------------------------------------------
// Checkpoint (simplified)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: Uuid,
    pub run_id: RunId,
    pub state: RunState,
    pub goal: String,
    pub context_summary: String,
    pub tool_call_log: Vec<ToolResult>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Orchestrator types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnPattern {
    Delegate,
    Pipeline,
    Parallel,
    Janitor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnConfig {
    pub pattern: SpawnPattern,
    pub model_ref: ModelRef,
    pub tool_manifest: Vec<String>,
    pub task_goal: String,
    pub shared_filesystem_scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub run_id: RunId,
    pub status: RunState,
    pub summary: String,
    pub artefacts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DispatchOptions {
    pub priority: TaskPriority,
    pub tags: Vec<String>,
    pub correlation_id: Option<String>,
    pub reply_to: Option<String>,
    pub depends_on: Vec<String>,
    pub ttl: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedTask {
    pub id: String,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskPriority {
    Critical,
    High,
    Normal,
    Low,
}

impl Default for TaskPriority {
    fn default() -> Self {
        Self::Normal
    }
}

// ---------------------------------------------------------------------------
// LLM types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<LlmToolCall>,
    pub usage: LlmUsage,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: Option<String>,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

// ---------------------------------------------------------------------------
// Skill type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub trigger_conditions: Vec<String>,
    pub content: String,
}

// ---------------------------------------------------------------------------
// Turn outcome
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum TurnOutcome {
    Response { content: String },
    ToolCalls { results: Vec<ToolResult> },
    Completed,
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Port Traits
// ---------------------------------------------------------------------------

pub mod ports {
    use super::*;
    use async_trait::async_trait;

    #[async_trait]
    pub trait SessionManager: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error>;
        async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error>;
        async fn transition_state(
            &self,
            run_id: RunId,
            new_state: RunState,
        ) -> Result<(), Self::Error>;
        async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error>;
        async fn get_latest_checkpoint(
            &self,
            run_id: RunId,
        ) -> Result<Option<Checkpoint>, Self::Error>;
    }

    #[async_trait]
    pub trait MemoryStore: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn write(&self, tier: MemoryTier, entry: &MemoryEntry) -> Result<(), Self::Error>;
        async fn search(
            &self,
            tier: MemoryTier,
            query: &str,
            limit: usize,
        ) -> Result<Vec<MemorySearchResult>, Self::Error>;
    }

    #[async_trait]
    pub trait ToolGateway: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn call_tool(&self, invocation: ToolInvocation) -> Result<ToolResult, Self::Error>;
    }

    #[async_trait]
    pub trait Orchestrator: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn spawn(
            &self,
            parent_run_id: RunId,
            config: SpawnConfig,
        ) -> Result<RunId, Self::Error>;
        async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error>;
        fn current_depth(&self, run_id: RunId) -> Result<u32, Self::Error>;
    }

    pub trait TaskDispatch: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        fn enqueue(&self, body: &str, opts: DispatchOptions) -> Result<String, Self::Error>;
        fn list_ready(&self) -> Result<Vec<String>, Self::Error>;
        fn dequeue(&self) -> Result<Option<ClaimedTask>, Self::Error>;
        fn complete(&self, task: &ClaimedTask) -> Result<(), Self::Error>;
        fn fail(&self, task: &ClaimedTask) -> Result<(), Self::Error>;
        fn depth(&self) -> Result<i64, Self::Error>;
    }

    #[async_trait]
    pub trait TrajectoryStore: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn emit_event(&self, event: TrajectoryEvent) -> Result<(), Self::Error>;
        async fn query_events(
            &self,
            run_id: Option<RunId>,
            event_type: Option<EventType>,
            limit: Option<usize>,
        ) -> Result<Vec<TrajectoryEvent>, Self::Error>;
    }

    #[async_trait]
    pub trait LlmClient: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn complete(
            &self,
            messages: &[LlmMessage],
            model: &str,
            tools: Option<&[ToolDefinition]>,
        ) -> Result<LlmResponse, Self::Error>;
    }

    #[async_trait]
    pub trait TurnExecutor: Send + Sync {
        type Error: std::error::Error + Send + Sync + 'static;

        async fn execute_turn(&self, run_id: RunId) -> Result<TurnOutcome, Self::Error>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_budget() {
        let budget = ContextBudget::default();
        assert_eq!(budget.max_tokens, 128_000);
        assert_eq!(budget.max_history_messages, 50);
    }

    #[test]
    fn event_type_is_flat() {
        let event = EventType::MemoryWritten;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#""MemoryWritten""#);
    }

    #[test]
    fn trajectory_event_new_sets_defaults() {
        let run_id = Uuid::new_v4();
        let payload = serde_json::json!({"key": "value"});
        let event = TrajectoryEvent::new(
            run_id,
            None,
            EventType::RunCreated,
            StratumLayer::Session,
            payload.clone(),
        );
        assert_eq!(event.run_id, run_id);
        assert_eq!(event.event_type, EventType::RunCreated);
        assert_eq!(event.payload, payload);
    }

    #[test]
    fn memory_tier_serializes() {
        let tier = MemoryTier::Working;
        let json = serde_json::to_string(&tier).unwrap();
        assert_eq!(json, r#""Working""#);
    }

    #[test]
    fn run_state_serializes() {
        let state = RunState::Running;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#""Running""#);
    }
}
