//! Daemon-only run loop: no initialiser, no resume, no HITL.

use stratum_core::ports::{MemoryStore, SessionManager};
use stratum_core::*;

use crate::wiring::AppContext;

/// Run a task from the queue. Creates a run, executes turns until completion.
pub async fn run_from_queue(task_goal: &str, ctx: &AppContext) -> anyhow::Result<()> {
    // Build system prompt
    let tool_names: Vec<String> = ctx
        .tool_registry
        .get_manifest_owned()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let system_prompt = stratum_engine::prompt::daemon_system_anchor(&tool_names);

    // Create run in Running state (no initialiser phase)
    let run = StratumRun {
        id: uuid::Uuid::new_v4(),
        parent_run_id: None,
        model_ref: ctx.config.model.clone(),
        tool_manifest: tool_names,
        context_budget: ContextBudget::default(),
        spawn_depth_limit: 2,
        state: RunState::Running,
        created_at: chrono::Utc::now(),
    };
    let run = ctx.session.create_run(run).await?;
    let run_id = run.id;
    tracing::info!(run_id = %run_id, "created queue run");

    worker_loop(run_id, &system_prompt, task_goal, ctx).await
}

/// Core worker loop: execute turns until completion or error.
async fn worker_loop(
    run_id: RunId,
    system_prompt: &str,
    task_goal: &str,
    ctx: &AppContext,
) -> anyhow::Result<()> {
    let max_turns = 100u32;
    let mut history: Vec<LlmMessage> = Vec::new();

    for turn in 1..=max_turns {
        tracing::info!(run_id = %run_id, turn, "executing turn");

        match crate::turn_executor::execute_turn(run_id, &history, system_prompt, task_goal, ctx)
            .await
        {
            Ok(TurnOutcome::Completed) => {
                ctx.session
                    .transition_state(run_id, RunState::Completed)
                    .await?;
                tracing::info!(run_id = %run_id, turn, "run completed");
                eprintln!("Completed after {turn} turns.");
                return Ok(());
            }
            Ok(TurnOutcome::Response { content }) => {
                tracing::debug!(run_id = %run_id, "got text response, continuing");
                history.push(LlmMessage {
                    role: "assistant".to_string(),
                    content: content.clone(),
                });
                // Add a continue prompt
                history.push(LlmMessage {
                    role: "user".to_string(),
                    content: "Continue with the next step.".to_string(),
                });
            }
            Ok(TurnOutcome::ToolCalls { results }) => {
                tracing::info!(
                    run_id = %run_id,
                    count = results.len(),
                    "tool calls executed"
                );

                // Process _builtin_action markers
                let mut results = results;
                for r in &mut results {
                    if r.status == ToolResultStatus::Success
                        && r.output.get("_builtin_action").is_some()
                    {
                        if let Some(replacement) = process_builtin_action(&r.output, ctx).await {
                            r.output = replacement;
                        }
                    }
                }

                // Feed results into history for next turn
                let mut tool_output = String::new();
                for r in &results {
                    let status_str = match r.status {
                        ToolResultStatus::Success => "success",
                        ToolResultStatus::ValidationFailure => "validation_failure",
                        ToolResultStatus::ExecutionError => "error",
                    };
                    tool_output.push_str(&format!(
                        "Tool `{}` ({}): {}\n",
                        r.tool_name, status_str, r.output
                    ));
                }
                history.push(LlmMessage {
                    role: "user".to_string(),
                    content: tool_output,
                });
            }
            Ok(TurnOutcome::Error { message }) => {
                ctx.session
                    .transition_state(run_id, RunState::Failed)
                    .await?;
                tracing::error!(run_id = %run_id, %message, "turn error");
                return Err(anyhow::anyhow!("turn error: {message}"));
            }
            Err(e) => {
                ctx.session
                    .transition_state(run_id, RunState::Failed)
                    .await?;
                tracing::error!(run_id = %run_id, error = %e, "turn executor error");
                return Err(e);
            }
        }
    }

    // Max turns exceeded
    ctx.session
        .transition_state(run_id, RunState::Failed)
        .await?;
    Err(anyhow::anyhow!("max turns ({max_turns}) exceeded"))
}

/// Process a `_builtin_action` marker from a stateful built-in tool.
async fn process_builtin_action(
    val: &serde_json::Value,
    ctx: &AppContext,
) -> Option<serde_json::Value> {
    let action = val.get("_builtin_action").and_then(|v| v.as_str())?;

    match action {
        "create_tool" => {
            let name = val["name"].as_str().unwrap_or_default();
            let description = val["description"].as_str().unwrap_or_default();
            let schema = val
                .get("schema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"}));
            let script_path = val["script_path"].as_str().map(String::from);
            let param_passing = val["param_passing"].as_str().unwrap_or("stdin");

            let def = ToolDefinition {
                name: name.to_string(),
                description: description.to_string(),
                schema,
            };

            match ctx
                .tool_registry
                .register_dynamic(def, script_path, param_passing)
            {
                Ok(()) => {
                    tracing::info!(tool = name, "registered dynamic tool");
                    eprintln!("Tool created: {name}");
                }
                Err(e) => {
                    tracing::error!(tool = name, error = %e, "failed to register dynamic tool");
                }
            }
            None
        }
        "create_skill" => {
            let params = stratum_engine::tools::create_skill::CreateSkillParams {
                name: val["name"].as_str().unwrap_or_default().to_string(),
                description: val["description"].as_str().unwrap_or_default().to_string(),
                triggers: stratum_engine::tools::create_skill::parse_triggers(val),
                content: val["content"].as_str().unwrap_or_default().to_string(),
            };

            let skills_dir = ctx.config.skills_dir();
            match stratum_engine::tools::create_skill::write_skill_file(&skills_dir, &params).await
            {
                Ok(path) => {
                    tracing::info!(skill = params.name, path = %path.display(), "created skill");
                    eprintln!("Skill created: {}", params.name);
                }
                Err(e) => {
                    tracing::error!(skill = params.name, error = %e, "failed to create skill");
                }
            }
            None
        }
        "memory_write" => {
            let tier = parse_memory_tier(val["tier"].as_str().unwrap_or("working"));
            let id = val["id"].as_str().unwrap_or_default();
            let content = val["content"].as_str().unwrap_or_default();

            let entry = MemoryEntry {
                id: id.to_string(),
                tier,
                content: content.to_string(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };

            match ctx.memory.write(tier, &entry).await {
                Ok(()) => tracing::info!(tier = ?tier, id, "memory written"),
                Err(e) => tracing::error!(tier = ?tier, id, error = %e, "memory write failed"),
            }
            None
        }
        "memory_search" => {
            let tier = parse_memory_tier(val["tier"].as_str().unwrap_or("working"));
            let query = val["query"].as_str().unwrap_or_default();
            let limit = val["limit"].as_u64().unwrap_or(10) as usize;

            match ctx.memory.search(tier, query, limit).await {
                Ok(results) => {
                    tracing::info!(tier = ?tier, query, count = results.len(), "memory search");
                    let entries: Vec<_> = results
                        .iter()
                        .map(|r| {
                            serde_json::json!({
                                "id": r.entry.id,
                                "content": r.entry.content,
                                "score": r.relevance_score,
                            })
                        })
                        .collect();
                    Some(serde_json::json!({
                        "results": entries,
                        "count": results.len(),
                    }))
                }
                Err(e) => {
                    tracing::error!(tier = ?tier, query, error = %e, "memory search failed");
                    None
                }
            }
        }
        _ => {
            tracing::warn!(action, "unknown _builtin_action");
            None
        }
    }
}

fn parse_memory_tier(s: &str) -> MemoryTier {
    stratum_engine::tools::memory::parse_tier(s).unwrap_or(MemoryTier::Working)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory_tier_all_variants() {
        assert_eq!(parse_memory_tier("working"), MemoryTier::Working);
        assert_eq!(parse_memory_tier("persistent"), MemoryTier::Persistent);
        assert_eq!(parse_memory_tier("unknown"), MemoryTier::Working);
    }
}
