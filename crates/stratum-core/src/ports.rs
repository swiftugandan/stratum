//! Port traits for all seven strata.
//!
//! Each trait defines a boundary. Implementations live in stratum-adapters
//! or in the stratum-specific crates. This separation follows the review
//! recommendation (Section 2.1) to keep stratum-core as pure abstractions.

use async_trait::async_trait;
use stratum_types::*;

// ---------------------------------------------------------------------------
// Stratum 1: Session Lifecycle Manager
// ---------------------------------------------------------------------------

#[async_trait]
pub trait SessionManager: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error>;
    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error>;
    async fn resume(&self, run_id: RunId) -> Result<StratumRun, Self::Error>;
    async fn transition_state(&self, run_id: RunId, new_state: RunState)
        -> Result<(), Self::Error>;
    async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error>;
    async fn get_latest_checkpoint(&self, run_id: RunId)
        -> Result<Option<Checkpoint>, Self::Error>;
}

// ---------------------------------------------------------------------------
// Stratum 2: Context Engine
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ContextEngine: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn assemble_context(&self, run_id: RunId) -> Result<AssembledContext, Self::Error>;

    fn check_budget(&self, context: &AssembledContext, budget: &ContextBudget) -> BudgetStatus;

    async fn trigger_compaction(
        &self,
        run_id: RunId,
        stage: CompactionStage,
    ) -> Result<(), Self::Error>;

    async fn compute_hygiene_score(
        &self,
        run_id: RunId,
        window_size: usize,
    ) -> Result<HygieneScore, Self::Error>;

    async fn inject_todo_recitation(&self, run_id: RunId) -> Result<(), Self::Error>;
}

// ---------------------------------------------------------------------------
// Stratum 3: Memory Hierarchy
// ---------------------------------------------------------------------------

#[async_trait]
pub trait MemoryStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn read(&self, tier: MemoryTier, id: &str) -> Result<Option<MemoryEntry>, Self::Error>;

    async fn write(&self, tier: MemoryTier, entry: &MemoryEntry) -> Result<(), Self::Error>;

    async fn search(
        &self,
        tier: MemoryTier,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchResult>, Self::Error>;

    async fn promote(
        &self,
        entry_id: &str,
        from: MemoryTier,
        to: MemoryTier,
    ) -> Result<(), Self::Error>;
}

#[async_trait]
pub trait SkillLoader: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn list_skills(&self) -> Result<Vec<(String, String)>, Self::Error>;
    async fn load_skill(&self, name: &str) -> Result<Option<Skill>, Self::Error>;
    async fn match_skills(&self, task_context: &str) -> Result<Vec<String>, Self::Error>;
}

// ---------------------------------------------------------------------------
// Stratum 4: Tool Execution Gateway
//
// Review 2.2: Single `call_tool()` method instead of exposing pipeline steps.
// The intercept-validate-execute-log pipeline is an internal implementation
// detail, not a public API. This eliminates temporal coupling.
//
// Review 3.1: ToolRegistry uses builder pattern -> frozen after init.
// `ToolRegistryBuilder` allows mutation; `FrozenToolRegistry` is read-only.
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ToolGateway: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Execute a tool call through the full intercept-validate-execute-log pipeline.
    /// Callers need not know the pipeline stages or their ordering.
    async fn call_tool(&self, invocation: ToolInvocation) -> Result<ToolResult, Self::Error>;
}

/// Read-only tool registry, frozen after run initialisation.
/// Review 3.1: Enforces KV-cache economics by preventing mid-run tool changes.
pub trait FrozenToolRegistry: Send + Sync {
    fn get_manifest(&self) -> &[ToolDefinition];
    fn is_permitted(&self, tool_name: &str, trust_level: TrustLevel) -> bool;
    fn get_tool(&self, name: &str) -> Option<&ToolDefinition>;
}

/// Builder for constructing a tool registry before freezing it.
pub trait ToolRegistryBuilder {
    type Frozen: FrozenToolRegistry;
    type Error: std::error::Error + Send + Sync + 'static;

    fn register(&mut self, definition: ToolDefinition) -> Result<(), Self::Error>;
    fn build(self) -> Result<Self::Frozen, Self::Error>;
}

// ---------------------------------------------------------------------------
// Constraint Enforcement (Review 3.3)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ConstraintEnforcer: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn constraints(&self) -> &[ConstraintDefinition];

    async fn check_all(&self, run_id: RunId) -> Result<Vec<ConstraintResult>, Self::Error>;

    async fn check_one(
        &self,
        run_id: RunId,
        constraint_name: &str,
    ) -> Result<ConstraintResult, Self::Error>;
}

// ---------------------------------------------------------------------------
// Stratum 5: Sub-Agent Orchestrator
// ---------------------------------------------------------------------------

#[async_trait]
pub trait Orchestrator: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn spawn(&self, parent_run_id: RunId, config: SpawnConfig) -> Result<RunId, Self::Error>;

    async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error>;
    fn current_depth(&self, run_id: RunId) -> Result<u32, Self::Error>;
    fn can_spawn(&self, parent_run_id: RunId) -> Result<bool, Self::Error>;
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

// ---------------------------------------------------------------------------
// Stratum 6: HITL Controller
// ---------------------------------------------------------------------------

#[async_trait]
pub trait HitlController: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn open_gate(&self, record: HitlRecord) -> Result<(), Self::Error>;
    async fn record_decision(
        &self,
        run_id: RunId,
        decision: HitlDecision,
    ) -> Result<(), Self::Error>;
    async fn pending_gates(&self) -> Result<Vec<HitlRecord>, Self::Error>;
    async fn get_decision(
        &self,
        run_id: RunId,
        gate_id: &str,
    ) -> Result<Option<HitlDecision>, Self::Error>;
}

#[async_trait]
pub trait Notifier: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn notify(&self, record: &HitlRecord) -> Result<(), Self::Error>;
}

// ---------------------------------------------------------------------------
// Stratum 7: Trajectory Store & Observability
// ---------------------------------------------------------------------------

#[async_trait]
pub trait TrajectoryStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn emit_event(&self, event: TrajectoryEvent) -> Result<(), Self::Error>;
    async fn query_events(
        &self,
        run_id: Option<RunId>,
        event_type: Option<EventType>,
        time_range: Option<TimeRange>,
        limit: Option<usize>,
    ) -> Result<Vec<TrajectoryEvent>, Self::Error>;
    async fn export(&self, run_id: RunId, format: ExportFormat) -> Result<Vec<u8>, Self::Error>;
}

pub trait MetricsExporter: Send + Sync {
    fn export_metrics(&self) -> String;
    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]);
    fn counter(&self, name: &str, labels: &[(&str, &str)]);
}

// ---------------------------------------------------------------------------
// Cross-cutting: LLM Client
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Artefact Validation (Review 3.2)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ArtefactValidator: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn validate_initialiser_output(
        &self,
        run_id: RunId,
        artefacts: &RunArtefacts,
    ) -> Result<(), Self::Error>;
}
