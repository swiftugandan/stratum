# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build                          # Build all crates
cargo test                           # Run all tests (33 currently)
cargo test -p stratum-adapters       # Test a single crate
cargo test test_name                 # Run a single test by name
cargo clippy --all-targets           # Lint (must pass clean)
cargo fmt --check                    # Format check
cargo fmt                            # Auto-format
```

## Architecture

Stratum is a model-agnostic agent harness with 7 independently swappable strata (layers), built in Rust using hexagonal (ports-and-adapters) architecture with Tokio async.

### Workspace: 9 crates

| Crate | Role |
|-------|------|
| `stratum-types` | All shared domain types (RunId, StratumRun, EventType, etc.) |
| `stratum-core` | **Pure port traits only** — no implementations. Defines boundaries for all 7 strata + TurnExecutor |
| `stratum-context` | Stratum 2: Context Engine |
| `stratum-memory` | Stratum 3: Memory Hierarchy |
| `stratum-tools` | Stratum 4: Tool Gateway |
| `stratum-orchestrator` | Stratum 5: Sub-Agent Orchestrator + rfbmq |
| `stratum-adapters` | Concrete implementations (SQLite stores, LLM clients, prompts). Houses Strata 1, 6, 7 impls |
| `stratum-cli` | CLI binary |
| `stratum-test-utils` | Mock implementations of all port traits |

### Key architectural rules

- **stratum-core is pure abstractions.** Never put implementations there — only trait definitions.
- **Implementations go in stratum-adapters** (for cross-cutting concerns like SQLite, LLM) or in the stratum-specific crate.
- **EventType is a flat enum** (unit variants only). Event-specific data goes in `TrajectoryEvent::payload` as `serde_json::Value`.
- **FrozenToolRegistry pattern**: `ToolRegistryBuilder` allows mutation during init; `FrozenToolRegistry` is immutable after. Protects KV-cache economics.
- **RunArtefacts**: Two-prompt pattern — Initialiser produces TASK.md, PROGRESS.md, DECISIONS.md; Worker executes.
- **All state transitions emit trajectory events.**

### The 7 Strata (port traits in stratum-core/src/ports.rs)

1. **SessionManager** — Run lifecycle, checkpointing, state transitions
2. **ContextEngine** — Context assembly, budget checking, compaction
3. **MemoryStore + SkillLoader** — Tiered memory (Working/Episodic/Project/Global)
4. **ToolGateway + FrozenToolRegistry** — Single `call_tool()` entry point (no temporal coupling)
5. **Orchestrator + TaskDispatch** — Sub-agent spawning, rfbmq integration
6. **HitlController + Notifier** — Human-in-the-loop gates and decisions
7. **TrajectoryStore + MetricsExporter** — Event capture and observability

Cross-cutting: **LlmClient**, **ArtefactValidator**, **ConstraintEnforcer**

### Key files

- `crates/stratum-types/src/lib.rs` — All domain types
- `crates/stratum-core/src/ports.rs` — All port traits
- `crates/stratum-core/src/turn.rs` — TurnExecutor trait (orchestrates a single agent turn)
- `crates/stratum-adapters/src/session.rs` — SqliteSessionManager (state machine with validated transitions)
- `crates/stratum-adapters/src/trajectory.rs` — SqliteTrajectoryStore
- `crates/stratum-adapters/src/prompt.rs` — Initialiser/Worker prompt templates
- `crates/stratum-test-utils/src/mocks.rs` — All mock implementations

## Documentation

- **PRD (read-only, never modify):** `docs/stratum-prd-v2.1.md`
- **Architecture:** `docs/stratum-sad-v3.0.md`
- **Implementation plan:** `docs/stratum-plan-v3.0.md` (13 phases, ~50 days)

## Current State

Phases 1-12 complete: workspace structure, domain types, all port traits, mocks, SqliteTrajectoryStore, InMemoryMetrics, SqliteSessionManager, two-prompt pattern, LLM client adapters, Context Engine, Memory Hierarchy, Tool Gateway, Sub-Agent Orchestrator, HITL Controller, CLI + Integration Wiring, Observability Dashboard, Testing & Hardening (377 tests, clippy + fmt clean). Next up: Phase 13 (Documentation & Release).

## Ways of Working

1. **All new code must be covered by tests** (unit + integration)
2. **All public APIs must be documented** (Rustdoc)
3. **No need for backward compatibility guards, we are in active development**
