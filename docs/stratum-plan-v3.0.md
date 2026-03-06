# Stratum -- Implementation Plan v3.0

> **Version 3.0** -- full rewrite aligned with PRD v1.0 seven-stratum architecture.
> Supersedes Plan v2.1 which described a narrower task-queue orchestrator build.

---

## Overview

This plan describes the phased implementation of Stratum, a model-agnostic agent harness with seven independently swappable strata. It targets a single developer working full-time.

### Key Changes from Plan v2.1

- **Scope:** From 3-crate task-queue orchestrator to 9-crate agent harness covering all 7 PRD strata.
- **Phases:** From 10 phases (~30 days) to 13 phases (~50 days).
- **Ordering:** Trajectory Store (Stratum 7) is built first -- every other stratum emits events to it. LLM adapters precede Context Engine and Orchestrator.
- **Existing code:** `crates/api` removed (just re-exports). `crates/core` expands. New crate directories created in Phase 1. Zero migration cost.

---

## Phase Summary

| Phase | Name | Days | PRD Stratum | Dependencies |
|-------|------|------|-------------|--------------|
| 1 | Foundation | 3 | All | -- |
| 2 | Trajectory Store | 3 | 7 | Phase 1 |
| 3 | Session Lifecycle Manager | 4 | 1 | Phases 1, 2 |
| 4 | LLM Client Adapters | 3 | Cross-cutting | Phase 1 |
| 5 | Context Engine | 5 | 2 | Phases 2, 3, 4 |
| 6 | Memory Hierarchy | 5 | 3 | Phases 2, 4 |
| 7 | Tool Execution Gateway | 4 | 4 | Phases 2, 3 |
| 8 | Sub-Agent Orchestrator + rfbmq | 5 | 5 | Phases 3, 4, 7 |
| 9 | HITL Controller | 3 | 6 | Phases 2, 3 |
| 10 | CLI + Integration Wiring | 4 | -- | Phases 5-9 |
| 11 | Observability Dashboard | 3 | 7 | Phases 2, 10 |
| 12 | Testing & Hardening | 5 | All | Phase 10 |
| 13 | Documentation & Release | 3 | -- | Phase 12 |
| | **Total** | **~50 days (~10 weeks)** | | |

### Dependency DAG

```
Phase 1 (Foundation)
  ├── Phase 2 (Trajectory Store)
  │     ├── Phase 3 (Session Lifecycle)
  │     │     ├── Phase 5 (Context Engine) ← also needs Phase 4
  │     │     ├── Phase 7 (Tool Gateway)
  │     │     ├── Phase 8 (Orchestrator) ← also needs Phases 4, 7
  │     │     └── Phase 9 (HITL Controller)
  │     ├── Phase 6 (Memory Hierarchy) ← also needs Phase 4
  │     └── Phase 11 (Observability) ← also needs Phase 10
  └── Phase 4 (LLM Client Adapters)

Phase 10 (CLI + Integration) ← needs Phases 5-9
Phase 12 (Testing & Hardening) ← needs Phase 10
Phase 13 (Documentation & Release) ← needs Phase 12
```

---

## Phase 1: Foundation (3 days)

### Goal

Set up the 9-crate workspace, define all domain types, declare all port traits, and establish CI.

### Tasks

1. **Restructure workspace** -- remove `crates/api`, expand to 9 crates:
   ```
   stratum/
     Cargo.toml                    # workspace
     crates/
       stratum-types/              # domain types
       stratum-core/               # port traits + thin strata (1, 6, 7)
       stratum-context/            # Stratum 2
       stratum-memory/             # Stratum 3
       stratum-tools/              # Stratum 4
       stratum-orchestrator/       # Stratum 5
       stratum-adapters/           # external adapters
       stratum-cli/                # CLI binary
       stratum-test-utils/         # test infrastructure
   ```

2. **Add workspace dependencies**:
   ```toml
   [workspace.dependencies]
   rfbmq-core = { git = "https://github.com/swiftugandan/rfbmq", version = "0.1" }
   uuid = { version = "1", features = ["v4", "serde"] }
   chrono = { version = "0.4", features = ["serde"] }
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   async-trait = "0.1"
   tokio = { version = "1", features = ["full"] }
   ```

3. **Implement all domain types** in `stratum-types` (SAD Section 5): `StratumRun`, `TaskManifest`, `ContextBudget`, `ToolResult`, `TrajectoryEvent`, `MemoryTier`, `HitlPolicy`, `SpawnPattern`, etc.

4. **Declare all port traits** in `stratum-core` (SAD Section 6): `SessionManager`, `ContextEngine`, `MemoryStore`, `SkillLoader`, `ToolGateway`, `ToolRegistry`, `Orchestrator`, `TaskDispatch`, `HitlController`, `Notifier`, `TrajectoryStore`, `MetricsExporter`, `LlmClient`.

5. **Set up CI**: GitHub Actions running `cargo check`, `cargo test`, `cargo clippy`, `cargo fmt --check` across the workspace.

6. **Create `stratum-test-utils`** with mock/stub implementations of all port traits for testing.

### Deliverables

- Compiling workspace with all types and trait declarations
- Mock implementations for all port traits
- CI pipeline passing

---

## Phase 2: Trajectory Store (3 days)

### Goal

Implement the append-only event store (Stratum 7). This is built first because every other stratum emits events to it.

### Tasks

1. **SQLite schema** for trajectory events:
   - `events` table: event_id, run_id, parent_run_id, timestamp, event_type, stratum_layer, payload (JSON), cached_tokens, uncached_tokens, estimated_cost_usd
   - Indexes on run_id, event_type, timestamp

2. **Implement `TrajectoryStore` trait** in `stratum-adapters` using rusqlite:
   - `emit_event()`: insert event row
   - `query_events()`: parameterised query with optional filters
   - `export()`: JSONL, CSV, and Replay format serialisation

3. **Implement basic `MetricsExporter`**: in-memory counters/gauges with Prometheus text format export.

4. **Contract tests**:
   - Emit events, query back by run_id, event_type, time_range
   - Export round-trip: emit -> export JSONL -> parse -> verify
   - Concurrent writes from multiple tasks

### Deliverables

- `SqliteTrajectoryStore` implementing `TrajectoryStore`
- `InMemoryMetrics` implementing `MetricsExporter`
- Contract test suite passing

---

## Phase 3: Session Lifecycle Manager (4 days)

### Goal

Implement run creation, state machine, checkpointing, and resume (Stratum 1).

### Tasks

1. **SQLite schema** for runs and checkpoints:
   - `runs` table: id, parent_run_id, model_ref, trust_level, tool_manifest (JSON), state, created_at, updated_at
   - `checkpoints` table: id, run_id, state, task_manifest (JSON), context_summary, tool_call_log (JSON), sub_agent_tree (JSON), created_at

2. **Implement `SessionManager` trait** in `stratum-core`:
   - `create_run()`: insert run, emit `RunCreated` event
   - `checkpoint()`: serialize and store checkpoint, emit `CheckpointWritten`
   - `resume()`: load latest checkpoint, transition to `Resuming`, emit `RunResumed`
   - `transition_state()`: validate state machine transitions, emit appropriate event

3. **Two-Prompt Pattern**: implement `InitialiserPrompt` and `WorkerPrompt` templates.

4. **Contract tests**:
   - Full lifecycle: create -> start -> checkpoint -> resume -> complete
   - Invalid state transitions are rejected
   - Checkpoint round-trip fidelity

### Deliverables

- `SessionManager` implementation
- State machine with validated transitions
- Checkpoint persistence and restore

---

## Phase 4: LLM Client Adapters (3 days)

### Goal

Implement the `LlmClient` port trait for Anthropic and OpenAI-compatible APIs.

### Tasks

1. **Implement `AnthropicClient`** in `stratum-adapters`:
   - Messages API with tool use support
   - Streaming support (optional, can defer)
   - Token usage tracking (input, output, cached)

2. **Implement `OpenAiClient`** in `stratum-adapters`:
   - Chat completions API with tool/function calling
   - Compatible with OpenAI-API-compatible providers

3. **Rate limiting and retry logic**: exponential backoff with jitter for transient failures (429, 500, 503).

4. **Integration tests** with mock HTTP server (wiremock or similar).

### Deliverables

- `AnthropicClient` and `OpenAiClient` implementing `LlmClient`
- Retry logic with backoff
- Integration tests passing

---

## Phase 5: Context Engine (5 days)

### Goal

Implement context assembly, budget model, 3-stage compaction, hygiene scoring, and todo recitation (Stratum 2).

### Tasks

1. **Context assembly** (`assemble_context()`):
   - Build the five-slot context structure (System Anchor, Task Manifest, Injected Knowledge, Tool Results, History)
   - Token counting per slot (tiktoken-rs or character-based estimate)

2. **Budget checking** (`check_budget()`):
   - Return `WithinBudget`, `ApproachingThreshold`, or `OverBudget`

3. **Three-stage compaction**:
   - Stage 1 (Offload): write large tool results to filesystem, replace with reference + preview
   - Stage 2 (Truncate): reduce already-persisted content to references
   - Stage 3 (Summarise): LLM-based structured summarisation. Archive full history to Trajectory Store.
   - Each stage emits appropriate `CompactionStage*Triggered` event.

4. **Todo recitation**: inject PROGRESS.md state every N turns. Configurable interval.

5. **Context hygiene score**: embedding similarity against task manifest. Track consecutive degradation. Inject re-anchor block when threshold exceeded.

6. **Tests**:
   - Budget calculation with known token counts
   - Compaction stages trigger at correct thresholds
   - Hygiene score detects drift

### Deliverables

- `ContextEngine` implementation in `stratum-context`
- Three-stage compaction pipeline
- Hygiene score with re-anchoring

---

## Phase 6: Memory Hierarchy (5 days)

### Goal

Implement the 4-tier memory system, semantic search, promotion, and skill loading (Stratum 3).

### Tasks

1. **Working tier**: in-process HashMap, ephemeral per turn.

2. **Episodic tier**: SQLite-backed KV store, scoped to run lifetime.

3. **Project tier**: filesystem-backed Markdown files with vector index.
   - Read/write TASK.md, PROGRESS.md, DECISIONS.md, MEMORY.md
   - Build and query vector index for semantic search

4. **Global tier**: SQLite-vec or similar for cross-project vector search.
   - Promotion queue: agent requests, harness queues for review.

5. **`MemoryStore` implementation** covering all four tiers.

6. **Semantic search**: hybrid vector + BM25 ranking with temporal decay.

7. **`SkillLoader` implementation**:
   - Scan `.stratum/skills/` for Markdown files with YAML frontmatter
   - Progressive disclosure: names/descriptions at boot, full content on match

8. **Tests**:
   - CRUD operations per tier
   - Cross-tier search ranking
   - Promotion flow (including Global tier review queue)
   - Skill matching and progressive loading

### Deliverables

- `MemoryStore` and `SkillLoader` implementations in `stratum-memory`
- 4-tier memory with search
- Skill loading with progressive disclosure

---

## Phase 7: Tool Execution Gateway (4 days)

### Goal

Implement the intercept-validate-execute-log pipeline, trust levels, and tool manifest (Stratum 4).

### Tasks

1. **`ToolRegistry` implementation**:
   - Store tool definitions with schema and trust level requirements
   - Whitelist enforcement: only manifested tools are callable

2. **`ToolGateway` implementation**:
   - Intercept: log invocation, check policy
   - Validate: JSON Schema validation of parameters, trust level check
   - Execute: subprocess execution with isolation appropriate to trust level
   - Log: emit `ToolCalled`, `ToolValidated`, `ToolExecuted` or `ToolFailed` events

3. **Error handling**:
   - Structured errors with remediation hints
   - Configurable retry with exponential backoff
   - HITL escalation after N failures

4. **Constraint enforcement tools**: linter/test runner tools that produce remediation-rich output.

5. **Tests**:
   - Validation rejects bad parameters with helpful errors
   - Trust level enforcement (sandboxed can't write, etc.)
   - Retry and escalation behaviour
   - Full intercept-validate-execute-log pipeline

### Deliverables

- `ToolGateway` and `ToolRegistry` implementations in `stratum-tools`
- Trust level enforcement
- Error handling with remediation

---

## Phase 8: Sub-Agent Orchestrator + rfbmq (5 days)

### Goal

Implement sub-agent spawning, rfbmq-backed task dispatch, spawn patterns, and depth bounds (Stratum 5).

### Tasks

1. **`RfbmqDispatcher` implementing `TaskDispatch`**:
   - Thin wrapper around `rfbmq::Queue`
   - Map `DispatchOptions` to rfbmq `Message` headers
   - `list_ready()` for dependency-aware scheduling

2. **`Orchestrator` implementation**:
   - `spawn()`: create sub-agent StratumRun with narrower scope
   - `await_result()`: monitor sub-agent completion, return compressed result
   - Depth tracking and enforcement

3. **Four spawn patterns**:
   - Delegate: single sub-agent, bounded task
   - Pipeline: sequential chain, each consuming prior's output
   - Parallel: concurrent sub-agents, supervisor merges
   - Janitor: scheduled invariant enforcement

4. **Queue topology**: create and manage `tasks/`, `results/`, `janitor/`, `dead-letter/` queues.

5. **Tests**:
   - Dependency DAG: A -> B -> C ordering enforced via `list_ready()`
   - Reply-To routing: results arrive in correct queue
   - Spawn depth enforcement
   - Each spawn pattern end-to-end

### Deliverables

- `RfbmqDispatcher` implementing `TaskDispatch`
- `Orchestrator` implementation with all 4 spawn patterns
- Depth enforcement and crash recovery

---

## Phase 9: HITL Controller (3 days)

### Goal

Implement gate management, decision recording, pause/resume, and notifications (Stratum 6).

### Tasks

1. **`HitlController` implementation** in `stratum-core`:
   - `open_gate()`: create HitlRecord, transition run to Paused, emit event
   - `record_decision()`: store decision, transition run based on decision type
   - `pending_gates()`: query for open gates
   - Durable pauses: run persists indefinitely in Paused state

2. **Gate policy engine**: evaluate gate categories against configured policy.

3. **`Notifier` implementations**:
   - Stdout notifier (default)
   - Webhook notifier (HTTP POST)

4. **Tests**:
   - Gate triggers pause, decision resumes
   - Each decision type (Approve, Modify, Redirect, Abort) produces correct state transition
   - Durable pause: create gate, "restart" process, gate still pending
   - Policy engine applies correct defaults

### Deliverables

- `HitlController` and `Notifier` implementations
- Gate policy engine
- Durable pause/resume

---

## Phase 10: CLI + Integration Wiring (4 days)

### Goal

Wire all strata together into a functional CLI binary.

### Tasks

1. **CLI commands** using clap:
   - `stratum run <task>` -- create and execute a full Stratum run
   - `stratum resume <run-id>` -- resume a checkpointed run
   - `stratum status [run-id]` -- show run state, progress, context budget
   - `stratum trajectory <run-id>` -- query and display trajectory events
   - `stratum export <run-id> --format jsonl|csv|replay` -- export trajectory
   - `stratum gates` -- list pending HITL gates
   - `stratum decide <run-id> <decision>` -- record a HITL decision
   - `stratum queue depth` -- show orchestrator queue stats

2. **Dependency injection**: wire all adapters to port traits at startup.

3. **Configuration**: environment variables and/or config file for:
   - API keys (Anthropic, OpenAI)
   - Model selection
   - Trust level defaults
   - Queue and DB paths
   - HITL policy overrides

4. **Structured logging** with tracing crate.

5. **End-to-end integration test**: task -> init -> run -> tool calls -> checkpoint -> resume -> complete.

### Deliverables

- Complete CLI binary
- Configuration system
- End-to-end integration test passing

---

## Phase 11: Observability Dashboard (3 days)

### Goal

Implement the live observability surface (Stratum 7 continuation).

### Tasks

1. **Metrics collection**: wire all strata to emit Prometheus-compatible metrics:
   - Run state gauge
   - Context budget utilisation by slot
   - KV-cache hit rate estimate
   - Tool call rate and error rate
   - Context Hygiene Score trend
   - HITL queue depth
   - Sub-agent tree depth
   - Cost-per-run counter

2. **Prometheus endpoint**: optional HTTP endpoint serving `/metrics`.

3. **CLI dashboard**: `stratum dashboard` command showing live metrics in terminal (tui-rs or similar).

4. **Tests**: metrics emit correctly for known event sequences.

### Deliverables

- Prometheus-compatible metrics export
- Terminal dashboard
- Metrics integration tests

---

## Phase 12: Testing & Hardening (5 days)

### Goal

Comprehensive testing, crash recovery, concurrency, and performance validation across all strata.

### Tasks

1. **Crash recovery tests**: kill process mid-run, verify checkpoint restore and `reap()` recovery.

2. **Concurrent sub-agent tests**: multiple sub-agents racing to claim tasks, verify correct isolation.

3. **Context compaction tests**: simulate long runs, verify compaction triggers and information preservation.

4. **Memory tier tests**: cross-session memory persistence, promotion flow, search quality.

5. **HITL durability tests**: gates survive process restart, decisions applied correctly after resume.

6. **Performance benchmarks**:
   - Event emission throughput
   - Context assembly latency
   - rfbmq enqueue/dequeue throughput with dependencies
   - Memory search latency

7. **Security audit**: verify trust level enforcement, no sandbox escapes, no tool manifest bypasses.

8. **Edge cases**: empty runs, max depth spawning, budget exhaustion, all gates triggered simultaneously.

### Deliverables

- Test suite with >80% coverage
- Benchmark results documented
- Security audit report

---

## Phase 13: Documentation & Release (3 days)

### Goal

User-facing documentation and first release.

### Tasks

1. **README** with quick start guide, architecture overview, and examples.

2. **CLI reference** with all commands documented.

3. **Configuration guide** with all options and defaults.

4. **Architecture link** to SAD v3.0.

5. **Trajectory export guide** for fine-tuning pipeline integration.

6. **Release**: `cargo build --release`, create GitHub release v0.1.0.

### Deliverables

- Complete documentation
- v0.1.0 release binary

---

## Risk Register

| # | Risk | Likelihood | Impact | Mitigation |
|---|------|-----------|--------|------------|
| R1 | rfbmq is a young crate | Medium | Medium | Same org/workspace -- bugs fixable directly. Contract tests in Phase 8 catch issues early. |
| R2 | Context compaction loses critical information | Medium | High | Stages 1-2 are lossless. Stage 3 preserves structured skeleton. Full history archived to Trajectory Store for recovery. |
| R3 | Embedding model quality affects hygiene score and memory search | Medium | Medium | Pluggable via LlmClient. Can use local models or API. Fallback to BM25-only search. |
| R4 | 9-crate workspace increases build complexity | Low | Low | Workspace-level caching. Most crates are small. Incremental compilation handles it. |
| R5 | LLM API cost overruns during long runs | Medium | Medium | Token budgets per run. Context budget ceilings. Compaction reduces token usage. Configurable model selection. |
| R6 | Sub-agent depth causes cascading complexity | Low | High | Default depth limit of 2. Hard enforcement. Each sub-agent has narrower scope and restricted tools. |
| R7 | HITL gates block long-running autonomous tasks | Medium | Medium | Configurable policies. Async notifications. Conservative defaults with relaxation path. |
| R8 | SQLite write contention with concurrent sub-agents | Low | Medium | WAL mode. Separate DB per run for isolation. State updates are infrequent. |
| R9 | rusqlite cross-compilation to musl | Low | Medium | Use bundled SQLite feature; well-tested path. |

---

## Timeline Visualisation

```
Week 1:  ██████ Phase 1 (Foundation)
Week 2:  ██████ Phase 2 (Trajectory Store)
         ██ Phase 4 (LLM Adapters, starts)
Week 3:  ████ Phase 4 (cont.)
         ████████ Phase 3 (Session Lifecycle)
Week 4:  ██████████ Phase 5 (Context Engine)
Week 5:  ██████████ Phase 5 (cont.)
         ██████████ Phase 6 (Memory Hierarchy)
Week 6:  ██████████ Phase 6 (cont.)
         ████████ Phase 7 (Tool Gateway)
Week 7:  ████████ Phase 7 (cont.)
         ██████████ Phase 8 (Orchestrator + rfbmq)
Week 8:  ██████████ Phase 8 (cont.)
         ██████ Phase 9 (HITL Controller)
Week 9:  ████████ Phase 10 (CLI + Integration)
         ██████ Phase 11 (Observability)
Week 10: ██████████ Phase 12 (Testing & Hardening)
         ██████ Phase 13 (Documentation & Release)
```

**Total: ~50 working days (~10 weeks)**

---

## PRD Traceability Matrix

Every PRD stratum has a corresponding crate, port trait(s), and implementation phase:

| PRD Stratum | Crate | Port Traits | Phase |
|-------------|-------|-------------|-------|
| 1 - Session Lifecycle | `stratum-core` | `SessionManager` | 3 |
| 2 - Context Engine | `stratum-context` | `ContextEngine` | 5 |
| 3 - Memory Hierarchy | `stratum-memory` | `MemoryStore`, `SkillLoader` | 6 |
| 4 - Tool Execution Gateway | `stratum-tools` | `ToolGateway`, `ToolRegistry` | 7 |
| 5 - Sub-Agent Orchestrator | `stratum-orchestrator` | `Orchestrator`, `TaskDispatch` | 8 |
| 6 - HITL Controller | `stratum-core` | `HitlController`, `Notifier` | 9 |
| 7 - Trajectory Store | `stratum-core` + `stratum-adapters` | `TrajectoryStore`, `MetricsExporter` | 2, 11 |

### PRD Design Principles Coverage

| Principle | SAD/Plan Coverage |
|-----------|------------------|
| P1 - Harness does not reason | All intelligence delegated to LlmClient. Harness provides structure only. |
| P2 - Build to delete | Port traits enable removing any stratum implementation. Each is independently swappable. |
| P3 - Constrain to accelerate | Tool manifest whitelist, trust levels, budget ceilings, depth bounds. |
| P4 - KV-cache economics | Context Engine: immutable prefix, static tool defs, append-only context. |
| P5 - Files are the durable API | Project tier memory: Markdown files, VCS-compatible, human-readable. |
| P6 - Errors teach | Tool Gateway: remediation hints on all failures. Errors stay in context. |
| P7 - Trajectories compound | Trajectory Store built first. Every event captured. Export for fine-tuning. |
