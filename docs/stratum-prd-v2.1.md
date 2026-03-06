Here it is — built directly from the research findings.

---

# PRD: Stratum — The Ultimate Agent Harness

**Version:** 1.0  
**Status:** Concept / Pre-inception  
**Author:** Derived from cross-harness synthesis  
**Last Updated:** March 2026

---

## 1. Executive Summary

Every major agent harness in production today solves the same problems in incompatible ways. Claude Code excels at context compaction but is Anthropic-locked. Manus cracked KV-cache economics but can't generalise beyond wrappers. OpenAI's Codex harness proved constraint-driven development works but requires greenfield conditions. Pi proved that minimalism outperforms complexity but punts security and sub-agents entirely. LangChain DeepAgents has the most elegant storage abstractions but is the least battle-tested. OpenClaw is the only persistent daemon — and the most dangerous thing running on most machines it's installed on. AutoGen modelled conversations correctly but got the everything-else wrong.

None of them got all of it right.

**Stratum** is a model-agnostic, production-grade agent harness that unifies the proven patterns from all seven. Its name reflects its core principle: distinct, independently swappable layers — context, memory, tools, execution, observation — each doing exactly one thing, each replaceable without touching the others.

The central bet: **the harness is now the product**. Models are commoditising. The orchestration layer — context engineering, constraint enforcement, memory management, trajectory capture — is where durable value accrues. Stratum is designed to be that layer.

---

## 2. Problem Statement

Agents fail in production not because models are weak, but because the runtime wrapping them is brittle. The field has identified the failure modes clearly: context rot after many turns, AI amnesia across sessions, uncontrolled task one-shotting, cascading tool errors, architectural drift in agent-generated artefacts, and security vulnerabilities in persistent systems. Every existing harness solves some of these. None solves all of them. The field is also discovering new constraints that nobody designed for: KV-cache economics that make token ordering an engineering discipline, context overfitting to current model limitations, the impossibility of evaluating harness quality on static benchmarks, and the entropy problem in long-running agent-generated codebases.

The gap is not a missing feature. It is missing *coherence* — a unified design that holds together across all failure modes simultaneously.

---

## 3. Goals

**G1 — Durability.** An agent run must survive days, crashes, restarts, and context window exhaustion without losing state or requiring human intervention to resume.

**G2 — Model-agnosticism.** The harness must work with any LLM. Swapping the model backend requires changing one configuration value, not rewriting the harness.

**G3 — Constraint-driven productivity.** Architectural constraints, linting rules, and structural tests should channel agent effort rather than restrict it. The harness enforces boundaries; the model operates freely within them.

**G4 — Economic efficiency.** KV-cache hit rate is a primary optimisation target. Token ordering, prompt stability, and context structure are engineering decisions, not afterthoughts.

**G5 — Deletability.** Every intelligent harness subsystem — planners, compactors, routers — must have a clean removal path. The harness must be able to get simpler as models get smarter.

**G6 — Observable and replayable.** Every harness decision produces a structured event. The full trajectory of any run must be reconstructable, queryable, and exportable as training data.

**G7 — Secure by default.** The harness must never be the attack vector. Persistent systems require minimal-authority principles enforced at the architecture level, not as configuration options.

---

## 4. Non-Goals

- Stratum is not a model. It does not reason, plan, or generate.
- Stratum is not a framework for building agents. It is the runtime that runs them.
- Stratum is not a SaaS platform. The reference implementation is a self-hosted library/daemon.
- Stratum does not define domain logic. It is domain-blind.
- Stratum does not train models. It produces trajectories for those who do.

---

## 5. Target Users

**Primary:** AI infrastructure engineers building production agent systems. Teams who have moved past prototyping and are debugging reliability at scale.

**Secondary:** Individual developers building persistent, autonomous agents for long-horizon tasks (multi-day coding projects, research workflows, life automation).

**Tertiary:** AI labs running evaluation harnesses and collecting training trajectories from real agent behaviour.

---

## 6. Architecture: The Seven Strata

Stratum is composed of seven independent layers. Each layer has a clean interface. Each layer is independently replaceable. Each layer produces events to the Trajectory Store. The model sits in the middle and sees only what the layers choose to show it.

```
┌─────────────────────────────────────────────┐
│            STRATUM HARNESS BOUNDARY          │
│                                             │
│  [1] Session Lifecycle Manager              │
│  [2] Context Engine       ←── core loop     │
│  [3] Memory Hierarchy                       │
│  [4] Tool Execution Gateway                 │
│  [5] Sub-Agent Orchestrator                 │
│  [6] Human-in-the-Loop Controller           │
│  [7] Trajectory Store & Observability       │
│                                             │
│              ↕ MODEL ↕                      │
└─────────────────────────────────────────────┘
```

---

### Stratum 1 — Session Lifecycle Manager

The Session Lifecycle Manager owns the complete existence of an agent run: boot, execute, checkpoint, resume, hibernate, terminate. Nothing else manages run state.

**Initialisation — the Two-Prompt Pattern.** Derived directly from Anthropic's research, every run uses two distinct system prompts. The *Initialiser Prompt* runs exactly once and instructs the agent to produce four required artefacts: a `TASK.md` (goal + acceptance criteria in structured form), a `PROGRESS.md` (all deliverables, initially `[ ]`), a `DECISIONS.md` (architectural decisions log), and an `INIT.sh` (environment setup script). These artefacts are the run's memory substrate — they survive every context window reset. JSON structures within these files are preferred over prose where structure matters, because models are less likely to modify JSON incidentally. The *Worker Prompt* runs for all subsequent context windows. It reads the artefact state, identifies the highest-priority incomplete item, and proceeds incrementally.

**Checkpointing.** State is snapshotted to durable storage at three triggers: after every significant tool call (any write, execute, or network operation), at configurable time intervals, and whenever a HITL gate opens. Snapshots capture: full artefact state, tool call log, context compaction summary, and active sub-agent tree. A run can be restored from any checkpoint within its retention window.

**Run Identity.** Every run is assigned a `RunID` (UUID), a `ParentTaskID` (for sub-runs), a `ModelRef` (swappable), a `TrustLevel` (sandboxed / supervised / autonomous), and a `ToolManifest` (explicit whitelist — tools are opt-in, not opt-out). This identity block is immutable for the run's lifetime.

**Run States.** `INITIALISING → RUNNING → PAUSED (HITL) → CHECKPOINTED → RESUMING → COMPLETED | FAILED | ABORTED`. State transitions are events in the Trajectory Store.

---

### Stratum 2 — Context Engine

The Context Engine is the most critical stratum. It owns the context window completely — the model never sees raw message history, only what the Context Engine curates and assembles.

**The KV-Cache-First Constraint.** Every structural decision in the Context Engine is evaluated against KV-cache impact. The system prompt prefix is **immutable within a run** — no dynamic injection into the prefix. Tool definitions are **static** — all tools are declared upfront and never removed mid-run. The context is **append-only** — no rewriting, reordering, or deletion of prior messages (compression replaces content with references, but never removes structure). Cached tokens cost 10x less; architecture must reflect this.

**The Budget Model.** The context window is divided into five named slots with hard token ceilings:

| Slot | Contents | Priority |
|---|---|---|
| System Anchor | System prompt, tool definitions, run identity | Immutable |
| Task Manifest | TASK.md, PROGRESS.md digest | Always present |
| Injected Knowledge | RAG results, retrieved memory, skill content | On-demand |
| Tool Results | Active tool outputs (hot tail) | Rolling |
| History | Summarised prior turns | Compressible |

When total token count approaches the budget ceiling, compaction triggers automatically.

**Three-Stage Compaction Pipeline.** Based on the convergent pattern across Claude Code, DeepAgents, and Manus:

*Stage 1 — Offload.* Tool results exceeding a configurable threshold (default: 15,000 tokens) are written to the filesystem. The message slot is replaced with a file reference plus a 10-line preview. The full content is recoverable. This is **restorable compression** — nothing is lost, only relocated.

*Stage 2 — Truncate.* Tool call inputs and outputs that reference content already persisted (file writes, executed scripts) are truncated to their reference only. The content is already durable.

*Stage 3 — Summarise.* At ~85% budget utilisation, a structured LLM summarisation runs. The summary preserves: original goal, completed steps with outcomes, key decisions made, files modified, errors encountered and resolved, active blockers, and next planned action. The full message history is archived to the Trajectory Store. The summary replaces history in-context.

**Errors stay in context.** Failed tool calls are never pruned. The model's implicit belief updating from seeing failures is a feature, not noise.

**Attention Maintenance — the Todo Recitation Pattern.** Every N turns (configurable, default: every 5), the current `PROGRESS.md` state is appended to the context as a compact reminder block. This exploits the recency bias of transformer attention to keep long-horizon goals salient. It is pure context engineering with zero architectural overhead.

**Context Hygiene Score.** A lightweight metric computed each turn: given the last K model outputs, what fraction remained on-task relative to the original goal? Computed via a fast embedding similarity check against the task manifest (not an LLM call). When the score drops below threshold for N consecutive turns, a re-anchor block is injected: a compact restatement of the original goal, completed progress, and the next required action.

---

### Stratum 3 — Memory Hierarchy

Four tiers, independently swappable backends, explicit promotion semantics.

| Tier | Scope | Default Backend | TTL | Agent Write Access |
|---|---|---|---|---|
| Working | Current turn | In-process | Ephemeral | Implicit |
| Episodic | Current run | SQLite / KV | Run lifetime | Via tool |
| Project | Across runs, same project | Filesystem (Markdown + vector index) | Project lifetime | Via tool |
| Global | Across all projects | Vector DB (pgvector / SQLite-vec) | Indefinite | Harness-managed only |

**Project tier is the primary memory medium.** It consists of versioned Markdown files — the same four artefacts created by the Initialiser — plus a `MEMORY.md` for curated persistent facts (coding conventions, environment details, architectural decisions). The agent can read and write Project tier memory freely. This tier is human-readable, VCS-compatible, diff-friendly, and model-legible.

**Global tier promotion requires explicit review.** The agent can *request* Global tier promotion (via a `memory_promote` tool call), but the harness queues this for human approval or a configured policy engine. Global memory is the harness operator's knowledge base — it must not be polluted by task-specific noise.

**Semantic search** across tiers uses hybrid vector + BM25 ranking with optional temporal decay weighting. Search is always scoped: by tier, by project, by recency window. The agent can invoke `memory_search` as a tool call.

**Skills are a Project tier specialisation.** A skill is a Markdown file in `.stratum/skills/` with YAML frontmatter describing its trigger conditions and capabilities. Skills are loaded with progressive disclosure: at boot, only names and descriptions are injected into context. Full skill content is loaded only when the current task matches the skill's trigger conditions. This keeps base prompt token count minimal.

---

### Stratum 4 — Tool Execution Gateway

Every tool call flows through the Gateway. The model never directly executes anything.

**The Tool Manifest.** Each run is initialised with an explicit whitelist of permitted tools. Tools are opt-in, not opt-out. The default manifest is deliberately minimal — a Vercel-style principle: start with the fewest tools that can complete the task class. The manifest is immutable for the run's lifetime.

**Trust Levels.** Three trust levels with escalating tool permissions:

- *Sandboxed:* read-only filesystem access, no network, no subprocess spawning
- *Supervised:* read-write filesystem in scoped directory, network to allowlisted domains, subprocess with output capture
- *Autonomous:* full filesystem access, unrestricted network, subprocess spawning, external API calls

Trust level escalation always requires HITL approval.

**The Intercept-Validate-Execute-Log Pipeline.** Every tool call passes through four gates in sequence. *Intercept:* capture the full tool invocation before execution. *Validate:* check schema conformance, parameter types, and policy rules — if validation fails, return a structured error with remediation instructions (the OpenAI Codex insight: error messages are teaching). *Execute:* run the tool in an isolated subprocess or container appropriate to the trust level. *Log:* write a complete record — tool name, parameters, result, latency, cost estimate, and success/failure — to the Trajectory Store.

**Error handling.** Tool errors are never swallowed. On failure: return the structured error to the model with the same remediation-rich formatting as validation failures. Configurable retry with exponential backoff for transient failures (network timeouts, rate limits). After N retries, escalate to HITL. The model sees all retry attempts in context — errors stay in context (Manus principle).

**Constraint enforcement tools.** A dedicated tool class runs linters, structural tests, and architectural validation. Error output from these tools is pre-formatted to include remediation instructions, not just error codes. The tool output teaches the agent while it works.

**KV-cache protection.** Tool definitions are declared at run initialisation and never change. Logit masking or a state-machine policy controls which tools the model is encouraged to call at each step — but the schema definitions remain static in context so the cache prefix is never invalidated (Manus's converged solution over dynamic tool loading).

---

### Stratum 5 — Sub-Agent Orchestrator

The Orchestrator manages the spawning, execution, and result collection of sub-agent runs.

**Sub-agents are Stratum runs.** A sub-agent is a full Stratum run with a narrower scope, a more restricted tool manifest, and a reference to its parent `RunID`. Sub-agents have access to a shared filesystem scope (configurable) but isolated context windows. They cannot access the parent's context — only communicate through structured result return and shared filesystem.

**Spawn depth is bounded.** Default maximum spawn depth: 2 (parent → child → no further). Configurable up to 5. Beyond depth 2, the complexity of result aggregation typically exceeds any benefit from further decomposition. The harness hard-blocks spawning beyond the configured maximum.

**Four spawn patterns:**

*Delegate* — Parent spawns one sub-agent for a bounded, well-defined sub-task. Sub-agent runs to completion, returns a compressed result summary. Used for: isolated research, single-function implementation, document processing.

*Pipeline* — A fixed sequence of specialised sub-agents, each consuming the prior's output artefact. Used for: Researcher → Synthesiser → Validator, or Planner → Implementer → Reviewer.

*Parallel* — N sub-agents work concurrently on independent workstreams. A supervisor sub-agent (or the parent itself) merges results. Used for: concurrent investigation of multiple hypotheses, parallel file processing.

*Janitor* — Background sub-agents run on a scheduled cadence to enforce quality. They scan artefacts against defined invariants (the OpenAI Codex garbage collection insight), identify degradation, and open targeted fix tasks. A Janitor run has no user goal — it has an invariant specification and a mandate to restore it.

**Context isolation is the primary value.** Sub-agent context never pollutes parent context. The parent sees only the compressed result. This is the mechanism by which Stratum enables long-horizon tasks without context rot accumulation.

---

### Stratum 6 — Human-in-the-Loop Controller

The HITL Controller manages all pauses. It is not a safety afterthought — it is a first-class architectural primitive.

**Gate categories with default policies:**

| Gate Category | Trigger | Default Policy |
|---|---|---|
| Destructive | Delete, drop, purge operations | Always ask |
| Irreversible | Deploy, publish, send external communication | Always ask |
| Trust escalation | Request for elevated tool access | Always ask |
| Ambiguity | Goal unclear after N failed attempts | Always ask |
| Drift | Context Hygiene Score below threshold for N turns | Notify + option to redirect |
| Budget | Token or cost threshold exceeded | Notify |
| Scheduled | Time-based check-ins for long runs | Configurable |

**Pause records are structured.** When a gate triggers, the harness produces a `HITLRecord`: current run state, the action attempted, alternatives considered by the model, relevant context summary, and the decision options available. This record is the input to the human decision.

**Human decisions are typed.** Options are: `APPROVE` (proceed as requested), `MODIFY` (approve with injected context or parameter change), `REDIRECT` (inject new goal or constraint), `ABORT` (terminate run cleanly). All decisions are events in the Trajectory Store.

**Pauses are durable.** If no human responds, the run persists indefinitely in `PAUSED` state. The checkpoint is complete. The run can be resumed days later with a decision. The agent picks up exactly where it left off.

**Async notification.** The HITL Controller supports pluggable notifiers — webhook, email, Slack, or custom adapter — to alert humans to pending decisions on long-running autonomous tasks.

---

### Stratum 7 — Trajectory Store & Observability

Every harness decision produces a structured event. The Trajectory Store is an append-only log of these events. It is the harness's second output — equally important as task completion.

**Event schema.** Every event has: `event_id`, `run_id`, `parent_run_id` (for sub-agents), `timestamp`, `event_type`, `stratum_layer`, `payload`, and `metadata` (model used, token counts, latency, cost estimate).

**Event types:**

- Session: `run_created`, `run_started`, `checkpoint_written`, `run_resumed`, `run_completed`, `run_failed`, `run_aborted`
- Context: `compaction_stage1_triggered`, `compaction_stage2_triggered`, `compaction_stage3_triggered`, `hygiene_score_degraded`, `reanchor_injected`, `todo_recitation_injected`
- Memory: `memory_written` (with tier), `memory_promoted`, `memory_searched`, `skill_loaded`
- Tools: `tool_called`, `tool_validated`, `tool_executed`, `tool_failed`, `tool_retried`, `constraint_violated` (with remediation)
- Sub-agents: `subagent_spawned`, `subagent_completed`, `subagent_failed`, `janitor_run_started`
- HITL: `gate_opened`, `gate_decision_received`, `run_paused`, `run_redirected`

**Why this is the product's second output.** The trajectory of every run — including failures, drift events, constraint violations, and recovery patterns — is training signal for the next model generation. As Manus and Anthropic's research both identify: the competitive advantage is no longer the prompt, it is the trajectories the harness captures. The Trajectory Store is the mechanism by which Stratum operator data becomes a structural advantage.

**Observability surface.** A live dashboard exposes: current run state, context budget utilisation by slot, KV-cache hit rate estimate, tool call rate and error rate, Context Hygiene Score trend, HITL queue depth, sub-agent tree, and cost-per-run tracking. All metrics emit as Prometheus-compatible gauges and counters.

**Queryable and exportable.** Trajectories are queryable by run, by event type, by time range, and by outcome. Export formats: JSONL (for fine-tuning pipelines), structured CSV (for analysis), and a replay format (for re-running a trajectory deterministically with a different model).

---

## 7. Core Design Principles

**P1 — The harness does not reason.** If any harness component requires intelligence to function, it is over-engineered. Intelligence belongs to the model. The harness provides structure, enforcement, and memory — not cognition.

**P2 — Build to delete.** Every non-trivial harness feature ships with a removal path. When the model makes a feature obsolete (context rot prevention, error in context, todo recitation), the feature must exit cleanly without architectural surgery. The test of good harness design: can you remove this component in a single PR?

**P3 — Constrain to accelerate.** Architectural constraints, structural tests, and lint rules are productivity multipliers, not restrictions. The OpenAI Codex team's lesson applies universally: strict boundaries channel agent effort into productive work. Default manifests are minimal; capabilities are earned through explicit configuration.

**P4 — KV-cache economics are architecture.** The 10x cost differential between cached and uncached tokens means token ordering is an engineering discipline. Static prompts, append-only context, and deterministic serialisation are not micro-optimisations — they are first-order architectural decisions.

**P5 — Files are the durable API.** The primary medium for cross-session state, cross-agent communication, and human inspection is versioned Markdown files on a filesystem. This makes state human-readable, VCS-compatible, diff-friendly, and model-legible. Any agent, on any model, can read it.

**P6 — Errors teach.** Tool errors are never cleaned up or suppressed. Remediation instructions are injected into error messages so the model learns from violations while they happen. Failures stay in context because the model's implicit belief updating is more valuable than a cleaner context window.

**P7 — Trajectories compound.** Every run makes the next run better — either by training better models, or by populating the memory hierarchy with hard-won knowledge. Stratum is designed with this feedback loop as a first-class architectural requirement, not a nice-to-have.

---

## 8. Key Interface Contracts

```typescript
// Run initialisation
interface StratumRun {
  id: UUID;
  modelRef: ModelRef;           // swappable: one field
  trustLevel: TrustLevel;       // sandboxed | supervised | autonomous
  toolManifest: ToolName[];     // whitelist, minimal by default
  memoryConfig: MemoryConfig;
  hitlPolicy: HITLPolicy;
  contextBudget: ContextBudget;
  spawnDepthLimit: number;      // default: 2
}

// Task artefact — the run's durable contract
interface TaskManifest {
  goal: string;
  acceptanceCriteria: Criterion[];
  progress: ProgressItem[];     // { id, description, status, completedAt? }
  decisions: Decision[];        // { timestamp, context, choice, rationale }
  blockers: Blocker[];
}

// Context budget — hard ceilings per slot
interface ContextBudget {
  systemAnchor: number;
  taskManifest: number;
  injectedKnowledge: number;
  toolResults: number;
  history: number;
  totalCeiling: number;
  compactionThreshold: number;  // default: 0.85
}

// Tool execution result
interface ToolResult {
  toolName: string;
  status: 'success' | 'validation_failure' | 'execution_error' | 'policy_denied';
  output: unknown;
  remediationHint?: string;     // always present on failure
  latencyMs: number;
  cachedTokensUsed: number;
  uncachedTokensUsed: number;
}

// Trajectory event — every harness decision
interface TrajectoryEvent {
  eventId: UUID;
  runId: UUID;
  parentRunId?: UUID;
  timestamp: ISO8601;
  eventType: EventType;
  stratumLayer: 1 | 2 | 3 | 4 | 5 | 6 | 7;
  payload: Record<string, unknown>;
  tokenCost: TokenCost;
}
```

---

## 9. What Stratum Deliberately Excludes

**No built-in LLM calls in the harness itself.** Compaction summarisation, context hygiene scoring, and constraint checking all use the configured model via the same API as the agent. The harness never calls a separate "judge" model with a hardcoded endpoint.

**No graph compiler or agent framework.** Stratum is not LangGraph. It does not require users to define graphs, nodes, or edges. The loop is implicit. The graph model adds abstraction overhead without adding reliability.

**No cloud-only features.** Every capability is self-hostable. The Trajectory Store is a local SQLite database by default. The filesystem backend is a local directory. Cloud adaptors are plugins.

**No opinion about domain logic.** Stratum is domain-blind. What the agent does inside a run is entirely defined by the task manifest and tool manifest. The harness enforces structural invariants, not domain correctness.

---

## 10. Success Metrics

| Metric | Target | Measurement |
|---|---|---|
| Task completion on multi-day runs | >80% without human intervention | Trajectory Store run outcomes |
| Context rot rate | <5 drift events per 100 turns | Context Hygiene Score events |
| Resume fidelity post-crash | 100% state recovery | Checkpoint restore tests |
| Tool error escalation rate | <2% of calls reach HITL | Tool execution event ratio |
| KV-cache hit rate | >70% of tokens served from cache | Model API cache metadata |
| Time to swap model backend | <1 hour engineering | Config change + integration test |
| Trajectory export to fine-tune pipeline | Supported, documented, runnable | Export format specification |
| Sub-agent result compression ratio | >10:1 context reduction vs. full delegation | Token count comparison |

---

## 11. Open Questions Inherited from the Field

**Memory decay.** When should the agent forget? No existing harness has a principled approach to memory expiration. Stratum's Global tier needs a decay model before production use at scale.

**Optimal spawn depth.** Claude Code enforces depth 1. OpenClaw allows arbitrary depth. No empirical evidence exists for what depth maximises task quality across domains. Stratum defaults to 2 and treats this as an active research question.

**Harness evaluation methodology.** Static benchmarks measure model quality, not harness quality. Manus tops GAIA but achieves ~2.5% on real paid-work tasks. Stratum needs a harness-specific evaluation framework before any benchmark claims can be made.

**Constraint retrofi.** The Codex harness approach works greenfield. How to enforce structural tests, layered dependencies, and architectural linting on an existing codebase is unsolved. Stratum ships for greenfield first.

**The escalation boundary.** When should an autonomous agent stop and ask? Every harness tunes this heuristically. A principled model for the `keep-going / escalate` decision boundary — one that generalises across task types and risk levels — remains an open research problem. Stratum's default HITL policy is conservative until this is answered.

---

## 12. Versioning Philosophy

Every major model generation should prompt a harness audit. Features that existed to compensate for model limitations — context rot mitigation, error in context, todo recitation — must be re-evaluated each time a new frontier model ships. The Trajectory Store makes this empirical: run a controlled experiment, compare hygiene scores, measure task completion, remove the feature if the delta is gone.

Stratum measures its own maturity not by feature count, but by how many features it has successfully deleted.