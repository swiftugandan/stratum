# Stratum -- Software Architecture Document v4.0

> **Version 4.0** -- complete rewrite reflecting the lean 3-crate architecture.
> Supersedes SAD v3.0 which described a 9-crate, 7-stratum design.

---

## 1. Introduction

### 1.1 Purpose

Stratum is a model-agnostic autonomous agent harness built in Rust. It manages the full agent runtime: session lifecycle, context assembly, 2-tier memory, tool execution with schema validation and retry, sub-agent orchestration, and trajectory capture.

### 1.2 Scope

This document covers the architecture of Stratum v4.0: its 3-crate decomposition, domain types, port traits, implementation modules, built-in tools, CLI commands, data model, and dependencies.

### 1.3 Key Changes from v3.0

| Aspect | SAD v3.0 | SAD v4.0 |
|--------|----------|----------|
| Crates | 9 (types, core, context, memory, tools, orchestrator, adapters, cli, test-utils) | 3 (core, engine, cli) |
| Memory | 4-tier (Working / Episodic / Project / Global) | 2-tier (Working / Persistent) |
| Context engine | 5-slot assembly, 3-stage compaction, hygiene scoring | Simple: system + goal + last N history messages |
| HITL | 7 gate categories, policy engine | Removed (fully autonomous) |
| Metrics | Prometheus metrics, TUI dashboard, HTTP server | Removed |
| Trust levels | Sandboxed / Supervised / Autonomous | Removed (always autonomous) |
| CLI commands | 13 | 4 (start, submit, stop, status) |
| LLM providers | Anthropic, OpenAI Chat, OpenAI Responses | Anthropic only |

---

## 2. Constraints

| ID | Constraint | Rationale |
|----|-----------|-----------|
| C1 | Single static binary | Simplifies deployment; no runtime dependencies |
| C3 | SQLite for persistent state | Proven embedded DB; single-file state |
| C4 | No unsafe code | All dependencies are safe Rust |
| C5 | Tokio async runtime | Required by LLM API clients (reqwest) |
| C6 | Hexagonal / ports-and-adapters | Testability; swap providers without touching core logic |
| C7 | Message format: Markdown with RFC 822 headers | Human-readable, git-friendly (rfbmq messages) |

---

## 3. Architecture Overview

Stratum follows hexagonal (ports-and-adapters) architecture:

- **`stratum-core`** -- Pure abstractions: domain types + port traits. No implementations.
- **`stratum-engine`** -- All implementations: session, memory, tools, LLM, orchestrator, trajectory, context, prompt, registry, dispatch, executor.
- **`stratum-cli`** -- CLI binary (`stratum`) with 4 commands, config, wiring, daemon loop, run loop, turn executor.

### Crate Dependency Diagram

```
stratum-cli
  |
  +---> stratum-engine
  |       |
  |       +---> stratum-core
  |
  +---> stratum-core
```

### Data Flow (Single Turn)

```
DaemonLoop
  -> drain_pending()
    -> run_from_queue()
      -> worker_loop()
        -> execute_turn()
          1. SessionManager::get_run()
          2. context::assemble_context()
          3. LlmClient::complete()
          4. ToolGateway::call_tool() (for each tool call)
          5. SessionManager::checkpoint()
```

---

## 4. Domain Types

All types are defined in `stratum-core/src/lib.rs`.

### Identity & State

| Type | Description |
|------|-------------|
| `RunId` | `Uuid` alias for run identification |
| `ModelRef` | `String` alias for model identifiers |
| `RunState` | Enum: `Running`, `Completed`, `Failed`, `Aborted` |
| `StratumRun` | Full run record: id, parent, model, tools, budget, depth limit, state, timestamp |

### Context

| Type | Description |
|------|-------------|
| `ContextBudget` | `max_tokens` (default: 128,000), `max_history_messages` (default: 50) |

### Tools

| Type | Description |
|------|-------------|
| `ToolDefinition` | name, description, JSON schema |
| `ToolInvocation` | tool_name, parameters, run_id |
| `ToolResult` | tool_name, status, output, remediation_hint, latency_ms |
| `ToolResultStatus` | Enum: `Success`, `ValidationFailure`, `ExecutionError` |

### Trajectory

| Type | Description |
|------|-------------|
| `TrajectoryEvent` | event_id, run_id, parent_run_id, timestamp, event_type, stratum_layer, payload |
| `EventType` | 23-variant flat enum (unit variants only; data goes in payload) |
| `StratumLayer` | Enum: `Session`, `Context`, `Memory`, `Tools`, `Orchestrator`, `Trajectory` |

### Memory

| Type | Description |
|------|-------------|
| `MemoryTier` | Enum: `Working`, `Persistent` |
| `MemoryEntry` | id, tier, content, metadata, created_at, updated_at |
| `MemorySearchResult` | entry + relevance_score |

### Checkpoints

| Type | Description |
|------|-------------|
| `Checkpoint` | id, run_id, state, goal, context_summary, tool_call_log, created_at |

### Orchestrator

| Type | Description |
|------|-------------|
| `SpawnPattern` | Enum: `Delegate`, `Pipeline`, `Parallel`, `Janitor` |
| `SpawnConfig` | pattern, model_ref, tool_manifest, task_goal, shared_filesystem_scope |
| `SubAgentResult` | run_id, status, summary, artefacts |

### Dispatch

| Type | Description |
|------|-------------|
| `TaskPriority` | Enum: `Critical`, `High`, `Normal` (default), `Low` |
| `DispatchOptions` | priority, tags, correlation_id, reply_to, depends_on, ttl |
| `ClaimedTask` | id, body, reply_to |

### LLM

| Type | Description |
|------|-------------|
| `LlmMessage` | role, content |
| `LlmResponse` | content, tool_calls, usage, stop_reason |
| `LlmToolCall` | id, tool_name, arguments |
| `LlmUsage` | input_tokens, output_tokens, cached_tokens |

### Other

| Type | Description |
|------|-------------|
| `Skill` | name, description, trigger_conditions, content |
| `TurnOutcome` | Enum: `Response`, `ToolCalls`, `Completed`, `Error` |

---

## 5. Port Traits

All traits are in `stratum-core/src/lib.rs::ports`. All async traits use `async_trait`.

### SessionManager

```rust
async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error>;
async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error>;
async fn transition_state(&self, run_id: RunId, new_state: RunState) -> Result<(), Self::Error>;
async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error>;
async fn get_latest_checkpoint(&self, run_id: RunId) -> Result<Option<Checkpoint>, Self::Error>;
```

### MemoryStore

```rust
async fn write(&self, tier: MemoryTier, entry: &MemoryEntry) -> Result<(), Self::Error>;
async fn search(&self, tier: MemoryTier, query: &str, limit: usize) -> Result<Vec<MemorySearchResult>, Self::Error>;
```

### ToolGateway

```rust
async fn call_tool(&self, invocation: ToolInvocation) -> Result<ToolResult, Self::Error>;
```

### Orchestrator

```rust
async fn spawn(&self, parent_run_id: RunId, config: SpawnConfig) -> Result<RunId, Self::Error>;
async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error>;
fn current_depth(&self, run_id: RunId) -> Result<u32, Self::Error>;
```

### TaskDispatch (sync trait)

```rust
fn enqueue(&self, body: &str, opts: DispatchOptions) -> Result<String, Self::Error>;
fn list_ready(&self) -> Result<Vec<String>, Self::Error>;
fn dequeue(&self) -> Result<Option<ClaimedTask>, Self::Error>;
fn complete(&self, task: &ClaimedTask) -> Result<(), Self::Error>;
fn fail(&self, task: &ClaimedTask) -> Result<(), Self::Error>;
fn depth(&self) -> Result<i64, Self::Error>;
```

### TrajectoryStore

```rust
async fn emit_event(&self, event: TrajectoryEvent) -> Result<(), Self::Error>;
async fn query_events(&self, run_id: Option<RunId>, event_type: Option<EventType>, limit: Option<usize>) -> Result<Vec<TrajectoryEvent>, Self::Error>;
```

### LlmClient

```rust
async fn complete(&self, messages: &[LlmMessage], model: &str, tools: Option<&[ToolDefinition]>) -> Result<LlmResponse, Self::Error>;
```

### TurnExecutor

```rust
async fn execute_turn(&self, run_id: RunId) -> Result<TurnOutcome, Self::Error>;
```

---

## 6. Implementations (stratum-engine)

All modules are in `crates/stratum-engine/src/`. All implementations use `EngineError` as their error type.

### 6.1 SqliteSessionManager (`session.rs`)

Implements `SessionManager`. SQLite-backed with WAL mode.

- **State machine**: `Running -> Completed | Failed | Aborted` (3 valid transitions)
- **Tables**: `runs` (id, parent_run_id, model_ref, tool_manifest, context_budget, spawn_depth_limit, state, created_at, updated_at), `checkpoints` (id, run_id, state, goal, context_summary, tool_call_log, created_at)
- All DB operations run via `tokio::task::spawn_blocking`

### 6.2 TwoTierMemoryStore (`memory.rs`)

Implements `MemoryStore`. Two tiers:

- **Working**: In-memory `HashMap<String, MemoryEntry>` behind `RwLock`. Case-insensitive substring search.
- **Persistent**: SQLite with FTS5 virtual table for full-text search. BM25 ranking. Upsert semantics.

### 6.3 DefaultToolGateway (`gateway.rs`)

Implements `ToolGateway`. Generic over `E: ToolExecutor`. 3-stage pipeline:

1. **Intercept** -- Look up tool in registry; return error if not found
2. **Validate** -- JSON Schema validation via `jsonschema` crate
3. **Execute** -- Run via executor with retry (exponential backoff + jitter, max 3 retries)

Configuration: `validate_schema`, `max_retries`, `retry_base_delay`, `retry_max_delay`.

### 6.4 PersistentToolRegistry (`registry.rs`)

SQLite-backed tool registry. Supports:

- `register_builtins()` -- Load built-in tool definitions (in-memory only, not persisted to DB)
- `register_dynamic()` -- Register agent-created tools (persisted to `agent_tools` table)
- `validate_params()` -- JSON Schema validation with compiled validators
- `get_manifest_owned()` / `get_tool_owned()` -- Read from in-memory cache
- `get_dynamic_tool_info()` -- Retrieve script_path and param_passing for dynamic tools
- Reloads dynamic tools from DB on construction

### 6.5 BuiltinExecutor / SubprocessExecutor / CompositeExecutor

Three executor implementations of the `ToolExecutor` trait:

- **BuiltinExecutor** (`tools/mod.rs`): Handles 9 built-in tools directly. Stateful tools (create_tool, create_skill, memory_write, memory_search) return `_builtin_action` markers instead of executing directly -- the run loop processes these markers with access to `AppContext`.
- **SubprocessExecutor** (`subprocess.rs`): Runs external tool scripts as child processes. Supports 3 parameter passing modes: `JsonArg`, `Stdin`, `CliFlags`. Configurable timeout (default 30s), retryable exit codes (69, 75), max output capture (1MB).
- **CompositeExecutor** (`subprocess.rs`): Routes to BuiltinExecutor if `handles(tool_name)` is true, otherwise falls back to SubprocessExecutor.

### 6.6 AnthropicClient (`llm.rs`)

Implements `LlmClient`. Anthropic Messages API client:

- Retry with exponential backoff + jitter on transient errors (429, 500, 503, 529)
- Extracts system messages from the message list (Anthropic API uses a separate `system` field)
- Parses text and tool_use content blocks
- Configurable: base URL, retry config, max_tokens

### 6.7 Context Assembly (`context.rs`)

`assemble_context()` builds the message list for LLM calls:

1. System prompt (role: system)
2. Task goal (role: user)
3. Last N history messages, truncating oldest when over `max_history_messages` budget

### 6.8 Prompt Templates (`prompt.rs`)

`daemon_system_anchor()` generates the daemon system prompt. Describes capabilities (bash, file ops, memory, tool/skill creation), lists available tools, and defines operating principles (autonomy, memory usage, incremental work, "TASK COMPLETE" signal).

### 6.9 SqliteTrajectoryStore (`trajectory.rs`)

Implements `TrajectoryStore`. SQLite-backed with WAL mode.

- **Table**: `trajectory_events` (event_id, run_id, parent_run_id, timestamp, event_type, stratum_layer, payload)
- **Indexes**: run_id, event_type, timestamp
- `query_events()` supports filtering by run_id, event_type, and limit
- Thread-safe via `Arc<Mutex<Connection>>`

### 6.10 RfbmqDispatcher (`dispatch.rs`)

Implements `TaskDispatch` (sync). File-based message queue backed by `rfbmq-core`:

- `enqueue()` -- Create message with priority, tags, correlation_id, reply_to, depends_on, ttl
- `dequeue()` -- Claim next ready message, parse body, track claimed message for completion
- `complete()` / `fail()` -- Finish claimed task
- `depth()` -- Current queue size
- `init_or_open()` -- Create queue if it doesn't exist, open if it does

### 6.11 DefaultOrchestrator (`orchestrator.rs`)

Implements `Orchestrator`. Generic over `S: SessionManager`:

- `spawn()` -- Create child run with depth enforcement. Walks parent chain to compute depth. Hard max depth (default: 5) and per-run spawn_depth_limit.
- `await_result()` -- Poll child run state until terminal, with configurable timeout (default: 300s)
- `current_depth()` -- Read from cache
- Depth cache (`RwLock<HashMap<RunId, u32>>`) avoids repeated parent chain walks

### 6.12 Error Type (`error.rs`)

`EngineError` enum with variants: `Sqlite`, `Serialization`, `Join`, `NotFound`, `InvalidState`, `Http`, `LlmApi`, `ToolNotFound`, `ValidationFailed`, `ExecutionFailed`, `SchemaCompilation`, `DepthLimitExceeded`, `Dispatch`, `Timeout`.

---

## 7. Built-in Tools

9 built-in tools, all defined in `crates/stratum-engine/src/tools/`:

| Tool | Module | Description |
|------|--------|-------------|
| `bash` | `bash.rs` | Execute shell commands |
| `read_file` | `file_ops.rs` | Read file contents |
| `write_file` | `file_ops.rs` | Write/create files |
| `list_directory` | `file_ops.rs` | List directory contents |
| `search_files` | `file_ops.rs` | Search files by pattern |
| `create_tool` | `create_tool.rs` | Register a new dynamic tool from a script |
| `create_skill` | `create_skill.rs` | Create a skill definition file |
| `memory_write` | `memory.rs` | Write to working or persistent memory |
| `memory_search` | `memory.rs` | Search working or persistent memory |

### `_builtin_action` Marker Pattern

Stateful tools (create_tool, create_skill, memory_write, memory_search) cannot execute fully within the executor because they need access to `AppContext` (registry, memory store, filesystem). Instead:

1. `BuiltinExecutor` validates parameters and returns a JSON object with `_builtin_action` key
2. The run loop in `run_loop.rs` detects `_builtin_action` in tool results
3. `process_builtin_action()` executes the actual side effect (e.g., registering the tool, writing memory)
4. The result is optionally replaced with the actual output (e.g., memory search results)

---

## 8. CLI Binary (stratum-cli)

### 8.1 Commands (`cli.rs`)

4 commands defined via Clap derive:

- `start` -- Launch daemon
- `submit <goal> [--priority] [--tag]...` -- Enqueue task
- `stop` -- SIGTERM to daemon via PID file
- `status` -- Show PID + queue depth

### 8.2 Configuration (`config.rs`)

`StratumConfig` with 5 fields: `api_key`, `model`, `max_tokens`, `data_dir`, `max_concurrent_runs`. Loaded from YAML file with env var overrides. Helper methods: `db_path()`, `queue_root()`, `pid_file()`, `skills_dir()`.

### 8.3 Wiring (`wiring.rs`)

`AppContext` constructs all concrete implementations:

```
AppContext {
    config:         StratumConfig
    session:        Arc<SqliteSessionManager>
    trajectory:     Arc<SqliteTrajectoryStore>
    llm:            Arc<AnthropicClient>
    memory:         Arc<TwoTierMemoryStore>
    tool_registry:  Arc<PersistentToolRegistry>
    tool_gateway:   Arc<DefaultToolGateway<CompositeExecutor>>
    dispatch:       Arc<RfbmqDispatcher>
    orchestrator:   Arc<DefaultOrchestrator<SqliteSessionManager>>
}
```

### 8.4 Daemon Loop (`daemon.rs`)

`DaemonLoop` watches rfbmq `pending/` via `notify` filesystem events:

1. Write PID file, set up Ctrl+C signal handler
2. Set up filesystem watcher on `<queue_root>/pending/`
3. Drain existing pending tasks
4. Event loop: on watcher notification or 5s fallback, drain pending tasks and clean up completed runs
5. On shutdown: wait up to 60s for active runs, remove PID file

### 8.5 Run Loop (`run_loop.rs`)

`run_from_queue()` creates a run and enters `worker_loop()`:

- Max 100 turns per run
- Each turn calls `execute_turn()`, routes `TurnOutcome`:
  - `Completed` -- Transition to Completed state, return
  - `Response` -- Append to history, add "Continue with the next step." prompt
  - `ToolCalls` -- Process `_builtin_action` markers, feed results into history
  - `Error` -- Transition to Failed state, return error

### 8.6 Turn Executor (`turn_executor.rs`)

`execute_turn()` implements a single agent turn:

1. Get run from SessionManager
2. Assemble context (system prompt + goal + history)
3. Get tool definitions from registry
4. Call LLM
5. Route response: if tool calls, execute each via gateway and checkpoint; if text, check for completion signals ("TASK COMPLETE", "all items complete", "task is complete" with end_turn/stop reason)

---

## 9. Data Model

### 9.1 SQLite Databases

**`stratum.db`** (main database, path: `<data_dir>/stratum.db`):

| Table | Description |
|-------|-------------|
| `runs` | Run records (id, parent_run_id, model_ref, tool_manifest, context_budget, spawn_depth_limit, state, created_at, updated_at) |
| `checkpoints` | Checkpoint records (id, run_id, state, goal, context_summary, tool_call_log, created_at) |
| `trajectory_events` | Trajectory events (event_id, run_id, parent_run_id, timestamp, event_type, stratum_layer, payload) |

**`memory.db`** (memory database, path: `<data_dir>/memory.db`):

| Table | Description |
|-------|-------------|
| `persistent_memory` | FTS5 virtual table (id, content, metadata) |
| `persistent_memory_meta` | Metadata table (id, content, metadata, created_at, updated_at) |

**`tools.db`** (tool registry, path: `<data_dir>/tools.db`):

| Table | Description |
|-------|-------------|
| `agent_tools` | Dynamic tool definitions (name, description, schema, script_path, param_passing, created_at) |

### 9.2 Filesystem Layout

```
<data_dir>/
  stratum.db
  memory.db
  tools.db
  queues/
    pending/      # rfbmq pending messages
    claimed/      # rfbmq claimed (in-progress) messages
    done/         # rfbmq completed messages
    failed/       # rfbmq failed messages
  skills/         # Skill definitions (*.md)
  daemon.pid      # PID of running daemon
```

---

## 10. Dependencies

### stratum-core

| Dependency | Purpose |
|-----------|---------|
| `uuid` | Run and event IDs |
| `chrono` | Timestamps |
| `serde` / `serde_json` | Serialization |
| `async-trait` | Async trait definitions |

### stratum-engine

| Dependency | Purpose |
|-----------|---------|
| `stratum-core` | Domain types + port traits |
| `rusqlite` (bundled) | SQLite databases |
| `reqwest` | HTTP client for Anthropic API |
| `jsonschema` | Tool parameter validation |
| `rfbmq-core` | File-based message queue |
| `tokio` | Async runtime |
| `tiktoken-rs` | Token counting |
| `rand` | Jitter for retry delays |
| `tracing` | Structured logging |
| `thiserror` | Error derive macro |

### stratum-cli

| Dependency | Purpose |
|-----------|---------|
| `stratum-core` / `stratum-engine` | Core types + implementations |
| `clap` | CLI argument parsing |
| `serde_yaml` | Config file parsing |
| `notify` | Filesystem watcher (FSEvents/inotify) |
| `anyhow` | Error handling |
| `tracing-subscriber` | Log output |
