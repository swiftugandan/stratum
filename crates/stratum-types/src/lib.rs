use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Run Identity (PRD Section 8: StratumRun)
// ---------------------------------------------------------------------------

pub type RunId = Uuid;
pub type ModelRef = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TrustLevel {
    Sandboxed,
    Supervised,
    Autonomous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunState {
    Initialising,
    Running,
    Paused,
    Checkpointed,
    Resuming,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StratumRun {
    pub id: RunId,
    pub parent_run_id: Option<RunId>,
    pub model_ref: ModelRef,
    pub trust_level: TrustLevel,
    pub tool_manifest: Vec<String>,
    pub memory_config: MemoryConfig,
    pub hitl_policy: HitlPolicy,
    pub context_budget: ContextBudget,
    pub spawn_depth_limit: u32,
    pub state: RunState,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Task Manifest (PRD Section 8: TaskManifest)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskManifest {
    pub goal: String,
    pub acceptance_criteria: Vec<Criterion>,
    pub progress: Vec<ProgressItem>,
    pub decisions: Vec<Decision>,
    pub blockers: Vec<Blocker>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    pub id: String,
    pub description: String,
    pub met: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressItem {
    pub id: String,
    pub description: String,
    pub status: ProgressStatus,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgressStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub timestamp: DateTime<Utc>,
    pub context: String,
    pub choice: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocker {
    pub id: String,
    pub description: String,
    pub resolved: bool,
}

// ---------------------------------------------------------------------------
// Run Artefacts (Review 3.2: two-prompt pattern contract)
// ---------------------------------------------------------------------------

/// The four artefacts produced by the Initialiser Prompt.
/// Validates the contract between model output and harness expectations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtefacts {
    pub task_md: String,
    pub progress_md: String,
    pub decisions_md: String,
    pub init_sh: Option<String>,
}

impl RunArtefacts {
    pub fn validate(&self) -> Result<(), ArtefactValidationError> {
        if self.task_md.trim().is_empty() {
            return Err(ArtefactValidationError::MissingArtefact("TASK.md"));
        }
        if self.progress_md.trim().is_empty() {
            return Err(ArtefactValidationError::MissingArtefact("PROGRESS.md"));
        }
        if self.decisions_md.trim().is_empty() {
            return Err(ArtefactValidationError::MissingArtefact("DECISIONS.md"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtefactValidationError {
    MissingArtefact(&'static str),
}

impl std::fmt::Display for ArtefactValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingArtefact(name) => write!(f, "missing required artefact: {name}"),
        }
    }
}

impl std::error::Error for ArtefactValidationError {}

// ---------------------------------------------------------------------------
// Context Budget (PRD Section 8: ContextBudget)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub system_anchor: u32,
    pub task_manifest: u32,
    pub injected_knowledge: u32,
    pub tool_results: u32,
    pub history: u32,
    pub total_ceiling: u32,
    /// Fraction of total_ceiling that triggers compaction (default: 0.85).
    pub compaction_threshold: f32,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            system_anchor: 4_000,
            task_manifest: 2_000,
            injected_knowledge: 8_000,
            tool_results: 16_000,
            history: 32_000,
            total_ceiling: 128_000,
            compaction_threshold: 0.85,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool Result (PRD Section 8: ToolResult)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_name: String,
    pub status: ToolResultStatus,
    pub output: serde_json::Value,
    pub remediation_hint: Option<String>,
    pub latency_ms: u64,
    pub cached_tokens_used: u64,
    pub uncached_tokens_used: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolResultStatus {
    Success,
    ValidationFailure,
    ExecutionError,
    PolicyDenied,
}

// ---------------------------------------------------------------------------
// Tool Definition & Invocation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
    pub trust_level_required: TrustLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub parameters: serde_json::Value,
    pub run_id: RunId,
}

// ---------------------------------------------------------------------------
// Constraint Enforcement (Review 3.3: dedicated type for constraint tools)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintDefinition {
    pub name: String,
    pub description: String,
    /// The tool to invoke for checking this constraint (linter, test runner, etc.)
    pub check_tool: String,
    pub severity: ConstraintSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintResult {
    pub constraint_name: String,
    pub passed: bool,
    pub violations: Vec<ConstraintViolation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintViolation {
    pub location: String,
    pub message: String,
    pub remediation: String,
    pub severity: ConstraintSeverity,
}

// ---------------------------------------------------------------------------
// Trajectory Event (PRD Section 8: TrajectoryEvent)
//
// Review 2.4: EventType is a flat enum of unit variants.
// All event-specific data goes into TrajectoryEvent::payload.
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
    pub token_cost: TokenCost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StratumLayer {
    SessionLifecycle,
    ContextEngine,
    MemoryHierarchy,
    ToolGateway,
    SubAgentOrchestrator,
    HitlController,
    TrajectoryStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCost {
    pub cached_tokens: u64,
    pub uncached_tokens: u64,
    pub estimated_cost_usd: f64,
}

impl Default for TokenCost {
    fn default() -> Self {
        Self {
            cached_tokens: 0,
            uncached_tokens: 0,
            estimated_cost_usd: 0.0,
        }
    }
}

/// All event types emitted by the harness.
/// Flat enum -- no data variants. Event-specific data belongs in `payload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    // Session (Stratum 1)
    RunCreated,
    RunStarted,
    CheckpointWritten,
    RunResumed,
    RunCompleted,
    RunFailed,
    RunAborted,

    // Context (Stratum 2)
    CompactionStage1Triggered,
    CompactionStage2Triggered,
    CompactionStage3Triggered,
    HygieneScoreDegraded,
    ReanchorInjected,
    TodoRecitationInjected,

    // Memory (Stratum 3)
    MemoryWritten,
    MemoryPromoted,
    MemorySearched,
    SkillLoaded,

    // Tools (Stratum 4)
    ToolCalled,
    ToolValidated,
    ToolExecuted,
    ToolFailed,
    ToolRetried,
    ConstraintViolated,

    // Sub-agents (Stratum 5)
    SubagentSpawned,
    SubagentCompleted,
    SubagentFailed,
    JanitorRunStarted,

    // HITL (Stratum 6)
    GateOpened,
    GateDecisionReceived,
    RunPaused,
    RunRedirected,
}

// ---------------------------------------------------------------------------
// Memory types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryTier {
    Working,
    Episodic,
    Project,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub episodic_backend: String,
    pub project_root: Option<String>,
    pub global_backend: String,
    pub search_strategy: SearchStrategy,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            episodic_backend: "sqlite".to_string(),
            project_root: None,
            global_backend: "sqlite-vec".to_string(),
            search_strategy: SearchStrategy::Hybrid { vector_weight: 0.7 },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchStrategy {
    Vector,
    Bm25,
    Hybrid { vector_weight: f32 },
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
// HITL types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlPolicy {
    pub destructive: GatePolicy,
    pub irreversible: GatePolicy,
    pub trust_escalation: GatePolicy,
    pub ambiguity: GatePolicy,
    pub drift: GatePolicy,
    pub budget: GatePolicy,
    pub scheduled_interval: Option<u64>,
}

impl Default for HitlPolicy {
    fn default() -> Self {
        Self {
            destructive: GatePolicy::AlwaysAsk,
            irreversible: GatePolicy::AlwaysAsk,
            trust_escalation: GatePolicy::AlwaysAsk,
            ambiguity: GatePolicy::AlwaysAsk,
            drift: GatePolicy::NotifyAndOption,
            budget: GatePolicy::Notify,
            scheduled_interval: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatePolicy {
    AlwaysAsk,
    NotifyAndOption,
    Notify,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HitlDecision {
    Approve,
    Modify { context: String },
    Redirect { new_goal: String },
    Abort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlRecord {
    pub id: String,
    pub run_id: RunId,
    pub gate_category: String,
    pub action_attempted: String,
    pub alternatives: Vec<String>,
    pub context_summary: String,
    pub decision: Option<HitlDecision>,
}

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: Uuid,
    pub run_id: RunId,
    pub state: RunState,
    pub task_manifest: TaskManifest,
    pub context_summary: String,
    pub tool_call_log: Vec<ToolResult>,
    pub sub_agent_tree: Vec<RunId>,
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
    pub trust_level: TrustLevel,
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
// Context Engine types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembledContext {
    pub system_anchor: String,
    pub task_manifest: String,
    pub injected_knowledge: Vec<String>,
    pub tool_results: Vec<String>,
    pub history: String,
    pub total_tokens: u64,
    /// Per-slot token counts, populated during assembly to avoid re-tokenisation.
    pub slot_tokens: SlotTokenCounts,
}

/// Pre-computed token counts for each context slot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlotTokenCounts {
    pub system_anchor: u64,
    pub task_manifest: u64,
    pub injected_knowledge: u64,
    pub tool_results: u64,
    pub history: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum BudgetStatus {
    WithinBudget,
    ApproachingThreshold { utilisation: f32 },
    OverBudget { overage_tokens: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionStage {
    Offload,
    Truncate,
    Summarise,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HygieneScore {
    pub score: f32,
    pub on_task_fraction: f32,
    pub consecutive_degraded: u32,
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
    /// Why the model stopped: `end_turn`, `stop`, `max_tokens`, `tool_use`, `tool_calls`, etc.
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    /// Provider-assigned ID for this tool call (needed to send `tool_result` back).
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
// Export format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Jsonl,
    Csv,
    Replay,
}

#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_context_budget() {
        let budget = ContextBudget::default();
        assert_eq!(budget.total_ceiling, 128_000);
        assert!((budget.compaction_threshold - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn test_default_hitl_policy() {
        let policy = HitlPolicy::default();
        assert_eq!(policy.destructive, GatePolicy::AlwaysAsk);
        assert_eq!(policy.budget, GatePolicy::Notify);
    }

    #[test]
    fn test_event_type_is_flat() {
        // EventType should serialize as a simple string, no embedded data
        let event = EventType::MemoryWritten;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#""MemoryWritten""#);
    }

    #[test]
    fn test_run_artefacts_validation() {
        let valid = RunArtefacts {
            task_md: "# Goal\nBuild a thing".to_string(),
            progress_md: "- [ ] Step 1".to_string(),
            decisions_md: "No decisions yet.".to_string(),
            init_sh: None,
        };
        assert!(valid.validate().is_ok());

        let missing_task = RunArtefacts {
            task_md: "".to_string(),
            progress_md: "content".to_string(),
            decisions_md: "content".to_string(),
            init_sh: None,
        };
        assert_eq!(
            missing_task.validate().unwrap_err(),
            ArtefactValidationError::MissingArtefact("TASK.md")
        );
    }

    #[test]
    fn test_token_cost_default() {
        let cost = TokenCost::default();
        assert_eq!(cost.cached_tokens, 0);
        assert_eq!(cost.uncached_tokens, 0);
    }
}
