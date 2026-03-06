use std::sync::Arc;

use async_trait::async_trait;
use stratum_adapters::{SqliteSessionManager, SqliteTrajectoryStore};
use stratum_context::DefaultContextEngine;
use stratum_core::turn::TurnOutcome;
use stratum_core::{
    ContextEngine, FrozenToolRegistry, LlmClient, SessionManager, ToolGateway, TrajectoryStore,
};
use stratum_tools::{DefaultToolGateway, InMemoryFrozenToolRegistry, SubprocessExecutor};
use stratum_types::*;

use crate::llm_adapter::LlmClientAdapter;
use crate::wiring::AppContext;

/// Error type for the turn executor.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct TurnError(#[from] pub anyhow::Error);

/// Concrete `TurnExecutor` that orchestrates a single agent turn.
pub struct DefaultTurnExecutor {
    pub session: Arc<SqliteSessionManager>,
    pub trajectory: Arc<SqliteTrajectoryStore>,
    pub context_engine:
        Arc<DefaultContextEngine<SqliteSessionManager, SqliteTrajectoryStore, LlmClientAdapter>>,
    pub llm: Arc<LlmClientAdapter>,
    pub tool_gateway: Arc<DefaultToolGateway<SqliteTrajectoryStore, SubprocessExecutor>>,
    pub tool_registry: Arc<InMemoryFrozenToolRegistry>,
    pub model: String,
}

impl DefaultTurnExecutor {
    /// Construct from an `AppContext`, extracting the needed dependencies.
    pub fn from_context(ctx: &AppContext) -> Self {
        Self {
            session: ctx.session.clone(),
            trajectory: ctx.trajectory.clone(),
            context_engine: ctx.context_engine.clone(),
            llm: ctx.llm.clone(),
            tool_gateway: ctx.tool_gateway.clone(),
            tool_registry: ctx.tool_registry.clone(),
            model: ctx.config.model.clone(),
        }
    }
}

#[async_trait]
impl stratum_core::TurnExecutor for DefaultTurnExecutor {
    type Error = TurnError;

    async fn execute_turn(&self, run_id: RunId) -> Result<TurnOutcome, Self::Error> {
        self.execute_turn_inner(run_id)
            .await
            .map_err(TurnError::from)
    }
}

/// Map any displayable error to `anyhow::Error`.
fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

impl DefaultTurnExecutor {
    async fn execute_turn_inner(&self, run_id: RunId) -> anyhow::Result<TurnOutcome> {
        // 1. Get run state
        let run = self
            .session
            .get_run(run_id)
            .await
            .map_err(err)?
            .ok_or_else(|| anyhow::anyhow!("run {run_id} not found"))?;

        // 2. Assemble context
        let context = self
            .context_engine
            .assemble_context(run_id)
            .await
            .map_err(err)?;

        // 3. Check budget, compact if needed
        let budget_status = self
            .context_engine
            .check_budget(&context, &run.context_budget);
        let context = match budget_status {
            BudgetStatus::OverBudget { .. } => {
                tracing::warn!(run_id = %run_id, "context over budget, triggering compaction");
                self.context_engine
                    .trigger_compaction(run_id, CompactionStage::Offload)
                    .await
                    .map_err(err)?;
                self.context_engine
                    .trigger_compaction(run_id, CompactionStage::Truncate)
                    .await
                    .map_err(err)?;
                self.context_engine
                    .assemble_context(run_id)
                    .await
                    .map_err(err)?
            }
            BudgetStatus::ApproachingThreshold { utilisation } => {
                tracing::debug!(run_id = %run_id, utilisation, "approaching budget threshold");
                context
            }
            BudgetStatus::WithinBudget => context,
        };

        // 4. Build LLM messages from assembled context
        let messages = build_messages(&context);

        // 5. Get tool definitions from registry for the LLM
        let tool_defs: Vec<ToolDefinition> = run
            .tool_manifest
            .iter()
            .filter_map(|name| self.tool_registry.get_tool(name).cloned())
            .collect();
        let tools_ref = if tool_defs.is_empty() {
            None
        } else {
            Some(tool_defs.as_slice())
        };

        // 6. Call LLM
        tracing::info!(run_id = %run_id, model = %self.model, "calling LLM");
        let response = self
            .llm
            .complete(&messages, &self.model, tools_ref)
            .await
            .map_err(err)?;

        // 7. Emit trajectory event
        self.trajectory
            .emit_event(TrajectoryEvent::new(
                run_id,
                run.parent_run_id,
                EventType::LlmCompleted,
                StratumLayer::ContextEngine,
                serde_json::json!({
                    "model": self.model,
                    "input_tokens": response.usage.input_tokens,
                    "output_tokens": response.usage.output_tokens,
                    "cached_tokens": response.usage.cached_tokens,
                }),
            ))
            .await
            .map_err(err)?;

        // 8. Route response
        if !response.tool_calls.is_empty() {
            let mut results = Vec::new();
            for tc in &response.tool_calls {
                tracing::info!(run_id = %run_id, tool = %tc.tool_name, "executing tool call");
                let invocation = ToolInvocation {
                    tool_name: tc.tool_name.clone(),
                    parameters: tc.arguments.clone(),
                    run_id,
                };
                let result = self.tool_gateway.call_tool(invocation).await.map_err(err)?;
                results.push(result);
            }

            // Checkpoint after tool calls
            self.session
                .checkpoint(&Checkpoint {
                    id: uuid::Uuid::new_v4(),
                    run_id,
                    state: run.state,
                    task_manifest: TaskManifest {
                        goal: context.task_manifest.clone(),
                        acceptance_criteria: vec![],
                        progress: vec![],
                        decisions: vec![],
                        blockers: vec![],
                    },
                    context_summary: format!("Turn with {} tool calls", response.tool_calls.len()),
                    tool_call_log: results.clone(),
                    sub_agent_tree: vec![],
                    created_at: chrono::Utc::now(),
                })
                .await
                .map_err(err)?;

            Ok(TurnOutcome::ToolCalls { results })
        } else {
            let stop = response.stop_reason.as_deref();
            let content = &response.content;

            let is_complete = stop == Some("end_turn") || stop == Some("stop");
            let content_signals_done = content.contains("all items complete")
                || content.contains("task is complete")
                || content.contains("TASK COMPLETE");

            if is_complete && content_signals_done {
                Ok(TurnOutcome::Completed)
            } else {
                Ok(TurnOutcome::Response {
                    content: content.clone(),
                })
            }
        }
    }
}

/// Convert `AssembledContext` into `Vec<LlmMessage>` for the LLM.
fn build_messages(context: &AssembledContext) -> Vec<LlmMessage> {
    let mut messages = Vec::new();

    // System message from anchor + task manifest
    let mut system = context.system_anchor.clone();
    if !context.task_manifest.is_empty() {
        system.push_str("\n\n## Task\n");
        system.push_str(&context.task_manifest);
    }
    if !context.injected_knowledge.is_empty() {
        system.push_str("\n\n## Knowledge\n");
        for k in &context.injected_knowledge {
            system.push_str(k);
            system.push('\n');
        }
    }
    messages.push(LlmMessage {
        role: "system".to_string(),
        content: system,
    });

    // Tool results as user messages
    for result in &context.tool_results {
        messages.push(LlmMessage {
            role: "user".to_string(),
            content: result.clone(),
        });
    }

    // History
    if !context.history.is_empty() {
        messages.push(LlmMessage {
            role: "user".to_string(),
            content: context.history.clone(),
        });
    }

    // If no user message yet, add a minimal one
    if messages.len() == 1 {
        messages.push(LlmMessage {
            role: "user".to_string(),
            content: "Continue with the next step.".to_string(),
        });
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_messages_includes_system_and_user() {
        let context = AssembledContext {
            system_anchor: "You are a helpful assistant.".to_string(),
            task_manifest: "Build a thing.".to_string(),
            injected_knowledge: vec![],
            tool_results: vec![],
            history: String::new(),
            total_tokens: 100,
            slot_tokens: SlotTokenCounts::default(),
        };

        let messages = build_messages(&context);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("Build a thing"));
        assert_eq!(messages[1].role, "user");
    }

    #[test]
    fn build_messages_includes_tool_results() {
        let context = AssembledContext {
            system_anchor: "System.".to_string(),
            task_manifest: String::new(),
            injected_knowledge: vec![],
            tool_results: vec!["result1".to_string(), "result2".to_string()],
            history: String::new(),
            total_tokens: 100,
            slot_tokens: SlotTokenCounts::default(),
        };

        let messages = build_messages(&context);
        // system + 2 tool results
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn build_messages_includes_knowledge() {
        let context = AssembledContext {
            system_anchor: "System.".to_string(),
            task_manifest: String::new(),
            injected_knowledge: vec!["fact1".to_string()],
            tool_results: vec![],
            history: String::new(),
            total_tokens: 100,
            slot_tokens: SlotTokenCounts::default(),
        };

        let messages = build_messages(&context);
        assert!(messages[0].content.contains("fact1"));
    }
}
