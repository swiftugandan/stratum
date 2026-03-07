use stratum_adapters::prompt::initialiser_prompt;
use stratum_core::turn::TurnOutcome;
use stratum_core::{LlmClient, MemoryStore, SessionManager, TurnExecutor};
use stratum_types::*;

use crate::turn_executor::DefaultTurnExecutor;
use crate::wiring::AppContext;

/// Create a new `StratumRun` from the current config.
fn new_run(ctx: &AppContext) -> StratumRun {
    StratumRun {
        id: uuid::Uuid::new_v4(),
        parent_run_id: None,
        model_ref: ctx.config.model.clone(),
        trust_level: ctx.config.trust_level,
        tool_manifest: vec![],
        memory_config: MemoryConfig::default(),
        hitl_policy: HitlPolicy::default(),
        context_budget: ContextBudget::default(),
        spawn_depth_limit: 2,
        state: RunState::Initialising,
        created_at: chrono::Utc::now(),
    }
}

/// Run a task that was dequeued from rfbmq. Uses daemon-mode system prompt.
pub async fn run_from_queue(task: &str, ctx: &AppContext) -> anyhow::Result<()> {
    // Use the daemon system anchor instead of the default one
    let tool_names: Vec<String> = ctx
        .tool_registry
        .get_manifest_owned()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let system_anchor = stratum_adapters::prompt::daemon_system_anchor(&tool_names);

    let run = ctx.session.create_run(new_run(ctx)).await?;
    let run_id = run.id;
    tracing::info!(run_id = %run_id, "created queue run");

    // Initialize context engine with daemon prompt and task as the manifest
    ctx.context_engine
        .init_run(run_id, system_anchor, task.to_string(), String::new())
        .await;
    ctx.context_engine
        .set_task_manifest(run_id, task.to_string())
        .await;

    // Transition directly to Running (skip initialiser prompt for queue tasks)
    ctx.session
        .transition_state(run_id, RunState::Running)
        .await?;

    let executor = DefaultTurnExecutor::from_context(ctx);
    worker_loop(run_id, &executor, ctx).await
}

/// Run a new agent task from scratch.
pub async fn run_new(task: &str, ctx: &AppContext) -> anyhow::Result<()> {
    // 1. Create a new run in Initialising state
    let run = ctx.session.create_run(new_run(ctx)).await?;
    let run_id = run.id;
    tracing::info!(run_id = %run_id, "created new run");
    eprintln!("Run created: {run_id}");

    // 2. Generate and execute initialiser prompt
    let init_prompt = initialiser_prompt(&run, task);
    let messages = vec![
        LlmMessage {
            role: "system".to_string(),
            content: "You are an AI agent planning a task.".to_string(),
        },
        LlmMessage {
            role: "user".to_string(),
            content: init_prompt,
        },
    ];

    tracing::info!(run_id = %run_id, "calling LLM for initialisation");
    eprintln!("Initialising...");
    let response = ctx.llm.complete(&messages, &ctx.config.model, None).await?;

    // 3. Parse artefacts from response
    let artefacts = parse_artefacts(&response.content)?;
    artefacts.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(run_id = %run_id, "artefacts parsed and validated");

    // 4. Initialize context engine with artefacts
    ctx.context_engine
        .init_run(
            run_id,
            "You are an AI agent operating within the Stratum harness.".to_string(),
            artefacts.task_md.clone(),
            artefacts.progress_md.clone(),
        )
        .await;
    ctx.context_engine
        .set_task_manifest(run_id, artefacts.task_md.clone())
        .await;

    // 5. Transition to Running
    ctx.session
        .transition_state(run_id, RunState::Running)
        .await?;
    eprintln!("Running...");

    // 6. Worker loop
    let executor = DefaultTurnExecutor::from_context(ctx);
    worker_loop(run_id, &executor, ctx).await
}

/// Resume a paused or checkpointed run.
pub async fn resume_run(run_id: RunId, ctx: &AppContext) -> anyhow::Result<()> {
    // 1. Resume — transitions Checkpointed → Resuming
    let run = ctx.session.resume(run_id).await?;
    tracing::info!(run_id = %run_id, state = ?run.state, "resuming run");
    eprintln!("Resuming run: {run_id}");

    // 2. Transition to Running
    ctx.session
        .transition_state(run_id, RunState::Running)
        .await?;
    eprintln!("Running...");

    // 3. Initialize context engine (minimal — just set system anchor)
    ctx.context_engine
        .init_run(
            run_id,
            "You are an AI agent operating within the Stratum harness.".to_string(),
            String::new(),
            String::new(),
        )
        .await;

    // 4. Worker loop
    let executor = DefaultTurnExecutor::from_context(ctx);
    worker_loop(run_id, &executor, ctx).await
}

/// The core worker loop: execute turns until completion, pause, or error.
async fn worker_loop(
    run_id: RunId,
    executor: &DefaultTurnExecutor,
    ctx: &AppContext,
) -> anyhow::Result<()> {
    let mut turn = 0u32;
    let max_turns = 100; // safety limit

    loop {
        turn += 1;
        if turn > max_turns {
            tracing::warn!(run_id = %run_id, "max turns reached, stopping");
            ctx.session
                .transition_state(run_id, RunState::Failed)
                .await?;
            eprintln!("Failed: max turns ({max_turns}) exceeded");
            return Ok(());
        }

        tracing::info!(run_id = %run_id, turn, "executing turn");
        eprintln!("Turn {turn}...");

        match executor.execute_turn(run_id).await {
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
                // Feed response back into context for next turn
                ctx.context_engine.append_history(run_id, content).await;
            }
            Ok(TurnOutcome::ToolCalls { results }) => {
                tracing::info!(
                    run_id = %run_id,
                    count = results.len(),
                    "tool calls executed"
                );

                // Process _builtin_action markers from stateful tools,
                // replacing marker output with real results where applicable.
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

                // Feed results into context for next turn
                for r in &results {
                    let text = format!(
                        "Tool `{}` ({}): {}",
                        r.tool_name,
                        match r.status {
                            ToolResultStatus::Success => "success",
                            ToolResultStatus::ValidationFailure => "validation_failure",
                            ToolResultStatus::ExecutionError => "error",
                            ToolResultStatus::PolicyDenied => "denied",
                        },
                        r.output
                    );
                    ctx.context_engine
                        .push_tool_result(run_id, text, r.status != ToolResultStatus::Success)
                        .await;
                }
            }
            Ok(TurnOutcome::Paused { gate }) => {
                ctx.session
                    .transition_state(run_id, RunState::Paused)
                    .await?;
                tracing::info!(run_id = %run_id, gate_id = %gate.id, "run paused at gate");
                eprintln!(
                    "Paused at HITL gate: {} ({})\nResume with: stratum decide {} approve",
                    gate.gate_category, gate.action_attempted, run_id
                );
                return Ok(());
            }
            Ok(TurnOutcome::Error { message }) => {
                ctx.session
                    .transition_state(run_id, RunState::Failed)
                    .await?;
                tracing::error!(run_id = %run_id, %message, "turn error");
                eprintln!("Failed: {message}");
                return Ok(());
            }
            Err(e) => {
                ctx.session
                    .transition_state(run_id, RunState::Failed)
                    .await?;
                tracing::error!(run_id = %run_id, error = %e, "turn executor error");
                eprintln!("Failed: {e}");
                return Ok(());
            }
        }
    }
}

/// Process a `_builtin_action` marker from a stateful built-in tool.
///
/// These markers are produced by `BuiltinExecutor` for tools that need access to
/// shared state (PersistentToolRegistry, MemoryStore, skill files). The wiring
/// layer intercepts the markers here and performs the actual mutations.
/// Returns `Some(replacement_output)` if the action produces a result the agent should see
/// (e.g., memory_search results). Returns `None` for actions where the marker itself is sufficient.
async fn process_builtin_action(
    val: &serde_json::Value,
    ctx: &AppContext,
) -> Option<serde_json::Value> {
    let action = match val.get("_builtin_action").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return None,
    };

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
                trust_level_required: TrustLevel::Supervised,
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
            let params = stratum_tools::builtin::create_skill::CreateSkillParams {
                name: val["name"].as_str().unwrap_or_default().to_string(),
                description: val["description"].as_str().unwrap_or_default().to_string(),
                triggers: val
                    .get("triggers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                content: val["content"].as_str().unwrap_or_default().to_string(),
            };

            let skills_dir = ctx.config.data_dir.join("skills");
            match stratum_tools::builtin::create_skill::write_skill_file(&skills_dir, &params).await
            {
                Ok(path) => {
                    tracing::info!(skill = params.name, path = %path.display(), "created skill file");
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
                    tracing::info!(tier = ?tier, query, count = results.len(), "memory search done");
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
        "memory_promote" => {
            let entry_id = val["entry_id"].as_str().unwrap_or_default();
            let from = parse_memory_tier(val["from_tier"].as_str().unwrap_or("working"));
            let to = parse_memory_tier(val["to_tier"].as_str().unwrap_or("episodic"));

            match ctx.memory.promote(entry_id, from, to).await {
                Ok(()) => tracing::info!(entry_id, from = ?from, to = ?to, "memory promoted"),
                Err(e) => {
                    tracing::error!(entry_id, error = %e, "memory promote failed");
                }
            }
            None
        }
        _ => {
            tracing::warn!(action, "unknown _builtin_action");
            None
        }
    }
}

fn parse_memory_tier(s: &str) -> MemoryTier {
    stratum_tools::builtin::memory::parse_tier(s).unwrap_or(MemoryTier::Working)
}

/// Parse `RunArtefacts` from LLM initialiser output.
///
/// Expects sections delimited by `=== TASK.md ===`, `=== PROGRESS.md ===`, etc.
pub fn parse_artefacts(content: &str) -> anyhow::Result<RunArtefacts> {
    fn extract_section(content: &str, marker: &str) -> Option<String> {
        let pattern = format!("=== {} ===", marker);
        let start = content.find(&pattern)?;
        let after = start + pattern.len();
        let rest = &content[after..];

        // Find the next section marker or end of content
        let end = rest.find("=== ").unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    }

    let task_md = extract_section(content, "TASK.md").unwrap_or_default();
    let progress_md = extract_section(content, "PROGRESS.md").unwrap_or_default();
    let decisions_md = extract_section(content, "DECISIONS.md").unwrap_or_default();
    let init_sh = extract_section(content, "INIT.sh");

    Ok(RunArtefacts {
        task_md,
        progress_md,
        decisions_md,
        init_sh,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_artefacts_basic() {
        let content = r#"
=== TASK.md ===
# Goal
Build a REST API

=== PROGRESS.md ===
- [ ] Set up project
- [ ] Add endpoints

=== DECISIONS.md ===
No decisions yet.
"#;
        let artefacts = parse_artefacts(content).unwrap();
        assert!(artefacts.task_md.contains("Build a REST API"));
        assert!(artefacts.progress_md.contains("Set up project"));
        assert!(artefacts.decisions_md.contains("No decisions yet"));
        assert!(artefacts.init_sh.is_none());
        assert!(artefacts.validate().is_ok());
    }

    #[test]
    fn parse_artefacts_with_init_sh() {
        let content = r#"
=== TASK.md ===
# Goal
Build a thing

=== PROGRESS.md ===
- [ ] Step 1

=== DECISIONS.md ===
None.

=== INIT.sh ===
#!/bin/bash
cargo init
"#;
        let artefacts = parse_artefacts(content).unwrap();
        assert!(artefacts.init_sh.is_some());
        assert!(artefacts.init_sh.unwrap().contains("cargo init"));
    }

    #[test]
    fn parse_artefacts_empty_yields_validation_error() {
        let content = "No sections here.";
        let artefacts = parse_artefacts(content).unwrap();
        assert!(artefacts.validate().is_err());
    }

    #[test]
    fn parse_memory_tier_all_variants() {
        assert_eq!(parse_memory_tier("working"), MemoryTier::Working);
        assert_eq!(parse_memory_tier("episodic"), MemoryTier::Episodic);
        assert_eq!(parse_memory_tier("project"), MemoryTier::Project);
        assert_eq!(parse_memory_tier("global"), MemoryTier::Global);
        assert_eq!(parse_memory_tier("unknown"), MemoryTier::Working); // default
    }
}
