# Trajectory Export Guide

Every action Stratum takes is recorded as a trajectory event — state transitions, LLM calls, tool executions, memory operations, context compaction, HITL gates, and sub-agent activity. These events can be exported for fine-tuning pipelines, analysis, and replay.

## Event Types

Events are categorized by stratum layer:

### Session Lifecycle (Stratum 1)
- `RunCreated` — New run initialized
- `RunStarted` — Run transitioned to running
- `CheckpointWritten` — Checkpoint saved
- `RunResumed` — Paused/checkpointed run resumed
- `RunCompleted` — Run finished successfully
- `RunFailed` — Run terminated with error
- `RunAborted` — Run aborted (by user or HITL)

### Core Loop
- `LlmCompleted` — LLM response received (includes token usage)

### Context Engine (Stratum 2)
- `CompactionStage1Triggered` — Offload stage (filesystem)
- `CompactionStage2Triggered` — Truncate stage (strip previews)
- `CompactionStage3Triggered` — Summarise stage (LLM-based)
- `HygieneScoreDegraded` — Context hygiene fell below threshold
- `ReanchorInjected` — Re-anchor injected to correct drift
- `TodoRecitationInjected` — PROGRESS.md injected for task focus

### Memory Hierarchy (Stratum 3)
- `MemoryWritten` — Memory entry stored
- `MemoryPromoted` — Entry promoted to higher tier
- `MemorySearched` — Memory search executed
- `SkillLoaded` — Skill definition loaded from filesystem

### Tool Gateway (Stratum 4)
- `ToolCalled` — Tool invocation started
- `ToolValidated` — Input passed schema validation
- `ToolExecuted` — Tool execution completed
- `ToolFailed` — Tool execution failed
- `ToolRetried` — Tool retried after transient failure
- `ConstraintViolated` — Constraint check failed

### Sub-Agent Orchestrator (Stratum 5)
- `SubagentSpawned` — Child agent spawned
- `SubagentCompleted` — Child agent finished
- `SubagentFailed` — Child agent failed
- `JanitorRunStarted` — Background janitor task started

### HITL Controller (Stratum 6)
- `GateOpened` — HITL gate opened (run paused)
- `GateDecisionReceived` — Human decision recorded
- `RunPaused` — Run paused for human review
- `RunRedirected` — Run goal changed via HITL redirect

## Event Schema

Each trajectory event has the following structure:

```json
{
  "event_id": "uuid-v4",
  "run_id": "uuid-v4",
  "parent_run_id": "uuid-v4 | null",
  "timestamp": "2025-01-15T10:30:00Z",
  "event_type": "ToolExecuted",
  "stratum_layer": "ToolGateway",
  "payload": { ... },
  "token_cost": {
    "cached_tokens": 0,
    "uncached_tokens": 150,
    "estimated_cost_usd": 0.0003
  }
}
```

The `payload` field contains event-specific data as a JSON object. Its schema varies by event type.

## Export Formats

### JSONL (default)

One JSON object per line. Best for fine-tuning pipelines and streaming processors.

```bash
stratum export <run_id> --format jsonl > training.jsonl
```

Output:
```
{"event_id":"...","run_id":"...","timestamp":"...","event_type":"RunCreated","stratum_layer":"SessionLifecycle","payload":{...},"token_cost":{...}}
{"event_id":"...","run_id":"...","timestamp":"...","event_type":"LlmCompleted","stratum_layer":"SessionLifecycle","payload":{...},"token_cost":{...}}
```

### CSV

Tabular format with flattened fields. Best for spreadsheet analysis and SQL import.

```bash
stratum export <run_id> --format csv > events.csv
```

### Replay

Structured JSON array preserving event ordering. Best for replaying or visualizing a run.

```bash
stratum export <run_id> --format replay > replay.json
```

## Usage Examples

### Export for fine-tuning

```bash
# Export a completed run as JSONL
stratum export 550e8400-e29b-41d4-a716-446655440000 --format jsonl > run_data.jsonl

# Export multiple runs
for run_id in $(stratum status | grep Completed | awk '{print $1}'); do
  stratum export "$run_id" --format jsonl >> all_training.jsonl
done
```

### Inspect a run

```bash
# View events interactively
stratum trajectory 550e8400-e29b-41d4-a716-446655440000

# Filter specific event types with jq
stratum export 550e8400-e29b-41d4-a716-446655440000 \
  | jq 'select(.event_type == "ToolExecuted")'

# Count events by type
stratum export 550e8400-e29b-41d4-a716-446655440000 \
  | jq -r '.event_type' | sort | uniq -c | sort -rn
```

### Analyze token costs

```bash
# Sum total tokens across a run
stratum export 550e8400-e29b-41d4-a716-446655440000 \
  | jq '[.token_cost.uncached_tokens] | add'

# Find the most expensive events
stratum export 550e8400-e29b-41d4-a716-446655440000 \
  | jq -s 'sort_by(.token_cost.estimated_cost_usd) | reverse | .[0:5]'
```

## Fine-Tuning Pipeline Integration

Stratum's JSONL export is designed for direct use in fine-tuning pipelines:

1. **Run agent tasks** — `stratum run "your task"` to generate trajectory data
2. **Export trajectories** — `stratum export <run_id> --format jsonl` for each run
3. **Filter and transform** — Use `jq` to extract LLM turns, tool calls, or specific event types
4. **Feed into your pipeline** — The JSONL format is compatible with standard fine-tuning tooling

Key fields for fine-tuning:
- `LlmCompleted` events contain the model's reasoning and responses
- `ToolExecuted` events show tool usage patterns
- `token_cost` tracks resource consumption per event
- `parent_run_id` links sub-agent events to their parent run
