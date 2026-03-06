# Stratum -- Software Architecture Document v3.0

> **Version 3.0** -- full rewrite aligned with PRD v1.0 seven-stratum architecture.
> Supersedes SAD v2.1 which described a narrower task-queue orchestrator.

---

## 1. Introduction

### 1.1 Purpose

Stratum is a model-agnostic, production-grade agent harness that unifies proven patterns from existing agent systems into seven independently swappable layers (strata). It manages the full agent runtime: session lifecycle, context engineering, memory, tool execution, sub-agent orchestration, human-in-the-loop control, and trajectory capture.

### 1.2 Scope

This document covers the software architecture of Stratum: its decomposition into crates, domain types, port traits for all seven strata, internal design of each stratum, the integration with rfbmq for sub-agent task dispatch, and the deployment profile.

### 1.3 Key Differences from SAD v2.1

| Aspect | SAD v2.1 | SAD v3.0 |
|--------|----------|----------|
| Scope | Task-queue orchestrator (Conductor/Worker) | Full 7-stratum agent harness |
| Crates | 3 (core, adapters, bin) | 9 (types, core, context, memory, tools, orchestrator, adapters, cli, test-utils) |
| Port traits | 3 (Queue, Store, LlmClient) | 15+ covering all 7 strata |
| rfbmq role | Central architectural element | Implementation detail under Stratum 5 |
| Domain types | Task, Plan, Subtask | StratumRun, TaskManifest, ContextBudget, ToolResult, TrajectoryEvent, and more |

---

## 2. Constraints

| ID | Constraint | Rationale |
|----|-----------|-----------|
| C1 | Single static binary | Simplifies deployment; no runtime dependencies |
| C2 | No network daemon | Stratum is a CLI tool suite, not a server |
| C3 | SQLite for persistent state | Proven embedded DB; single-file state |
| C4 | No unsafe code | rfbmq is pure Rust; all dependencies are safe |
| C5 | Tokio async runtime | Required by LLM API clients (reqwest) |
| C6 | Hexagonal / ports-and-adapters | Testability; swap providers without touching core logic |
| C7 | Message format: Markdown with RFC 822 headers | Human-readable, git-friendly, inspectable with standard tools |

---

## 3. ADR-001: Why Rust

### Decision

Stratum is implemented in Rust.

### Context

The harness must be a single static binary with predictable resource usage, suitable for deployment on modest hardware. It must integrate with a filesystem-based message queue and an embedded database while managing concurrent LLM API calls and sub-agent processes.

### Rationale

1. **Single binary deployment** -- `cargo build --release` produces one executable with no runtime dependencies.
2. **Native rfbmq integration** -- rfbmq is written in Rust. Zero FFI, zero unsafe, zero build complexity.
3. **Memory safety without GC** -- Ownership model prevents use-after-free, double-free, and data races at compile time.
4. **Async ecosystem** -- Tokio + reqwest provide ergonomic async HTTP for LLM API calls.
5. **Cross-compilation** -- `cross` or `cargo-zigbuild` can target `aarch64-unknown-linux-musl` for ARM deployment.
6. **Type system expressiveness** -- Trait objects and enums model the port/adapter pattern naturally. Sum types make state machines explicit.

### Alternatives Considered

- **Go**: No sum types for state machines; would need its own queue implementation.
- **Python**: No single-binary story; runtime dependency management is fragile.

---

## 4. Container View

### 4.1 Workspace Structure

```
stratum/
  Cargo.toml                    # workspace root
  crates/
    stratum-types/              # shared domain types
    stratum-core/               # port traits, session lifecycle, HITL, trajectory
    stratum-context/            # context engine
    stratum-memory/             # memory hierarchy
    stratum-tools/              # tool execution gateway
    stratum-orchestrator/       # sub-agent orchestrator + rfbmq
    stratum-adapters/           # external adapters (LLM, SQLite, vector DB)
    stratum-cli/                # CLI binary, wires everything together
    stratum-test-utils/         # shared test infrastructure
```

### 4.2 Crate Responsibilities

| Crate | PRD Stratum | Role | Key Dependencies |
|-------|-------------|------|------------------|
| `stratum-types` | All | Shared domain types: `StratumRun`, `TaskManifest`, `ContextBudget`, `ToolResult`, `TrajectoryEvent`, enums, IDs | uuid, chrono, serde |
| `stratum-core` | 1, 6, 7 | Port traits for all 7 strata. Session Lifecycle Manager, HITL Controller, and Trajectory Store implementations | stratum-types, async-trait |
| `stratum-context` | 2 | Context Engine: budget model, 3-stage compaction, hygiene score, todo recitation | stratum-types, stratum-core |
| `stratum-memory` | 3 | Memory Hierarchy: 4-tier memory, semantic search, skill loading | stratum-types, stratum-core |
| `stratum-tools` | 4 | Tool Execution Gateway: intercept-validate-execute-log pipeline, trust levels, tool manifest | stratum-types, stratum-core |
| `stratum-orchestrator` | 5 | Sub-Agent Orchestrator: spawn patterns, rfbmq-backed task dispatch, depth bounds | stratum-types, stratum-core, rfbmq-core |
| `stratum-adapters` | Cross-cutting | Concrete implementations: LLM clients, SQLite store, vector DB, filesystem memory | stratum-types, stratum-core, reqwest, rusqlite |
| `stratum-cli` | -- | CLI binary, argument parsing, dependency injection, wiring | All crates, clap, tokio |
| `stratum-test-utils` | -- | Test fixtures, mock adapters, assertion helpers | stratum-types, stratum-core |

### 4.3 Dependency Graph

```
stratum-cli
  ├── stratum-core
  │     └── stratum-types
  ├── stratum-context
  │     ├── stratum-core
  │     └── stratum-types
  ├── stratum-memory
  │     ├── stratum-core
  │     └── stratum-types
  ├── stratum-tools
  │     ├── stratum-core
  │     └── stratum-types
  ├── stratum-orchestrator
  │     ├── stratum-core
  │     ├── stratum-types
  │     └── rfbmq-core  (external, Cargo dependency)
  ├── stratum-adapters
  │     ├── stratum-core
  │     ├── stratum-types
  │     ├── rusqlite
  │     ├── reqwest
  │     └── rfbmq-core
  ├── clap
  └── tokio
```

### 4.4 System Context

```
┌──────────────────────────────────────────────────────────────────┐
│                        STRATUM HARNESS                           │
│                                                                  │
│  ┌────────────┐ ┌─────────┐ ┌────────┐ ┌───────┐ ┌───────────┐ │
│  │  Context   │ │ Memory  │ │ Tools  │ │ Orch. │ │   Core    │ │
│  │  Engine    │ │ Hier.   │ │ Gatew. │ │       │ │ (Session, │ │
│  │ (Stratum 2)│ │(Str. 3) │ │(Str. 4)│ │(Str.5)│ │ HITL,     │ │
│  └─────┬──────┘ └────┬────┘ └───┬────┘ └──┬────┘ │ Traject.) │ │
│        │             │          │          │      │ (1, 6, 7) │ │
│        └─────────────┴──────────┴──────────┘      └─────┬─────┘ │
│                          │                              │       │
│                     ┌────┴─────┐                        │       │
│                     │ Adapters │────────────────────────-┘       │
│                     └──┬──┬──┬─┘                                │
│                        │  │  │                                  │
└────────────────────────┼──┼──┼──────────────────────────────────┘
                         │  │  │
                         ▼  ▼  ▼
                    ┌────┐ ┌──┐ ┌────────┐
                    │rfbmq│ │DB│ │LLM API │
                    │queue│ │  │ │(HTTP)  │
                    └────┘ └──┘ └────────┘
                      │      │       │
                      ▼      ▼       ▼
                 Filesystem  SQLite  HTTPS to
                 queue dirs  .db     Claude/OpenAI/etc.
```

---

## 5. Domain Types

All domain types live in `stratum-types` and are derived from PRD Section 8 interface contracts. Types are written in Rust with serde support for serialisation.

### 5.1 Run Identity

```rust
use uuid::Uuid;
use chrono::{DateTime, Utc};

/// Unique identifier for a Stratum run.
pub type RunId = Uuid;

/// Reference to a model backend (e.g., "claude-sonnet-4-20250514", "gpt-4o").
pub type ModelRef = String;

/// Trust level governing tool permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustLevel {
    /// Read-only filesystem, no network, no subprocess spawning.
    Sandboxed,
    /// Read-write in scoped directory, allowlisted network, subprocess with capture.
    Supervised,
    /// Full filesystem, unrestricted network, subprocess spawning.
    Autonomous,
}

/// Run state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// The top-level identity and configuration of a Stratum run.
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
```

### 5.2 Task Manifest

```rust
/// The run's durable contract -- goal, progress, decisions.
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
```

### 5.3 Context Budget

```rust
/// Hard token ceilings per context slot.
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
```

### 5.4 Tool Result

```rust
/// Result of a tool execution through the Gateway.
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
```

### 5.5 Trajectory Event

```rust
/// Every harness decision produces a trajectory event.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StratumLayer {
    SessionLifecycle,   // 1
    ContextEngine,      // 2
    MemoryHierarchy,    // 3
    ToolGateway,        // 4
    SubAgentOrchestrator, // 5
    HitlController,     // 6
    TrajectoryStore,    // 7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCost {
    pub cached_tokens: u64,
    pub uncached_tokens: u64,
    pub estimated_cost_usd: f64,
}

/// All event types emitted by the harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    MemoryWritten { tier: MemoryTier },
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
```

### 5.6 Supporting Types

```rust
/// Memory tier classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryTier {
    Working,
    Episodic,
    Project,
    Global,
}

/// Memory configuration for a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub episodic_backend: String,
    pub project_root: Option<String>,
    pub global_backend: String,
    pub search_strategy: SearchStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchStrategy {
    Vector,
    Bm25,
    Hybrid { vector_weight: f32 },
}

/// HITL policy configuration.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatePolicy {
    AlwaysAsk,
    NotifyAndOption,
    Notify,
    Auto,
}

/// HITL decision types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HitlDecision {
    Approve,
    Modify { context: String },
    Redirect { new_goal: String },
    Abort,
}

/// A HITL gate record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlRecord {
    pub run_id: RunId,
    pub gate_category: String,
    pub action_attempted: String,
    pub alternatives: Vec<String>,
    pub context_summary: String,
    pub decision: Option<HitlDecision>,
}

/// Checkpoint data for run persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub run_id: RunId,
    pub state: RunState,
    pub task_manifest: TaskManifest,
    pub context_summary: String,
    pub tool_call_log: Vec<ToolResult>,
    pub sub_agent_tree: Vec<RunId>,
    pub created_at: DateTime<Utc>,
}

/// Sub-agent spawn pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnPattern {
    Delegate,
    Pipeline,
    Parallel,
    Janitor,
}
```

---

## 6. Port Traits

Port traits are defined in `stratum-core` and implemented by concrete adapters. Each stratum has one or more port traits. All async traits use `async_trait`.

### 6.1 Stratum 1 -- Session Lifecycle Manager

```rust
/// Manages the complete lifecycle of a Stratum run.
#[async_trait]
pub trait SessionManager: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Create a new run with the given configuration. Returns the initialised StratumRun.
    async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error>;

    /// Write a checkpoint for the given run.
    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error>;

    /// Resume a run from its most recent checkpoint.
    async fn resume(&self, run_id: RunId) -> Result<StratumRun, Self::Error>;

    /// Transition a run to a new state. Emits a trajectory event.
    async fn transition_state(
        &self,
        run_id: RunId,
        new_state: RunState,
    ) -> Result<(), Self::Error>;

    /// Load a run by ID.
    async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error>;

    /// Load the most recent checkpoint for a run.
    async fn get_latest_checkpoint(
        &self,
        run_id: RunId,
    ) -> Result<Option<Checkpoint>, Self::Error>;
}
```

### 6.2 Stratum 2 -- Context Engine

```rust
/// Assembled context ready for model consumption.
pub struct AssembledContext {
    pub system_anchor: String,
    pub task_manifest: String,
    pub injected_knowledge: Vec<String>,
    pub tool_results: Vec<String>,
    pub history: String,
    pub total_tokens: u64,
}

/// Context hygiene score.
pub struct HygieneScore {
    pub score: f32,
    pub on_task_fraction: f32,
    pub consecutive_degraded: u32,
}

/// Manages context window assembly, budgeting, and compaction.
#[async_trait]
pub trait ContextEngine: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Assemble the context window for the next model call.
    async fn assemble_context(
        &self,
        run_id: RunId,
    ) -> Result<AssembledContext, Self::Error>;

    /// Check whether the current context fits within the budget.
    fn check_budget(&self, context: &AssembledContext, budget: &ContextBudget) -> BudgetStatus;

    /// Trigger compaction pipeline (stages 1-3 as needed).
    async fn trigger_compaction(
        &self,
        run_id: RunId,
        stage: CompactionStage,
    ) -> Result<(), Self::Error>;

    /// Compute the context hygiene score for the last K turns.
    async fn compute_hygiene_score(
        &self,
        run_id: RunId,
        window_size: usize,
    ) -> Result<HygieneScore, Self::Error>;

    /// Inject a todo recitation block into context.
    async fn inject_todo_recitation(
        &self,
        run_id: RunId,
    ) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy)]
pub enum BudgetStatus {
    WithinBudget,
    ApproachingThreshold { utilisation: f32 },
    OverBudget { overage_tokens: u64 },
}

#[derive(Debug, Clone, Copy)]
pub enum CompactionStage {
    Offload,
    Truncate,
    Summarise,
}
```

### 6.3 Stratum 3 -- Memory Hierarchy

```rust
/// A memory entry with tier and metadata.
pub struct MemoryEntry {
    pub id: String,
    pub tier: MemoryTier,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Search result from memory.
pub struct MemorySearchResult {
    pub entry: MemoryEntry,
    pub relevance_score: f32,
}

/// Port trait for memory read/write/search across tiers.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Read a memory entry by ID within a tier.
    async fn read(
        &self,
        tier: MemoryTier,
        id: &str,
    ) -> Result<Option<MemoryEntry>, Self::Error>;

    /// Write a memory entry to the specified tier.
    async fn write(
        &self,
        tier: MemoryTier,
        entry: &MemoryEntry,
    ) -> Result<(), Self::Error>;

    /// Search memory using hybrid vector + BM25 ranking.
    async fn search(
        &self,
        tier: MemoryTier,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchResult>, Self::Error>;

    /// Promote an entry from one tier to a higher tier. Global promotion is queued for review.
    async fn promote(
        &self,
        entry_id: &str,
        from: MemoryTier,
        to: MemoryTier,
    ) -> Result<(), Self::Error>;
}

/// Skill definition with trigger conditions.
pub struct Skill {
    pub name: String,
    pub description: String,
    pub trigger_conditions: Vec<String>,
    pub content: String,
}

/// Port trait for loading skills (Project tier specialisation).
#[async_trait]
pub trait SkillLoader: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// List available skills (names and descriptions only).
    async fn list_skills(&self) -> Result<Vec<(String, String)>, Self::Error>;

    /// Load a skill's full content by name.
    async fn load_skill(&self, name: &str) -> Result<Option<Skill>, Self::Error>;

    /// Match current task against skill triggers, return matching skill names.
    async fn match_skills(&self, task_context: &str) -> Result<Vec<String>, Self::Error>;
}
```

### 6.4 Stratum 4 -- Tool Execution Gateway

```rust
/// A tool definition in the manifest.
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
    pub trust_level_required: TrustLevel,
}

/// A pending tool invocation before execution.
pub struct ToolInvocation {
    pub tool_name: String,
    pub parameters: serde_json::Value,
    pub run_id: RunId,
}

/// Port trait for tool execution with intercept-validate-execute-log pipeline.
#[async_trait]
pub trait ToolGateway: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Intercept a tool call before execution (logging, policy check).
    async fn intercept(&self, invocation: &ToolInvocation) -> Result<(), Self::Error>;

    /// Validate parameters against schema and policy rules.
    async fn validate(&self, invocation: &ToolInvocation) -> Result<(), Self::Error>;

    /// Execute the tool in an isolated environment appropriate to the trust level.
    async fn execute(&self, invocation: &ToolInvocation) -> Result<ToolResult, Self::Error>;

    /// Log the result to the Trajectory Store.
    async fn log_result(&self, result: &ToolResult, run_id: RunId) -> Result<(), Self::Error>;
}

/// Port trait for managing the tool manifest.
pub trait ToolRegistry: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Get the full tool manifest for a run.
    fn get_manifest(&self, run_id: RunId) -> Result<Vec<ToolDefinition>, Self::Error>;

    /// Check if a tool is permitted at the given trust level.
    fn is_permitted(&self, tool_name: &str, trust_level: TrustLevel) -> bool;

    /// Register a tool definition.
    fn register(&mut self, definition: ToolDefinition) -> Result<(), Self::Error>;
}
```

### 6.5 Stratum 5 -- Sub-Agent Orchestrator

```rust
/// Configuration for spawning a sub-agent.
pub struct SpawnConfig {
    pub pattern: SpawnPattern,
    pub model_ref: ModelRef,
    pub trust_level: TrustLevel,
    pub tool_manifest: Vec<String>,
    pub task_goal: String,
    pub shared_filesystem_scope: Option<String>,
}

/// Result returned from a completed sub-agent.
pub struct SubAgentResult {
    pub run_id: RunId,
    pub status: RunState,
    pub summary: String,
    pub artefacts: Vec<String>,
}

/// Port trait for spawning and managing sub-agent runs.
#[async_trait]
pub trait Orchestrator: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Spawn a sub-agent using the specified pattern. Returns the sub-agent's RunId.
    async fn spawn(
        &self,
        parent_run_id: RunId,
        config: SpawnConfig,
    ) -> Result<RunId, Self::Error>;

    /// Wait for a sub-agent to complete and return its compressed result.
    async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error>;

    /// Get the current spawn depth for a run.
    fn current_depth(&self, run_id: RunId) -> Result<u32, Self::Error>;

    /// Check if spawning is permitted (depth limit not exceeded).
    fn can_spawn(&self, parent_run_id: RunId) -> Result<bool, Self::Error>;
}

/// Port trait for rfbmq-backed task dispatch within the orchestrator.
pub trait TaskDispatch: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Enqueue a task with dependency and routing metadata.
    fn enqueue(
        &self,
        body: &str,
        opts: DispatchOptions,
    ) -> Result<String, Self::Error>;

    /// List tasks whose dependencies are all satisfied.
    fn list_ready(&self) -> Result<Vec<String>, Self::Error>;

    /// Claim the next ready task.
    fn dequeue(&self) -> Result<Option<ClaimedTask>, Self::Error>;

    /// Mark a task as completed.
    fn complete(&self, task: &ClaimedTask) -> Result<(), Self::Error>;

    /// Mark a task as failed.
    fn fail(&self, task: &ClaimedTask) -> Result<(), Self::Error>;

    /// Current queue depth.
    fn depth(&self) -> Result<i64, Self::Error>;
}

pub struct DispatchOptions {
    pub priority: TaskPriority,
    pub tags: Vec<String>,
    pub correlation_id: Option<String>,
    pub reply_to: Option<String>,
    pub depends_on: Vec<String>,
    pub ttl: u32,
}

pub struct ClaimedTask {
    pub id: String,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum TaskPriority {
    Critical,
    High,
    Normal,
    Low,
}
```

### 6.6 Stratum 6 -- HITL Controller

```rust
/// Port trait for managing human-in-the-loop gates.
#[async_trait]
pub trait HitlController: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Open a gate, pausing the run until a human decision is received.
    async fn open_gate(&self, record: HitlRecord) -> Result<(), Self::Error>;

    /// Record a human decision for an open gate.
    async fn record_decision(
        &self,
        run_id: RunId,
        decision: HitlDecision,
    ) -> Result<(), Self::Error>;

    /// Get all pending gates (no decision yet).
    async fn pending_gates(&self) -> Result<Vec<HitlRecord>, Self::Error>;

    /// Get the decision for a specific gate, if one exists.
    async fn get_decision(
        &self,
        run_id: RunId,
        gate_id: &str,
    ) -> Result<Option<HitlDecision>, Self::Error>;
}

/// Port trait for async notifications to humans.
#[async_trait]
pub trait Notifier: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Send a notification about a pending HITL decision.
    async fn notify(&self, record: &HitlRecord) -> Result<(), Self::Error>;
}
```

### 6.7 Stratum 7 -- Trajectory Store & Observability

```rust
/// Time range for querying events.
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Export format for trajectory data.
#[derive(Debug, Clone, Copy)]
pub enum ExportFormat {
    Jsonl,
    Csv,
    Replay,
}

/// Port trait for the append-only trajectory event store.
#[async_trait]
pub trait TrajectoryStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Emit a trajectory event.
    async fn emit_event(&self, event: TrajectoryEvent) -> Result<(), Self::Error>;

    /// Query events by run, event type, time range, and/or outcome.
    async fn query_events(
        &self,
        run_id: Option<RunId>,
        event_type: Option<EventType>,
        time_range: Option<TimeRange>,
        limit: Option<usize>,
    ) -> Result<Vec<TrajectoryEvent>, Self::Error>;

    /// Export trajectory data in the specified format.
    async fn export(
        &self,
        run_id: RunId,
        format: ExportFormat,
    ) -> Result<Vec<u8>, Self::Error>;
}

/// Port trait for exporting metrics (Prometheus-compatible).
pub trait MetricsExporter: Send + Sync {
    /// Get current metrics as Prometheus text format.
    fn export_metrics(&self) -> String;

    /// Record a gauge value.
    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]);

    /// Increment a counter.
    fn counter(&self, name: &str, labels: &[(&str, &str)]);
}
```

### 6.8 Cross-Cutting -- LLM Client

```rust
/// Port trait for LLM API calls (used by Context Engine, Orchestrator, etc.).
#[async_trait]
pub trait LlmClient: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Send a completion request to the model.
    async fn complete(
        &self,
        messages: &[LlmMessage],
        model: &str,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error>;
}

pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<LlmToolCall>,
    pub usage: LlmUsage,
}

pub struct LlmToolCall {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}
```

---

## 7. Stratum Internals

### 7.1 Stratum 1 -- Session Lifecycle Manager (in `stratum-core`)

**Two-Prompt Pattern.** Every run uses two distinct system prompts:
- *Initialiser Prompt*: runs once, instructs the agent to produce four artefacts (TASK.md, PROGRESS.md, DECISIONS.md, INIT.sh). These survive every context window reset.
- *Worker Prompt*: runs for all subsequent context windows. Reads artefact state, identifies the highest-priority incomplete item, proceeds incrementally.

**Checkpointing triggers:**
1. After every significant tool call (write, execute, network)
2. At configurable time intervals
3. Whenever a HITL gate opens

**State transitions:** `Initialising -> Running -> Paused -> Checkpointed -> Resuming -> Completed | Failed | Aborted`. Every transition emits a `TrajectoryEvent`.

### 7.2 Stratum 2 -- Context Engine (in `stratum-context`)

**KV-Cache-First Constraint.** System prompt prefix is immutable within a run. Tool definitions are static. Context is append-only.

**Budget Model.** Five named slots with hard token ceilings: System Anchor, Task Manifest, Injected Knowledge, Tool Results, History.

**Three-Stage Compaction Pipeline:**
1. *Offload* -- Tool results exceeding threshold (default: 15K tokens) are written to filesystem. Slot replaced with file reference + 10-line preview. Restorable compression.
2. *Truncate* -- Tool call I/O referencing already-persisted content reduced to reference only.
3. *Summarise* -- At ~85% budget utilisation, structured LLM summarisation runs. Preserves: goal, completed steps, key decisions, files modified, errors, blockers, next action. Full history archived to Trajectory Store.

**Errors stay in context.** Failed tool calls are never pruned.

**Todo Recitation.** Every N turns (configurable, default: 5), current PROGRESS.md state is appended as a compact reminder. Exploits recency bias to keep long-horizon goals salient.

**Context Hygiene Score.** Embedding similarity check against task manifest each turn. When score drops below threshold for N consecutive turns, a re-anchor block is injected.

### 7.3 Stratum 3 -- Memory Hierarchy (in `stratum-memory`)

**Four tiers:**

| Tier | Scope | Default Backend | TTL | Agent Write Access |
|------|-------|----------------|-----|-------------------|
| Working | Current turn | In-process | Ephemeral | Implicit |
| Episodic | Current run | SQLite / KV | Run lifetime | Via tool |
| Project | Across runs, same project | Filesystem (Markdown + vector index) | Project lifetime | Via tool |
| Global | Across all projects | Vector DB (SQLite-vec) | Indefinite | Harness-managed only |

**Project tier** is the primary memory medium: versioned Markdown files (TASK.md, PROGRESS.md, DECISIONS.md, MEMORY.md). Human-readable, VCS-compatible, diff-friendly.

**Global tier promotion** requires explicit review. Agent requests via `memory_promote` tool; harness queues for human approval or policy engine.

**Semantic search** uses hybrid vector + BM25 ranking with optional temporal decay. Always scoped by tier, project, and recency window.

**Skills** are Project tier Markdown files in `.stratum/skills/` with YAML frontmatter. Progressive disclosure: only names/descriptions injected at boot; full content loaded on trigger match.

### 7.4 Stratum 4 -- Tool Execution Gateway (in `stratum-tools`)

**Tool Manifest.** Explicit whitelist, opt-in, immutable for the run's lifetime. Deliberately minimal defaults.

**Trust Levels.** Sandboxed -> Supervised -> Autonomous. Escalation always requires HITL approval.

**Intercept-Validate-Execute-Log Pipeline:**
1. *Intercept:* capture full invocation before execution.
2. *Validate:* check schema, parameter types, policy rules. On failure, return structured error with remediation instructions.
3. *Execute:* run in isolated subprocess/container appropriate to trust level.
4. *Log:* write complete record to Trajectory Store.

**Error handling.** Errors are never swallowed. Configurable retry with exponential backoff for transient failures. After N retries, escalate to HITL. All retry attempts visible in context.

**KV-cache protection.** Tool definitions declared at init, never change. Logit masking or state-machine policy controls encouraged tool calls per step, but schema definitions remain static.

### 7.5 Stratum 5 -- Sub-Agent Orchestrator (in `stratum-orchestrator`)

**Sub-agents are full Stratum runs** with narrower scope, more restricted tool manifest, and a reference to parent `RunId`. Isolated context windows; communicate only through structured result return and shared filesystem.

**Spawn depth bounded.** Default max: 2 (parent -> child -> no further). Configurable up to 5. Hard-blocked beyond configured maximum.

**Four spawn patterns:**
- *Delegate:* one sub-agent for a bounded sub-task, returns compressed result.
- *Pipeline:* fixed sequence of specialised sub-agents, each consuming prior's output.
- *Parallel:* N concurrent sub-agents on independent workstreams, supervisor merges results.
- *Janitor:* background sub-agents on scheduled cadence enforcing quality invariants.

**Context isolation** is the primary value. Parent sees only compressed result. This prevents context rot accumulation in long-horizon tasks.

**rfbmq integration.** The `TaskDispatch` port trait wraps rfbmq for dependency-aware task scheduling within the orchestrator. See ADR-002 (Section 8) for details.

### 7.6 Stratum 6 -- HITL Controller (in `stratum-core`)

**Gate categories:** Destructive, Irreversible, Trust Escalation, Ambiguity, Drift, Budget, Scheduled. Each has a configurable default policy (AlwaysAsk, NotifyAndOption, Notify, Auto).

**Pause records** are structured `HitlRecord` values containing: run state, action attempted, alternatives considered, context summary, decision options.

**Decisions are typed:** Approve, Modify, Redirect, Abort. All decisions are trajectory events.

**Pauses are durable.** Run persists in Paused state indefinitely. Resumable days later from checkpoint.

**Async notification** via pluggable `Notifier` adapters: webhook, email, Slack, custom.

### 7.7 Stratum 7 -- Trajectory Store (in `stratum-core`)

**Append-only event log.** Default backend: SQLite. Every event has: event_id, run_id, parent_run_id, timestamp, event_type, stratum_layer, payload, token_cost.

**Queryable and exportable.** By run, event type, time range, outcome. Export formats: JSONL (fine-tuning), CSV (analysis), Replay (deterministic re-run with different model).

**Observability surface.** Metrics exposed as Prometheus-compatible gauges/counters: run state, context budget utilisation, KV-cache hit rate estimate, tool call rate/error rate, hygiene score trend, HITL queue depth, sub-agent tree, cost-per-run.

---

## 8. ADR-002: rfbmq for Sub-Agent Task Dispatch

### Decision

Use rfbmq-core as a direct Cargo dependency for sub-agent task dispatch within the Orchestrator (Stratum 5). No FFI, no adapter crate, no unsafe code.

### Context

The Sub-Agent Orchestrator needs a mechanism to dispatch tasks to sub-agents with dependency ordering, result routing, and crash recovery. rfbmq provides all of these as a pure Rust filesystem-based message queue.

In SAD v2.1, rfbmq was the central architectural element (Conductor/Worker pattern). In v3.0, rfbmq is scoped to Stratum 5 as an implementation detail of sub-agent task dispatch.

### Implementation

The `TaskDispatch` trait adapter wraps `rfbmq::Queue`:

```rust
use std::path::PathBuf;
use rfbmq::{Queue, Message, ClaimedMessage, MessageId};

pub struct RfbmqDispatcher {
    root: PathBuf,
    queue: Queue,
}

impl RfbmqDispatcher {
    pub fn new(root: PathBuf) -> Result<Self, rfbmq::Error> {
        let queue = Queue::open(&root)?;
        Ok(Self { root, queue })
    }

    pub fn init(root: PathBuf, max_pending: i64) -> Result<Self, rfbmq::Error> {
        let queue = Queue::init(&root, true, max_pending)?;
        Ok(Self { root, queue })
    }
}
```

### Leveraging rfbmq Features

**Task Dependency DAGs with `Depends-On`:**

```rust
// Decompose a pipeline: research -> synthesise -> validate
let research_id = dispatcher.enqueue("Research the topic", DispatchOptions {
    tags: vec!["research".into()],
    ..Default::default()
})?;

let synth_id = dispatcher.enqueue("Synthesise findings", DispatchOptions {
    depends_on: vec![research_id.clone()],
    ..Default::default()
})?;

let validate_id = dispatcher.enqueue("Validate output", DispatchOptions {
    depends_on: vec![synth_id],
    ..Default::default()
})?;
```

**Result Routing with `Reply-To`:**

```rust
let eval_id = dispatcher.enqueue("Evaluate correctness", DispatchOptions {
    reply_to: Some("/var/stratum/queues/eval-results".into()),
    correlation_id: Some(parent_run_id.to_string()),
    ..Default::default()
})?;
```

**Dependency-Aware Scheduling with `list_ready()`:**

```rust
let ready = dispatcher.list_ready()?;
// Only tasks whose Depends-On predecessors are all completed appear here.
```

### Queue Topology (Orchestrator-scoped)

```
~/.stratum/queues/
  tasks/          # main task queue (Orchestrator -> Sub-agents)
  results/        # result routing (Sub-agents -> Orchestrator)
  janitor/        # janitor tasks (scheduled invariant checks)
  dead-letter/    # failed tasks for inspection
```

Each queue uses standard rfbmq directory structure (`pending/`, `processing/`, `done/`, `failed/`, `.tmp/`, `.meta/`).

---

## 9. Event Flow Across Strata

A typical agent turn flows through the strata as follows:

```
1. Session Manager receives a turn trigger
   └─> emits RunStarted / RunResumed event to Trajectory Store

2. Context Engine assembles the context window
   ├─> checks budget, triggers compaction if needed
   ├─> injects todo recitation if interval reached
   ├─> computes hygiene score
   └─> emits context events to Trajectory Store

3. Memory Hierarchy provides retrieved knowledge
   ├─> searches relevant tiers (episodic, project)
   ├─> loads matched skills
   └─> emits MemorySearched / SkillLoaded events

4. Model generates a response (external -- not a stratum)

5. Tool Gateway processes any tool calls
   ├─> intercept -> validate -> execute -> log
   ├─> on failure: returns remediation, retries, or escalates to HITL
   └─> emits tool events to Trajectory Store

6. HITL Controller checks gate conditions
   ├─> if gate triggered: pauses run, notifies human
   └─> emits GateOpened / RunPaused events

7. Sub-Agent Orchestrator handles delegation if needed
   ├─> spawns sub-agents via rfbmq dispatch
   ├─> monitors completion, collects compressed results
   └─> emits subagent events to Trajectory Store

8. Session Manager checkpoints after significant actions
   └─> emits CheckpointWritten event

9. Trajectory Store persists all events (append-only)
   └─> available for query, export, and dashboard
```

---

## 10. Deployment View

### 10.1 Build

```bash
cargo build --release --target x86_64-unknown-linux-musl
# or for ARM:
cargo build --release --target aarch64-unknown-linux-musl
```

No special build steps. Standard Cargo. No C compiler, no `build.rs` for FFI.

### 10.2 Binary Size Estimate

| Component | Approximate Size |
|-----------|-----------------|
| Stratum crates (types, core, context, memory, tools, orchestrator) | ~3 MB |
| rfbmq-core | ~200 KB |
| rusqlite (bundled SQLite) | ~1.5 MB |
| reqwest + TLS | ~3 MB |
| **Total (stripped, LTO)** | **~8-9 MB** |

### 10.3 Runtime Requirements

- Filesystem supporting `rename(2)` atomicity (ext4, APFS, ZFS, XFS)
- No network daemon, no ports, no config files required
- Queue directories and SQLite DB writable by the Stratum process

### 10.4 Development Environment

All crates are pure Rust. Full stack runs on macOS and Linux. No VM or container needed.

---

## 11. Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| rfbmq is a young crate | Undiscovered bugs | Same org/workspace -- bugs fixable directly. Contract tests catch issues early. |
| Filesystem performance at scale | Slow with >100K pending messages | `max_pending` soft limit; priority subdirs reduce scan scope |
| SQLite write contention | Multiple sub-agents updating state DB | WAL mode; coarse-grained locking; state updates are infrequent |
| LLM API rate limits | Processing stalls | Exponential backoff; configurable concurrency limits |
| Context compaction quality | LLM summarisation loses critical info | Stage 1 and 2 are lossless; Stage 3 preserves structured skeleton; full history archived |
| Embedding model dependency | Hygiene score and semantic search need embeddings | Pluggable via LlmClient; can use local models or API |
| Crate count (9) increases build time | Slower iteration | Workspace-level caching; most crates are small |

---

## Appendix A: rfbmq API Summary

Scoped to Stratum 5 (Sub-Agent Orchestrator). For reference, the rfbmq-core public API used by the `TaskDispatch` adapter:

| Method | Signature | Description |
|--------|-----------|-------------|
| `Queue::init` | `(root: &Path, use_priority: bool, max_pending: i64) -> Result<Queue>` | Create a new queue |
| `Queue::open` | `(root: &Path) -> Result<Queue>` | Open an existing queue |
| `Queue::enqueue` | `(&self, msg: &mut Message) -> Result<MessageId>` | Add a message to the queue |
| `Queue::dequeue` | `(&self) -> Result<Option<ClaimedMessage>>` | Claim the next pending message |
| `Queue::complete` | `(&self, claimed: &ClaimedMessage) -> Result<()>` | Move claimed message to done/ |
| `Queue::fail` | `(&self, claimed: &ClaimedMessage) -> Result<()>` | Retry or dead-letter a message |
| `Queue::reap` | `(&self) -> Result<u32>` | Reclaim expired leases |
| `Queue::reap_ttl` | `(&self) -> Result<u32>` | Expire TTL-exceeded pending messages |
| `Queue::purge` | `(&self, max_age_seconds: u32) -> Result<u32>` | Delete old completed messages |
| `Queue::list_ready` | `(&self) -> Result<Vec<MessageId>>` | List dependency-satisfied message IDs |
| `Queue::depth` | `(&self) -> Result<i64>` | Pending + processing count |

Key types: `MessageId`, `ClaimedMessage`, `Message`, `Header` (with `depends_on`, `reply_to`), `Priority`.

---

## Appendix B: Glossary

| Term | Definition |
|------|-----------|
| **Stratum** | One of the seven independent layers of the harness |
| **Run** | A complete agent execution, identified by a RunId |
| **Sub-agent** | A child Stratum run with narrower scope, spawned by the Orchestrator |
| **Checkpoint** | A durable snapshot of run state, restorable after crashes |
| **Gate** | A HITL pause point triggered by policy |
| **Compaction** | The 3-stage process of reducing context window size |
| **Trajectory** | The append-only event log of all harness decisions for a run |
| **Skill** | A Project-tier Markdown file with trigger conditions for progressive loading |
| **rfbmq** | Pure Rust filesystem-based message queue used for sub-agent task dispatch |
| **DAG** | Directed acyclic graph of task dependencies via `Depends-On` headers |
