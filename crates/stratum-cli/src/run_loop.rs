use stratum_adapters::prompt::initialiser_prompt;
use stratum_core::turn::TurnOutcome;
use stratum_core::{LlmClient, SessionManager, TurnExecutor};
use stratum_types::*;

use crate::turn_executor::DefaultTurnExecutor;
use crate::wiring::AppContext;

/// Run a new agent task from scratch.
pub async fn run_new(task: &str, ctx: &AppContext) -> anyhow::Result<()> {
    // 1. Create a new run in Initialising state
    let run = StratumRun {
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
    };

    let run = ctx.session.create_run(run).await?;
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
}
