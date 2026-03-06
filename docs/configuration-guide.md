# Configuration Guide

Stratum loads configuration with the following precedence (highest wins):

1. **Environment variables** — override everything
2. **YAML config file** — `stratum.yaml` or path specified via `--config`
3. **Built-in defaults** — sensible out-of-the-box values

## Config File

By default, Stratum looks for `stratum.yaml` in the current working directory. Use `--config <path>` to specify a different location.

### Example `stratum.yaml`

```yaml
llm_provider: anthropic
api_key: sk-ant-your-key-here
model: claude-sonnet-4-20250514
max_tokens: 4096
trust_level: supervised
data_dir: .stratum
```

### All Config Keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `llm_provider` | string | `anthropic` | LLM provider to use |
| `api_key` | string | *(empty)* | API key for the LLM provider |
| `model` | string | `claude-sonnet-4-20250514` | Model identifier |
| `max_tokens` | integer | `4096` | Maximum tokens for LLM responses |
| `trust_level` | string | `supervised` | Trust level for tool execution |
| `data_dir` | path | `.stratum` | Directory for SQLite DBs and data |

## Environment Variables

Environment variables override config file values:

| Variable | Overrides | Example |
|----------|-----------|---------|
| `STRATUM_API_KEY` | `api_key` | `export STRATUM_API_KEY=sk-ant-...` |
| `STRATUM_MODEL` | `model` | `export STRATUM_MODEL=claude-sonnet-4-20250514` |
| `STRATUM_TRUST_LEVEL` | `trust_level` | `export STRATUM_TRUST_LEVEL=autonomous` |
| `STRATUM_DATA_DIR` | `data_dir` | `export STRATUM_DATA_DIR=/var/lib/stratum` |

## LLM Providers

| Provider | `llm_provider` value | API Key Format |
|----------|---------------------|----------------|
| Anthropic | `anthropic` | `sk-ant-...` |
| OpenAI (Chat Completions) | `openai` | `sk-...` |
| OpenAI (Responses API) | `openai_responses` | `sk-...` |

All providers support retry with exponential backoff and jitter on transient errors (429, 500, 503, 529).

## Trust Levels

Trust levels control which tools the agent can execute. Each tool declares a minimum trust level; the run's trust level must meet or exceed it.

| Level | `trust_level` value | Description |
|-------|--------------------|-------------|
| Sandboxed | `sandboxed` | Most restrictive. Only safe, read-only tools. |
| Supervised | `supervised` | Default. Tools that modify state require HITL approval. |
| Autonomous | `autonomous` | Least restrictive. All tools available without gates. |

Trust levels are ordered: `sandboxed < supervised < autonomous`. A tool requiring `supervised` trust will be denied in a `sandboxed` run but allowed in `supervised` or `autonomous` runs.

## Data Directory

The `data_dir` (default: `.stratum/`) stores all persistent state:

```
.stratum/
  stratum.db        # Main SQLite database (runs, checkpoints, trajectory, HITL gates)
  episodic.db       # Episodic memory tier (FTS5)
  global.db         # Global memory tier (FTS5 + promotion queue)
  offload/          # Context compaction offloaded data
  memory/           # Project-tier memory (Markdown + sidecar FTS5 index)
  skills/           # Skill definitions (Markdown with YAML frontmatter)
  queues/           # rfbmq task dispatch queues
  sandbox/          # Subprocess executor sandbox root
```

## Commands Requiring an API Key

The following commands require a valid API key (via `STRATUM_API_KEY` or config):

- `stratum run`
- `stratum resume`

All other commands (status, trajectory, export, gates, decide, queue, metrics, dashboard, serve) are read-only and do not require an API key.
