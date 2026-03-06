# Stratum

A model-agnostic agent harness with 7 independently swappable strata, built in Rust.

Stratum uses hexagonal (ports-and-adapters) architecture to decouple every layer of an AI agent system — from session management to tool execution to human-in-the-loop gates. Swap any stratum without touching the others.

## Features

| Stratum | Layer | What It Does |
|---------|-------|-------------|
| 1 | **Session Manager** | Run lifecycle, checkpointing, state machine transitions |
| 2 | **Context Engine** | 5-slot context assembly, budget checking, 3-stage compaction |
| 3 | **Memory Hierarchy** | 4-tier memory (Working / Episodic / Project / Global) with FTS5 search |
| 4 | **Tool Gateway** | Schema validation, trust enforcement, retry with backoff |
| 5 | **Sub-Agent Orchestrator** | Delegate / Pipeline / Parallel / Janitor spawn patterns via rfbmq |
| 6 | **HITL Controller** | 7 gate categories, policy engine, webhook notifications |
| 7 | **Trajectory Store** | Full event capture, Prometheus metrics, JSONL/CSV/Replay export |

**Cross-cutting:** LLM client adapters (Anthropic, OpenAI Chat, OpenAI Responses), constraint enforcement, artefact validation.

## Quick Start

### Prerequisites

- Rust 1.75+ (2021 edition)
- An API key for your LLM provider

### Install

```bash
git clone https://github.com/chamuka/stratum.git
cd stratum
cargo build --release
```

The binary is at `target/release/stratum`.

### Configure

Set your API key:

```bash
export STRATUM_API_KEY=sk-your-key-here
```

Or create a `stratum.yaml` in your working directory:

```yaml
llm_provider: anthropic
api_key: sk-your-key-here
model: claude-sonnet-4-20250514
trust_level: supervised
```

See the [Configuration Guide](docs/configuration-guide.md) for all options.

### Run

```bash
# Start an agent run
stratum run "Implement a REST API for user management"

# Check status
stratum status

# View trajectory events
stratum trajectory <run_id>

# Export for fine-tuning
stratum export <run_id> --format jsonl > training.jsonl
```

See the [CLI Reference](docs/cli-reference.md) for all 11 commands.

## Architecture

Stratum follows hexagonal architecture. All port traits live in `stratum-core` (pure abstractions, no implementations). Concrete implementations live in their respective crates or in `stratum-adapters`.

```
                    +------------------+
                    |   stratum-cli    |  CLI binary
                    +--------+---------+
                             |
                    +--------+---------+
                    |  TurnExecutor    |  Core loop
                    +--------+---------+
                             |
     +-------+-------+------+------+-------+-------+
     |       |       |      |      |       |       |
   [S1]    [S2]    [S3]   [S4]   [S5]    [S6]    [S7]
  Session Context Memory  Tool  Orch.   HITL   Trajectory
  Manager Engine  Store  Gateway        Ctrl    Store
     |       |       |      |      |       |       |
     +-------+-------+------+------+-------+-------+
                             |
                    +--------+---------+
                    | stratum-adapters |  SQLite, LLM, etc.
                    +------------------+
```

For the full architecture, see the [System Architecture Document](docs/stratum-sad-v3.0.md).

### Workspace (9 crates)

| Crate | Role |
|-------|------|
| `stratum-types` | All shared domain types (RunId, StratumRun, EventType, etc.) |
| `stratum-core` | Pure port traits only — defines boundaries for all 7 strata + TurnExecutor |
| `stratum-context` | Stratum 2: Context Engine |
| `stratum-memory` | Stratum 3: Memory Hierarchy |
| `stratum-tools` | Stratum 4: Tool Gateway |
| `stratum-orchestrator` | Stratum 5: Sub-Agent Orchestrator + rfbmq |
| `stratum-adapters` | Concrete implementations (SQLite, LLM clients, HITL, Trajectory) |
| `stratum-cli` | CLI binary |
| `stratum-test-utils` | Mock implementations of all port traits |

### Key Design Decisions

- **EventType is a flat enum** — unit variants only; event data goes in `TrajectoryEvent::payload`
- **FrozenToolRegistry** — builder allows mutation during init; frozen registry is immutable after (protects KV-cache economics)
- **Two-prompt pattern** — Initialiser produces TASK.md, PROGRESS.md, DECISIONS.md; Worker executes against them
- **All state transitions emit trajectory events** — full observability by default

## Documentation

- [CLI Reference](docs/cli-reference.md) — All 11 commands with usage and examples
- [Configuration Guide](docs/configuration-guide.md) — Config file, env vars, defaults
- [Trajectory Export Guide](docs/trajectory-export-guide.md) — Export formats and fine-tuning integration
- [System Architecture Document](docs/stratum-sad-v3.0.md) — Full architecture
- [Product Requirements](docs/stratum-prd-v2.1.md) — PRD v2.1
- [Implementation Plan](docs/stratum-plan-v3.0.md) — 13-phase plan

## Development

```bash
cargo build                    # Build all crates
cargo test                     # Run all tests (377)
cargo clippy --all-targets     # Lint
cargo fmt --check              # Format check
```

## License

MIT
