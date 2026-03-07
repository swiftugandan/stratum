# Stratum

A model-agnostic autonomous agent harness built in Rust.

## What is Stratum?

Stratum manages the full agent runtime -- session lifecycle, context assembly, 2-tier memory, tool execution with schema validation and retry, sub-agent orchestration, and trajectory capture. It uses hexagonal (ports-and-adapters) architecture: pure abstractions in `stratum-core`, all implementations in `stratum-engine`, CLI binary in `stratum-cli`.

## Quick Start

### Prerequisites

- Rust 1.75+ (2021 edition)
- An Anthropic API key

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
export STRATUM_API_KEY=sk-ant-your-key-here
```

Or create a `stratum.yaml` in your working directory:

```yaml
api_key: sk-ant-your-key-here
model: claude-sonnet-4-20250514
max_tokens: 4096
data_dir: .stratum
max_concurrent_runs: 4
```

See the [Configuration Guide](docs/configuration-guide.md) for all options.

### Run

```bash
# Start the daemon (foreground, watches rfbmq queue)
stratum start

# Submit a task from another terminal
stratum submit "Build a hello world web server" --priority high --tag web

# Check daemon status and queue depth
stratum status

# Stop the daemon
stratum stop
```

## Architecture

### 3 Crates

| Crate | Role |
|-------|------|
| `stratum-core` | Domain types + port traits (no implementations) |
| `stratum-engine` | All implementations: session, memory, tools, LLM, orchestrator, trajectory, context, prompt, registry, dispatch |
| `stratum-cli` | CLI binary (`stratum`) with 4 commands: `start`, `submit`, `stop`, `status` |

```
stratum-cli  -->  stratum-engine  -->  stratum-core
```

### Key Design Decisions

- **Hexagonal architecture** -- Port traits in core, all implementations in engine
- **EventType is a flat enum** -- Unit variants only; event-specific data goes in `TrajectoryEvent::payload`
- **2-tier memory** -- Working (in-memory HashMap) + Persistent (SQLite/FTS5)
- **PersistentToolRegistry** -- SQLite-backed, supports dynamic tool creation between turns
- **`_builtin_action` marker pattern** -- Stateful tools return markers; the run loop processes them with access to application context
- **Daemon mode** -- Persistent process watches rfbmq queue, auto-runs tasks with built-in tools
- **All state transitions emit trajectory events** -- Full observability by default

## Built-in Tools

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands |
| `read_file` | Read file contents |
| `write_file` | Write/create files |
| `list_directory` | List directory contents |
| `search_files` | Search files by pattern |
| `create_tool` | Register a new dynamic tool from a script |
| `create_skill` | Create a skill definition file |
| `memory_write` | Write to working or persistent memory |
| `memory_search` | Search working or persistent memory |

## Documentation

- [CLI Reference](docs/cli-reference.md) -- All 4 commands with usage and examples
- [Configuration Guide](docs/configuration-guide.md) -- Config file, env vars, defaults
- [Trajectory Events Reference](docs/trajectory-events-reference.md) -- Event types, schema, and querying
- [System Architecture Document](docs/stratum-sad-v4.0.md) -- Full architecture
- [Product Requirements](docs/stratum-prd-v2.1.md) -- PRD v2.1 (read-only)

## Development

```bash
cargo build                    # Build all crates
cargo test                     # Run all tests
cargo clippy --all-targets     # Lint
cargo fmt --check              # Format check
```

## License

MIT
