# Configuration Guide

Stratum loads configuration with the following precedence (highest wins):

1. **Environment variables** -- override everything
2. **YAML config file** -- `stratum.yaml` or path specified via `--config`
3. **Built-in defaults** -- sensible out-of-the-box values

## Config File

By default, Stratum looks for `stratum.yaml` in the current working directory. Use `--config <path>` to specify a different location.

### Example `stratum.yaml`

```yaml
api_key: sk-ant-your-key-here
model: claude-sonnet-4-20250514
max_tokens: 4096
data_dir: .stratum
max_concurrent_runs: 4
```

### All Config Keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `api_key` | string | *(empty)* | Anthropic API key |
| `model` | string | `claude-sonnet-4-20250514` | Model identifier |
| `max_tokens` | integer | `4096` | Maximum tokens for LLM responses |
| `data_dir` | path | `.stratum` | Directory for SQLite DBs and data |
| `max_concurrent_runs` | integer | `4` | Maximum concurrent daemon runs |

## Environment Variables

Environment variables override config file values:

| Variable | Overrides | Example |
|----------|-----------|---------|
| `STRATUM_API_KEY` | `api_key` | `export STRATUM_API_KEY=sk-ant-...` |
| `STRATUM_MODEL` | `model` | `export STRATUM_MODEL=claude-sonnet-4-20250514` |
| `STRATUM_DATA_DIR` | `data_dir` | `export STRATUM_DATA_DIR=/var/lib/stratum` |

## Data Directory

The `data_dir` (default: `.stratum/`) stores all persistent state:

```
.stratum/
  stratum.db        # Main SQLite database (runs, checkpoints, trajectory events)
  memory.db         # Persistent memory tier (FTS5)
  tools.db          # Persistent tool registry (agent-created tools)
  queues/           # rfbmq task dispatch queues
  skills/           # Skill definitions (Markdown with YAML frontmatter)
  daemon.pid        # Daemon PID file (when running)
```

## Commands Requiring an API Key

Only `stratum start` requires a valid API key (via `STRATUM_API_KEY` or config). All other commands (`submit`, `stop`, `status`) are queue-only or read-only and do not require an API key.
