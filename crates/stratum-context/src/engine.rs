//! Default implementation of the `ContextEngine` port trait.
//!
//! Manages context assembly, budget checking, 3-stage compaction,
//! hygiene scoring, and todo recitation for a Stratum run.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use uuid::Uuid;

use stratum_core::{ContextEngine, LlmClient, SessionManager, TrajectoryStore};
use stratum_types::*;

use crate::compaction;
use crate::error::ContextError;
use crate::token::{count_tokens, count_tokens_many};

/// Configuration for the context engine.
#[derive(Debug, Clone)]
pub struct ContextEngineConfig {
    /// Directory to write offloaded tool results.
    pub offload_dir: PathBuf,
    /// Token threshold for offloading a single tool result (default: 3750 ≈ 15K chars).
    pub offload_token_threshold: u64,
    /// Inject todo recitation every N turns (default: 5).
    pub todo_recitation_interval: u32,
    /// Hygiene score threshold below which re-anchoring is triggered (default: 0.6).
    pub hygiene_threshold: f32,
    /// Number of consecutive degraded turns before re-anchor injection (default: 3).
    pub hygiene_consecutive_limit: u32,
    /// Model to use for Stage 3 summarisation.
    pub summarisation_model: String,
}

impl Default for ContextEngineConfig {
    fn default() -> Self {
        Self {
            offload_dir: PathBuf::from(".stratum/offload"),
            offload_token_threshold: compaction::OFFLOAD_TOKEN_THRESHOLD,
            todo_recitation_interval: 5,
            hygiene_threshold: 0.6,
            hygiene_consecutive_limit: 3,
            summarisation_model: "claude-sonnet-4-20250514".to_string(),
        }
    }
}

/// Per-run mutable state tracked by the context engine.
#[derive(Debug, Clone, Default)]
struct RunContextState {
    /// Cached parent_run_id to avoid querying session store on every event emit.
    parent_run_id: Option<RunId>,
    /// System anchor (immutable within a run, per KV-cache-first constraint).
    system_anchor: String,
    /// Current task manifest content.
    task_manifest: String,
    /// Injected knowledge blocks.
    injected_knowledge: Vec<String>,
    /// Tool result entries (may contain offloaded references).
    tool_results: Vec<String>,
    /// Whether each tool result is an error (errors are never pruned during compaction).
    tool_result_is_error: Vec<bool>,
    /// Conversation history.
    history: String,
    /// Current turn number.
    turn_count: u32,
    /// The task goal for hygiene scoring and summarisation.
    task_goal: String,
    /// Current PROGRESS.md content for todo recitation.
    progress_md: String,
    /// Running count of consecutive hygiene-degraded turns.
    consecutive_degraded: u32,
}

/// Default context engine implementation.
///
/// Requires injected dependencies for session, trajectory, and LLM access.
/// All interactions with external systems go through port traits.
pub struct DefaultContextEngine<S, T, L>
where
    S: SessionManager,
    T: TrajectoryStore,
    L: LlmClient,
{
    config: ContextEngineConfig,
    session: Arc<S>,
    trajectory: Arc<T>,
    llm: Arc<L>,
    /// Per-run state keyed by RunId.
    states: RwLock<std::collections::HashMap<RunId, RunContextState>>,
}

impl<S, T, L> DefaultContextEngine<S, T, L>
where
    S: SessionManager,
    T: TrajectoryStore,
    L: LlmClient,
{
    /// Create a new context engine with the given configuration and dependencies.
    pub fn new(
        config: ContextEngineConfig,
        session: Arc<S>,
        trajectory: Arc<T>,
        llm: Arc<L>,
    ) -> Self {
        Self {
            config,
            session,
            trajectory,
            llm,
            states: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Initialise context state for a run. Call once after run creation.
    pub async fn init_run(
        &self,
        run_id: RunId,
        system_anchor: String,
        task_goal: String,
        progress_md: String,
    ) {
        // Cache parent_run_id from session store so emit_event never needs to query it.
        let parent_run_id = self
            .session
            .get_run(run_id)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.parent_run_id);

        let mut states = self.states.write().await;
        states.insert(
            run_id,
            RunContextState {
                parent_run_id,
                system_anchor,
                task_goal,
                progress_md,
                ..Default::default()
            },
        );
    }

    /// Update the task manifest content for a run.
    pub async fn set_task_manifest(&self, run_id: RunId, manifest: String) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(&run_id) {
            state.task_manifest = manifest;
        }
    }

    /// Append a tool result entry to the run's context.
    ///
    /// If `is_error` is true, the entry is protected from compaction
    /// ("errors stay in context" — SAD §7.2).
    pub async fn push_tool_result(&self, run_id: RunId, result: String, is_error: bool) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(&run_id) {
            state.tool_results.push(result);
            state.tool_result_is_error.push(is_error);
        }
    }

    /// Append to conversation history.
    pub async fn append_history(&self, run_id: RunId, message: String) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(&run_id) {
            if !state.history.is_empty() {
                state.history.push('\n');
            }
            state.history.push_str(&message);
        }
    }

    /// Add an injected knowledge block.
    pub async fn push_knowledge(&self, run_id: RunId, knowledge: String) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(&run_id) {
            state.injected_knowledge.push(knowledge);
        }
    }

    /// Update PROGRESS.md content for todo recitation.
    pub async fn set_progress(&self, run_id: RunId, progress_md: String) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(&run_id) {
            state.progress_md = progress_md;
        }
    }

    /// Increment the turn counter and return the new count.
    pub async fn advance_turn(&self, run_id: RunId) -> u32 {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(&run_id) {
            state.turn_count += 1;
            state.turn_count
        } else {
            0
        }
    }

    async fn emit_event(
        &self,
        run_id: RunId,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> Result<(), ContextError> {
        let parent_run_id = {
            let states = self.states.read().await;
            states.get(&run_id).and_then(|s| s.parent_run_id)
        };

        let event = TrajectoryEvent {
            event_id: Uuid::new_v4(),
            run_id,
            parent_run_id,
            timestamp: Utc::now(),
            event_type,
            stratum_layer: StratumLayer::ContextEngine,
            payload,
            token_cost: TokenCost::default(),
        };
        self.trajectory
            .emit_event(event)
            .await
            .map_err(|e| ContextError::Trajectory(e.to_string()))
    }
}

impl<S, T, L> std::fmt::Debug for DefaultContextEngine<S, T, L>
where
    S: SessionManager,
    T: TrajectoryStore,
    L: LlmClient,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultContextEngine")
            .field("config", &self.config)
            .finish()
    }
}

#[async_trait]
impl<S, T, L> ContextEngine for DefaultContextEngine<S, T, L>
where
    S: SessionManager + 'static,
    T: TrajectoryStore + 'static,
    L: LlmClient + 'static,
{
    type Error = ContextError;

    async fn assemble_context(&self, run_id: RunId) -> Result<AssembledContext, Self::Error> {
        let states = self.states.read().await;
        let state = states
            .get(&run_id)
            .ok_or_else(|| ContextError::RunNotFound(run_id.to_string()))?;

        let slot_tokens = SlotTokenCounts {
            system_anchor: count_tokens(&state.system_anchor),
            task_manifest: count_tokens(&state.task_manifest),
            injected_knowledge: count_tokens_many(&state.injected_knowledge),
            tool_results: count_tokens_many(&state.tool_results),
            history: count_tokens(&state.history),
        };
        let total_tokens = slot_tokens.system_anchor
            + slot_tokens.task_manifest
            + slot_tokens.injected_knowledge
            + slot_tokens.tool_results
            + slot_tokens.history;

        Ok(AssembledContext {
            system_anchor: state.system_anchor.clone(),
            task_manifest: state.task_manifest.clone(),
            injected_knowledge: state.injected_knowledge.clone(),
            tool_results: state.tool_results.clone(),
            history: state.history.clone(),
            total_tokens,
            slot_tokens,
        })
    }

    fn check_budget(&self, context: &AssembledContext, budget: &ContextBudget) -> BudgetStatus {
        let s = &context.slot_tokens;

        // Check per-slot budgets — any single slot exceeding is OverBudget
        let slot_overage = [
            (s.system_anchor, budget.system_anchor as u64),
            (s.task_manifest, budget.task_manifest as u64),
            (s.injected_knowledge, budget.injected_knowledge as u64),
            (s.tool_results, budget.tool_results as u64),
            (s.history, budget.history as u64),
        ]
        .iter()
        .filter_map(|(actual, limit)| {
            if actual > limit {
                Some(actual - limit)
            } else {
                None
            }
        })
        .sum::<u64>();

        if slot_overage > 0 {
            return BudgetStatus::OverBudget {
                overage_tokens: slot_overage,
            };
        }

        let total = context.total_tokens;
        let ceiling = budget.total_ceiling as u64;
        let threshold = (ceiling as f32 * budget.compaction_threshold) as u64;

        if total > ceiling {
            BudgetStatus::OverBudget {
                overage_tokens: total - ceiling,
            }
        } else if total > threshold {
            BudgetStatus::ApproachingThreshold {
                utilisation: total as f32 / ceiling as f32,
            }
        } else {
            BudgetStatus::WithinBudget
        }
    }

    async fn trigger_compaction(
        &self,
        run_id: RunId,
        stage: CompactionStage,
    ) -> Result<(), Self::Error> {
        match stage {
            CompactionStage::Offload => {
                let (tool_results, protect) = {
                    let states = self.states.read().await;
                    let state = states
                        .get(&run_id)
                        .ok_or_else(|| ContextError::RunNotFound(run_id.to_string()))?;
                    (
                        state.tool_results.clone(),
                        state.tool_result_is_error.clone(),
                    )
                };

                let offload_dir = self.config.offload_dir.join(run_id.to_string());
                let result = compaction::offload(
                    &tool_results,
                    &offload_dir,
                    self.config.offload_token_threshold,
                    &protect,
                )?;

                {
                    let mut states = self.states.write().await;
                    if let Some(state) = states.get_mut(&run_id) {
                        state.tool_results = result.entries;
                    }
                }

                self.emit_event(
                    run_id,
                    EventType::CompactionStage1Triggered,
                    serde_json::json!({
                        "tokens_freed": result.tokens_freed,
                        "files_offloaded": result.offloaded_files.len(),
                    }),
                )
                .await?;

                tracing::info!(
                    run_id = %run_id,
                    tokens_freed = result.tokens_freed,
                    "compaction stage 1 (offload) complete"
                );
            }
            CompactionStage::Truncate => {
                let (tool_results, protect) = {
                    let states = self.states.read().await;
                    let state = states
                        .get(&run_id)
                        .ok_or_else(|| ContextError::RunNotFound(run_id.to_string()))?;
                    (
                        state.tool_results.clone(),
                        state.tool_result_is_error.clone(),
                    )
                };

                let result = compaction::truncate(&tool_results, &protect);

                {
                    let mut states = self.states.write().await;
                    if let Some(state) = states.get_mut(&run_id) {
                        state.tool_results = result.entries;
                    }
                }

                self.emit_event(
                    run_id,
                    EventType::CompactionStage2Triggered,
                    serde_json::json!({
                        "tokens_freed": result.tokens_freed,
                    }),
                )
                .await?;

                tracing::info!(
                    run_id = %run_id,
                    tokens_freed = result.tokens_freed,
                    "compaction stage 2 (truncate) complete"
                );
            }
            CompactionStage::Summarise => {
                let (history, task_goal) = {
                    let states = self.states.read().await;
                    let state = states
                        .get(&run_id)
                        .ok_or_else(|| ContextError::RunNotFound(run_id.to_string()))?;
                    (state.history.clone(), state.task_goal.clone())
                };

                // Archive full history to trajectory store before summarising
                self.emit_event(
                    run_id,
                    EventType::CompactionStage3Triggered,
                    serde_json::json!({
                        "history_tokens": count_tokens(&history),
                        "action": "archive_and_summarise",
                    }),
                )
                .await?;

                let prompt = compaction::build_summarisation_prompt(&history, &task_goal);
                let messages = vec![LlmMessage {
                    role: "user".to_string(),
                    content: prompt,
                }];

                let response = self
                    .llm
                    .complete(&messages, &self.config.summarisation_model, None)
                    .await
                    .map_err(|e| ContextError::Llm(e.to_string()))?;

                {
                    let mut states = self.states.write().await;
                    if let Some(state) = states.get_mut(&run_id) {
                        let old_tokens = count_tokens(&state.history);
                        state.history = response.content;
                        let new_tokens = count_tokens(&state.history);
                        tracing::info!(
                            run_id = %run_id,
                            old_tokens,
                            new_tokens,
                            "compaction stage 3 (summarise) complete"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    async fn compute_hygiene_score(
        &self,
        run_id: RunId,
        window_size: usize,
    ) -> Result<HygieneScore, Self::Error> {
        let (score, consecutive_degraded, needs_reanchor) = {
            let mut states = self.states.write().await;
            let state = states
                .get_mut(&run_id)
                .ok_or_else(|| ContextError::RunNotFound(run_id.to_string()))?;

            // Compute overlap between recent history and task goal using token-level Jaccard.
            let history_lines: Vec<&str> = state.history.lines().collect();
            let window_start = history_lines.len().saturating_sub(window_size);
            let recent_window = history_lines[window_start..].join("\n");

            let score = if state.task_goal.is_empty() || recent_window.is_empty() {
                1.0
            } else {
                token_jaccard(&state.task_goal, &recent_window)
            };

            // Track consecutive degradation in persistent state
            if score < self.config.hygiene_threshold {
                state.consecutive_degraded += 1;
            } else {
                state.consecutive_degraded = 0;
            }

            let consecutive_degraded = state.consecutive_degraded;

            // Inject re-anchor block if needed (while we already hold the write lock)
            let needs_reanchor = consecutive_degraded >= self.config.hygiene_consecutive_limit;
            if needs_reanchor {
                let reanchor_block = format!(
                    "\n---\n## RE-ANCHOR: You are drifting from the task.\n\n\
                     **Original goal:** {}\n\n\
                     Review TASK.md and PROGRESS.md. Focus on the next incomplete item.\n---\n",
                    state.task_goal
                );
                state.history.push_str(&reanchor_block);
                state.consecutive_degraded = 0;
            }

            (score, consecutive_degraded, needs_reanchor)
        };

        if needs_reanchor {
            self.emit_event(
                run_id,
                EventType::ReanchorInjected,
                serde_json::json!({
                    "score": score,
                    "consecutive_degraded_before_reset": consecutive_degraded,
                }),
            )
            .await?;

            self.emit_event(
                run_id,
                EventType::HygieneScoreDegraded,
                serde_json::json!({ "score": score }),
            )
            .await?;
        }

        Ok(HygieneScore {
            score,
            on_task_fraction: score,
            consecutive_degraded,
        })
    }

    async fn inject_todo_recitation(&self, run_id: RunId) -> Result<(), Self::Error> {
        let turn = {
            let mut states = self.states.write().await;
            let state = states
                .get_mut(&run_id)
                .ok_or_else(|| ContextError::RunNotFound(run_id.to_string()))?;

            let should_inject = state.turn_count > 0
                && state.turn_count % self.config.todo_recitation_interval == 0
                && !state.progress_md.is_empty();

            if should_inject {
                let recitation_block = format!(
                    "\n---\n## TODO Recitation (turn {})\n\n{}\n---\n",
                    state.turn_count, state.progress_md
                );
                state.history.push_str(&recitation_block);
                Some(state.turn_count)
            } else {
                None
            }
        };

        if let Some(turn) = turn {
            self.emit_event(
                run_id,
                EventType::TodoRecitationInjected,
                serde_json::json!({ "turn": turn }),
            )
            .await?;
        }

        Ok(())
    }
}

/// Compute token-level Jaccard similarity between two texts.
///
/// Tokenises both texts using tiktoken, computes set intersection / union.
fn token_jaccard(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;

    let bpe = crate::token::bpe();
    let tokens_a: HashSet<u32> = bpe.encode_with_special_tokens(a).into_iter().collect();
    let tokens_b: HashSet<u32> = bpe.encode_with_special_tokens(b).into_iter().collect();

    if tokens_a.is_empty() && tokens_b.is_empty() {
        return 1.0;
    }

    let intersection = tokens_a.intersection(&tokens_b).count() as f32;
    let union = tokens_a.union(&tokens_b).count() as f32;

    if union == 0.0 {
        1.0
    } else {
        intersection / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_jaccard_identical() {
        let score = token_jaccard("hello world", "hello world");
        assert!((score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_token_jaccard_disjoint() {
        let score = token_jaccard("apple banana cherry", "xyz 123 456");
        assert!(score < 0.3);
    }

    #[test]
    fn test_token_jaccard_partial() {
        let score = token_jaccard(
            "build a REST API with endpoints",
            "build a REST API with tests",
        );
        assert!(score > 0.3);
        assert!(score < 1.0);
    }

    #[test]
    fn test_token_jaccard_empty() {
        assert!((token_jaccard("", "") - 1.0).abs() < f32::EPSILON);
    }
}
