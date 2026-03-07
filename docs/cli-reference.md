# CLI Reference

Stratum provides 13 commands for managing agent runs, inspecting trajectory events, handling human-in-the-loop gates, monitoring metrics, and running a persistent daemon.

## Global Options

| Option | Description |
|--------|-------------|
| `--config <path>` | Path to configuration file (default: `stratum.yaml` in current directory) |
| `-h, --help` | Print help |

## Commands

### `stratum run`

Start a new agent run with the given task.

```
stratum run <task>
```

**Arguments:**
- `<task>` — The task goal for the agent (required)

**Requires:** API key (via `STRATUM_API_KEY` or config file)

**Example:**
```bash
stratum run "Implement a REST API for user management"
stratum run "Fix the failing test in src/auth.rs"
```

The run goes through two phases:
1. **Initialiser** — The LLM produces TASK.md, PROGRESS.md, and DECISIONS.md artefacts
2. **Worker loop** — The LLM executes turns against the task until completion, pause (HITL gate), or error

---

### `stratum resume`

Resume a paused or checkpointed run.

```
stratum resume <run_id>
```

**Arguments:**
- `<run_id>` — The UUID of the run to resume (required)

**Requires:** API key

**Example:**
```bash
stratum resume 550e8400-e29b-41d4-a716-446655440000
```

---

### `stratum status`

Show the status of a specific run, or list all runs.

```
stratum status [run_id]
```

**Arguments:**
- `[run_id]` — Optional run ID; shows all runs if omitted

**Example:**
```bash
# List all runs
stratum status

# Show specific run
stratum status 550e8400-e29b-41d4-a716-446655440000
```

---

### `stratum trajectory`

Show trajectory events for a run.

```
stratum trajectory <run_id>
```

**Arguments:**
- `<run_id>` — The run ID to query (required)

**Example:**
```bash
stratum trajectory 550e8400-e29b-41d4-a716-446655440000
```

Displays all events emitted during the run, including state transitions, tool calls, context compaction, memory operations, and HITL gates.

---

### `stratum export`

Export trajectory events for a run in a machine-readable format.

```
stratum export <run_id> [--format <format>]
```

**Arguments:**
- `<run_id>` — The run ID to export (required)

**Options:**
- `--format <format>` — Export format: `jsonl`, `csv`, or `replay` (default: `jsonl`)

**Example:**
```bash
# Export as JSONL (default)
stratum export 550e8400-e29b-41d4-a716-446655440000 > events.jsonl

# Export as CSV
stratum export 550e8400-e29b-41d4-a716-446655440000 --format csv > events.csv

# Export as replay
stratum export 550e8400-e29b-41d4-a716-446655440000 --format replay > replay.json
```

Output is written to stdout, so redirect to a file as needed. See the [Trajectory Export Guide](trajectory-export-guide.md) for format details.

---

### `stratum gates`

List all pending HITL (human-in-the-loop) gates across all runs.

```
stratum gates
```

Shows gates that are waiting for a human decision. Each gate includes the run ID, gate category, action attempted, and available alternatives.

**Example:**
```bash
stratum gates
```

---

### `stratum decide`

Record a decision on a pending HITL gate.

```
stratum decide <run_id> <decision>
```

**Arguments:**
- `<run_id>` — The run ID with the pending gate (required)
- `<decision>` — The decision to record (required)

**Decision formats:**
- `approve` — Approve the gated action
- `abort` — Abort the run
- `modify:<context>` — Approve with modified context
- `redirect:<goal>` — Redirect the run to a new goal

**Example:**
```bash
# Approve a pending gate
stratum decide 550e8400-e29b-41d4-a716-446655440000 approve

# Abort the run
stratum decide 550e8400-e29b-41d4-a716-446655440000 abort

# Approve with modifications
stratum decide 550e8400-e29b-41d4-a716-446655440000 "modify:use postgres instead of sqlite"

# Redirect to a new goal
stratum decide 550e8400-e29b-41d4-a716-446655440000 "redirect:focus on the API layer only"
```

---

### `stratum queue depth`

Show the current depth of the task dispatch queue.

```
stratum queue depth
```

**Example:**
```bash
stratum queue depth
```

---

### `stratum metrics`

Show current metrics in Prometheus exposition format.

```
stratum metrics [run_id]
```

**Arguments:**
- `[run_id]` — Optional run ID to show metrics for a specific run

**Example:**
```bash
# All metrics
stratum metrics

# Metrics for a specific run
stratum metrics 550e8400-e29b-41d4-a716-446655440000
```

**Metric families:**
- `run_state` — Current state of each run
- `context_tokens` — Token usage per context slot
- `kv_cache_hit_rate` — KV-cache hit rate
- `tool_calls` / `tool_errors` / `tool_retries` — Tool execution stats
- `hygiene_degradations` — Context hygiene score degradations
- `subagents` — Sub-agent spawn counts
- `llm_tokens` — LLM token usage (input/output/cached)
- `compaction_count` — Context compaction events

---

### `stratum dashboard`

Launch a live terminal dashboard showing metrics in real-time.

```
stratum dashboard
```

The dashboard uses a TUI (ratatui + crossterm) with:
- Runs table showing all active/completed runs
- Global metrics overview
- Raw metrics view

Press `q` to quit.

---

### `stratum serve`

Start a Prometheus-compatible HTTP metrics server.

```
stratum serve [--port <port>]
```

**Options:**
- `--port <port>` — Port to listen on (default: `9090`)

**Endpoints:**
- `GET /metrics` — Prometheus exposition format metrics
- `GET /health` — Health check

**Example:**
```bash
# Start on default port 9090
stratum serve

# Start on custom port
stratum serve --port 8080
```

Configure Prometheus to scrape `http://localhost:9090/metrics`.

---

### `stratum daemon`

Start the persistent daemon that watches for tasks via rfbmq and runs them autonomously.

```
stratum daemon [--foreground]
```

**Options:**
- `--foreground` — Run in foreground (default: `true`; the daemon does not background itself)

**Requires:** API key

**Behavior:**
- Watches the rfbmq `pending/` directory via filesystem events (`notify` crate — FSEvents on macOS, inotify on Linux)
- Dequeues tasks and spawns agent runs up to the concurrency limit (default: 4)
- Uses a daemon-specific system prompt with 10 built-in tools (bash, file ops, memory, tool/skill creation)
- Auto-approves global memory promotions (no human approval queue)
- Writes a PID file to `<data_dir>/daemon.pid`
- Handles `Ctrl+C` for graceful shutdown (waits up to 60s for active runs)
- Periodically checks for new tasks every 5 seconds as a fallback

**Example:**
```bash
# Start daemon in foreground
stratum daemon

# Stop with Ctrl+C
```

---

### `stratum submit`

Submit a task for the daemon to execute.

```
stratum submit <task> [--priority <priority>] [--tags <tags>]
```

**Arguments:**
- `<task>` — The task goal (required)

**Options:**
- `--priority <priority>` — Priority level: `critical`, `high`, `normal`, `low` (default: `normal`)
- `--tags <tags>` — Comma-separated tags for categorization

**Example:**
```bash
# Submit a basic task
stratum submit "Build a hello world web server"

# Submit with priority and tags
stratum submit "Fix the auth bug in login.rs" --priority high --tags bugfix,auth

# Submit a low-priority task
stratum submit "Add documentation to utils module" --priority low --tags docs
```

The task is enqueued into rfbmq and will be picked up by a running daemon.
