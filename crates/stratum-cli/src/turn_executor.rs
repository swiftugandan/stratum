//! Simplified turn executor: no compaction, no hygiene, no HITL.

use stratum_core::ports::{LlmClient, SessionManager, ToolGateway};
use stratum_core::*;

use crate::wiring::AppContext;

/// Execute a single agent turn: assemble context -> LLM call -> tool routing.
pub async fn execute_turn(
    run_id: RunId,
    history: &[LlmMessage],
    system_prompt: &str,
    task_goal: &str,
    ctx: &AppContext,
) -> anyhow::Result<TurnOutcome> {
    // 1. Get run state
    let run = ctx
        .session
        .get_run(run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("run {run_id} not found"))?;

    // 2. Assemble context
    let messages = stratum_engine::context::assemble_context(
        system_prompt,
        task_goal,
        history,
        &run.context_budget,
    );

    // 3. Get tool definitions
    let tool_defs = ctx.tool_registry.get_manifest_owned();
    let tools_ref = if tool_defs.is_empty() {
        None
    } else {
        Some(tool_defs.as_slice())
    };

    // 4. Call LLM
    tracing::info!(run_id = %run_id, model = %ctx.config.model, "calling LLM");
    let response = ctx
        .llm
        .complete(&messages, &ctx.config.model, tools_ref)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // 5. Route response
    if !response.tool_calls.is_empty() {
        let mut results = Vec::new();
        for tc in &response.tool_calls {
            tracing::info!(run_id = %run_id, tool = %tc.tool_name, "executing tool call");
            let invocation = ToolInvocation {
                tool_name: tc.tool_name.clone(),
                parameters: tc.arguments.clone(),
                run_id,
            };
            let result = ctx
                .tool_gateway
                .call_tool(invocation)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            results.push(result);
        }

        // Checkpoint after tool calls
        ctx.session
            .checkpoint(&Checkpoint {
                id: uuid::Uuid::new_v4(),
                run_id,
                state: run.state,
                goal: task_goal.to_string(),
                context_summary: format!("Turn with {} tool calls", response.tool_calls.len()),
                tool_call_log: results.clone(),
                created_at: chrono::Utc::now(),
            })
            .await?;

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
