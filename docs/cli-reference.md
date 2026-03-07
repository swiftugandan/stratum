# CLI Reference

Stratum provides 4 commands for managing the daemon, submitting tasks, and checking status.

## Global Options

| Option | Description |
|--------|-------------|
| `-c, --config <path>` | Path to configuration file (default: `stratum.yaml` in current directory) |
| `-h, --help` | Print help |

## Commands

### `stratum start`

Start the daemon in the foreground. The daemon watches the rfbmq queue for tasks and runs them autonomously.

```
stratum start
```

**Requires:** API key (via `STRATUM_API_KEY` or `api_key` in `stratum.yaml`)

**Behavior:**
- Watches the rfbmq `pending/` directory via filesystem events (`notify` crate -- FSEvents on macOS, inotify on Linux)
- Dequeues tasks and spawns agent runs up to the concurrency limit (default: 4)
- Uses a daemon-specific system prompt with 9 built-in tools
- Writes a PID file to `<data_dir>/daemon.pid`
- Handles `Ctrl+C` for graceful shutdown (waits up to 60s for active runs)
- Periodically checks for new tasks every 5 seconds as a fallback

**Example:**
```bash
# Start daemon (foreground, Ctrl+C to stop)
stratum start
```

---

### `stratum submit`

Submit a task for the daemon to execute.

```
stratum submit <goal> [--priority <level>] [--tag <tag>]...
```

**Arguments:**
- `<goal>` -- The task goal (required)

**Options:**
- `--priority <level>` -- Priority level: `critical`, `high`, `normal`, `low` (default: `normal`)
- `--tag <tag>` -- Tag for categorization (repeatable)

**Example:**
```bash
# Submit a basic task
stratum submit "Build a hello world web server"

# Submit with priority and tags
stratum submit "Fix the auth bug in login.rs" --priority high --tag bugfix --tag auth

# Submit a low-priority task
stratum submit "Add documentation to utils module" --priority low --tag docs
```

The task is enqueued into rfbmq and will be picked up by a running daemon.

---

### `stratum stop`

Stop a running daemon by sending SIGTERM via the PID file.

```
stratum stop
```

Reads `<data_dir>/daemon.pid` and sends a termination signal to the daemon process.

**Example:**
```bash
stratum stop
```

---

### `stratum status`

Show the daemon PID and queue depth.

```
stratum status
```

Reads the PID file to check if the daemon is running and queries rfbmq for the current queue depth.

**Example:**
```bash
stratum status
```
