# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build                          # Build all crates
cargo test                           # Run all tests
cargo test -p stratum-engine         # Test a single crate
cargo test test_name                 # Run a single test by name
cargo clippy --all-targets           # Lint (must pass clean)
cargo fmt --check                    # Format check
cargo fmt                            # Auto-format
```

## Architecture

Stratum is a model-agnostic autonomous agent harness built in Rust with Tokio async. Hexagonal (ports-and-adapters) architecture: types + traits in `stratum-core`, all implementations in `stratum-engine`, CLI binary in `stratum-cli`.

### Workspace: 3 crates

| Crate | Role |
|-------|------|
| `stratum-core` | Domain types + port traits (no implementations) |
| `stratum-engine` | All implementations: session, memory, tools, LLM, orchestrator, trajectory, context, prompt, registry, dispatch |
| `stratum-cli` | CLI binary (`stratum`) with 4 commands: `start`, `submit`, `stop`, `status` |

### Key architectural rules

- **stratum-core is pure abstractions.** Types + trait definitions only, no implementations.
- **All implementations go in stratum-engine.**
- **EventType is a flat enum** (unit variants only). Event-specific data goes in `TrajectoryEvent::payload` as `serde_json::Value`.
- **2-tier memory**: Working (in-memory) + Persistent (SQLite + FTS5).
- **Built-in tools**: bash, file ops (read/write/list/search), memory (write/search), create_tool, create_skill.
- **Daemon mode**: `stratum start` watches rfbmq queue via `notify` (FSEvents/inotify), auto-runs tasks.
- **All state transitions emit trajectory events.**

### Port traits (in `stratum-core/src/lib.rs::ports`)

- **SessionManager** — Run lifecycle, checkpointing, state transitions
- **MemoryStore** — 2-tier memory (Working/Persistent)
- **ToolGateway** — Single `call_tool()` entry point
- **Orchestrator** — Sub-agent spawning
- **TaskDispatch** — rfbmq queue operations (sync trait)
- **TrajectoryStore** — Event capture
- **LlmClient** — LLM completion
- **TurnExecutor** — Core loop (context → LLM → tools → checkpoint)

### Key files

- `crates/stratum-core/src/lib.rs` — All domain types + port traits
- `crates/stratum-engine/src/session.rs` — SqliteSessionManager
- `crates/stratum-engine/src/trajectory.rs` — SqliteTrajectoryStore
- `crates/stratum-engine/src/memory.rs` — DefaultMemoryStore (Working + SQLite/FTS5)
- `crates/stratum-engine/src/gateway.rs` — DefaultToolGateway (validate → execute → log)
- `crates/stratum-engine/src/registry.rs` — ToolRegistryBuilder + FrozenToolRegistry
- `crates/stratum-engine/src/llm.rs` — AnthropicClient
- `crates/stratum-engine/src/context.rs` — Context assembly
- `crates/stratum-engine/src/prompt.rs` — System prompt templates
- `crates/stratum-engine/src/orchestrator.rs` — DefaultOrchestrator (sub-agent spawning)
- `crates/stratum-engine/src/dispatch.rs` — RfbmqDispatch (rfbmq queue adapter)
- `crates/stratum-engine/src/executor.rs` — BuiltinExecutor + SubprocessExecutor
- `crates/stratum-engine/src/tools/` — Built-in tool implementations (bash, file_ops, memory, create_tool, create_skill)
- `crates/stratum-engine/src/error.rs` — EngineError
- `crates/stratum-cli/src/cli.rs` — Clap CLI definition (4 commands)
- `crates/stratum-cli/src/config.rs` — StratumConfig (YAML + env)
- `crates/stratum-cli/src/wiring.rs` — AppContext construction
- `crates/stratum-cli/src/turn_executor.rs` — DefaultTurnExecutor
- `crates/stratum-cli/src/run_loop.rs` — Run loop (turn executor until complete)
- `crates/stratum-cli/src/daemon.rs` — DaemonLoop (rfbmq watcher, concurrent tasks)

## Documentation

- **PRD (read-only, never modify):** `docs/stratum-prd-v2.1.md`

## Ways of Working

1. **All new code must be covered by tests** (unit + integration)
2. **All public APIs must be documented** (Rustdoc)
3. **No need for backward compatibility guards, we are in active development**
