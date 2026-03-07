# Trajectory Events Reference

Every action Stratum takes is recorded as a `TrajectoryEvent` and persisted by `SqliteTrajectoryStore`. Events provide full observability into agent runs -- state transitions, LLM calls, tool executions, memory operations, and sub-agent activity.

## Event Types

All 23 `EventType` variants, grouped by category:

### Session Lifecycle

| Event | Description |
|-------|-------------|
| `RunCreated` | New run initialized |
| `RunStarted` | Run transitioned to running |
| `CheckpointWritten` | Checkpoint saved |
| `RunCompleted` | Run finished successfully |
| `RunFailed` | Run terminated with error |
| `RunAborted` | Run aborted |

### Core Loop

| Event | Description |
|-------|-------------|
| `LlmCompleted` | LLM response received (includes token usage) |

### Memory

| Event | Description |
|-------|-------------|
| `MemoryWritten` | Memory entry stored |
| `MemorySearched` | Memory search executed |

### Tools

| Event | Description |
|-------|-------------|
| `ToolCalled` | Tool invocation started |
| `ToolValidated` | Input passed schema validation |
| `ToolExecuted` | Tool execution completed |
| `ToolFailed` | Tool execution failed |
| `ToolRetried` | Tool retried after transient failure |

### Sub-Agents

| Event | Description |
|-------|-------------|
| `SubagentSpawned` | Child agent spawned |
| `SubagentCompleted` | Child agent finished |
| `SubagentFailed` | Child agent failed |
| `JanitorRunStarted` | Background janitor task started |

### Daemon

| Event | Description |
|-------|-------------|
| `DaemonStarted` | Daemon process started |
| `DaemonStopped` | Daemon process stopped |
| `TaskDequeued` | Task dequeued from rfbmq |
| `TaskCompleted` | Queued task completed |
| `ToolCreated` | Dynamic tool registered |
| `SkillCreated` | Skill definition saved |

## Event Schema

Each `TrajectoryEvent` has the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `event_id` | UUID v4 | Unique event identifier |
| `run_id` | UUID v4 | Run this event belongs to |
| `parent_run_id` | UUID v4 or null | Parent run (for sub-agents) |
| `timestamp` | RFC 3339 datetime | When the event occurred |
| `event_type` | string | One of the 23 `EventType` variants |
| `stratum_layer` | string | Which layer emitted the event |
| `payload` | JSON object | Event-specific data (varies by event type) |

### StratumLayer Values

| Value | Description |
|-------|-------------|
| `Session` | Session lifecycle events |
| `Context` | Context assembly |
| `Memory` | Memory operations |
| `Tools` | Tool gateway |
| `Orchestrator` | Sub-agent orchestration |
| `Trajectory` | Trajectory store itself |

### Example Event (JSON)

```json
{
  "event_id": "a1b2c3d4-...",
  "run_id": "550e8400-...",
  "parent_run_id": null,
  "timestamp": "2025-01-15T10:30:00Z",
  "event_type": "ToolExecuted",
  "stratum_layer": "Tools",
  "payload": {
    "tool_name": "bash",
    "status": "Success",
    "latency_ms": 120
  }
}
```

## Querying Events

### Programmatically (via `TrajectoryStore` trait)

```rust
// Query all events for a run
let events = store.query_events(Some(run_id), None, None).await?;

// Query specific event type
let tool_events = store.query_events(None, Some(EventType::ToolCalled), None).await?;

// Query with limit
let recent = store.query_events(Some(run_id), None, Some(10)).await?;
```

### Direct SQLite

Events are stored in the `trajectory_events` table within `stratum.db`:

```sql
SELECT event_id, run_id, timestamp, event_type, payload
FROM trajectory_events
WHERE run_id = '550e8400-...'
ORDER BY timestamp ASC;

-- Count events by type for a run
SELECT event_type, COUNT(*) as count
FROM trajectory_events
WHERE run_id = '550e8400-...'
GROUP BY event_type
ORDER BY count DESC;

-- Find all tool failures
SELECT timestamp, payload
FROM trajectory_events
WHERE event_type = 'ToolFailed'
ORDER BY timestamp DESC;
```

The table has indexes on `run_id`, `event_type`, and `timestamp` for efficient queries.
