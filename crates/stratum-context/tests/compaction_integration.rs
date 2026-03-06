//! Comprehensive context compaction and long-run simulation tests.
//!
//! Exercises the 3-stage compaction pipeline, budget thresholds, hygiene scoring,
//! re-anchor injection, todo recitation, and trajectory event emission under
//! realistic multi-turn workloads.

use std::sync::Arc;
use std::sync::Mutex;

use stratum_context::engine::{ContextEngineConfig, DefaultContextEngine};
use stratum_core::{ContextEngine, SessionManager, TrajectoryStore};
use stratum_test_utils::mocks::{
    make_test_run, MockLlmClient, MockSessionManager, MockTrajectoryStore,
};
use stratum_types::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a running engine with configurable settings and an optional custom LLM client.
async fn setup_engine(
    config: ContextEngineConfig,
    llm: Arc<MockLlmClient>,
) -> (
    DefaultContextEngine<MockSessionManager, MockTrajectoryStore, MockLlmClient>,
    RunId,
    Arc<MockTrajectoryStore>,
) {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());

    let run = StratumRun {
        state: RunState::Running,
        ..make_test_run()
    };
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let engine = DefaultContextEngine::new(config, session, trajectory.clone(), llm);
    engine
        .init_run(
            run_id,
            "You are a helpful coding assistant.".to_string(),
            "Build a REST API with CRUD endpoints".to_string(),
            "- [ ] Set up project\n- [ ] Add endpoints\n- [ ] Write tests".to_string(),
        )
        .await;

    (engine, run_id, trajectory)
}

/// Convenience wrapper using the default MockLlmClient.
async fn setup_with_config(
    config: ContextEngineConfig,
) -> (
    DefaultContextEngine<MockSessionManager, MockTrajectoryStore, MockLlmClient>,
    RunId,
    Arc<MockTrajectoryStore>,
) {
    setup_engine(config, Arc::new(MockLlmClient::default())).await
}

/// Generate a large string that tokenises to many tokens.
fn large_text(repeat: usize) -> String {
    "The quick brown fox jumps over the lazy dog. ".repeat(repeat)
}

// ---------------------------------------------------------------------------
// 1. Budget approaches compaction threshold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_long_run_budget_approaches_threshold() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    // Push many tool results and history to accumulate tokens.
    for i in 0..30 {
        engine
            .push_tool_result(
                run_id,
                format!("Tool result {i}: operation completed successfully with detailed output."),
                false,
            )
            .await;
    }
    for i in 0..20 {
        engine
            .append_history(
                run_id,
                format!("Turn {i}: The assistant continued working on the REST API endpoints."),
            )
            .await;
    }

    let ctx = engine.assemble_context(run_id).await.unwrap();

    // Create a budget where total_tokens falls between threshold and ceiling.
    // compaction_threshold = 0.85, so threshold_line = ceiling * 0.85
    // We want: threshold_line < total_tokens < ceiling
    let total = ctx.total_tokens;
    // Set ceiling so that utilisation is ~90% (above 85% threshold but below 100%)
    let ceiling = (total as f64 / 0.90) as u32;

    let budget = ContextBudget {
        system_anchor: 50_000,
        task_manifest: 50_000,
        injected_knowledge: 50_000,
        tool_results: 50_000,
        history: 50_000,
        total_ceiling: ceiling,
        compaction_threshold: 0.85,
    };

    let status = engine.check_budget(&ctx, &budget);
    match status {
        BudgetStatus::ApproachingThreshold { utilisation } => {
            assert!(utilisation > 0.85, "utilisation should be above threshold");
            assert!(utilisation < 1.0, "utilisation should be below ceiling");
        }
        other => panic!("expected ApproachingThreshold, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 2. Budget exceeds ceiling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_long_run_budget_exceeds_ceiling() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    // Push enough content to blow through the ceiling (1,000 token budget).
    let huge_history = "word ".repeat(2_000);
    engine.append_history(run_id, huge_history).await;

    let ctx = engine.assemble_context(run_id).await.unwrap();

    let budget = ContextBudget {
        system_anchor: 50_000,
        task_manifest: 50_000,
        injected_knowledge: 50_000,
        tool_results: 50_000,
        history: 500, // very low slot budget
        total_ceiling: 1_000,
        compaction_threshold: 0.85,
    };

    let status = engine.check_budget(&ctx, &budget);
    match status {
        BudgetStatus::OverBudget { overage_tokens } => {
            assert!(overage_tokens > 0, "overage_tokens must be positive");
        }
        other => panic!("expected OverBudget, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 3. Compaction Stage 1 (Offload) reduces tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_compaction_stage1_offload_reduces_tokens() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 10, // very low to force offload
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    // Push several large tool results. Each has many lines so the 10-line preview
    // kept during offload is much smaller than the original.
    for i in 0..5 {
        let lines: String = (0..200)
            .map(|j| format!("Line {j}: Tool result {i} output detail about operation {j}."))
            .collect::<Vec<_>>()
            .join("\n");
        engine.push_tool_result(run_id, lines, false).await;
    }

    let ctx_before = engine.assemble_context(run_id).await.unwrap();
    let tokens_before = ctx_before.total_tokens;

    engine
        .trigger_compaction(run_id, CompactionStage::Offload)
        .await
        .unwrap();

    let ctx_after = engine.assemble_context(run_id).await.unwrap();
    let tokens_after = ctx_after.total_tokens;

    // Token count should decrease after offload.
    assert!(
        tokens_after < tokens_before,
        "offload should reduce tokens: before={tokens_before}, after={tokens_after}"
    );

    // All tool results should now contain offload references.
    for entry in &ctx_after.tool_results {
        assert!(
            entry.starts_with("[Offloaded to "),
            "expected offloaded reference, got: {}",
            &entry[..entry.len().min(80)]
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Compaction Stage 2 (Truncate) removes previews
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_compaction_stage2_truncate_removes_previews() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 10,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    // Push large tool results and offload them first.
    for i in 0..3 {
        let large_result = format!("Tool result {i}:\n{}", large_text(50));
        engine.push_tool_result(run_id, large_result, false).await;
    }

    engine
        .trigger_compaction(run_id, CompactionStage::Offload)
        .await
        .unwrap();

    let ctx_after_offload = engine.assemble_context(run_id).await.unwrap();
    let tokens_after_offload = ctx_after_offload.total_tokens;

    // Offloaded entries still contain preview lines. Truncate should strip them.
    engine
        .trigger_compaction(run_id, CompactionStage::Truncate)
        .await
        .unwrap();

    let ctx_after_truncate = engine.assemble_context(run_id).await.unwrap();
    let tokens_after_truncate = ctx_after_truncate.total_tokens;

    assert!(
        tokens_after_truncate <= tokens_after_offload,
        "truncate should further reduce tokens: offload={tokens_after_offload}, truncate={tokens_after_truncate}"
    );

    // Each truncated entry should be a single line (just the reference, no preview).
    for entry in &ctx_after_truncate.tool_results {
        assert!(
            entry.starts_with("[Offloaded to "),
            "expected offload reference"
        );
        let line_count = entry.lines().count();
        assert_eq!(
            line_count, 1,
            "truncated entry should be a single line, got {line_count} lines"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Compaction Stage 3 (Summarise) compresses history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_compaction_stage3_summarise_compresses_history() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };

    let summary = "Summary of conversation.";
    let llm = Arc::new(MockLlmClient {
        responses: Mutex::new(vec![LlmResponse {
            content: summary.to_string(),
            tool_calls: vec![],
            usage: LlmUsage {
                input_tokens: 100,
                output_tokens: 50,
                cached_tokens: 0,
            },
            stop_reason: Some("end_turn".to_string()),
        }]),
    });

    let (engine, run_id, _trajectory) = setup_engine(config, llm).await;

    // Accumulate a long conversation history.
    for i in 0..50 {
        engine
            .append_history(
                run_id,
                format!(
                    "Turn {i}: User asked to implement endpoint /api/items/{i}. \
                     Assistant created the handler, added validation, wrote tests, \
                     and verified the endpoint works correctly with curl."
                ),
            )
            .await;
    }

    let ctx_before = engine.assemble_context(run_id).await.unwrap();
    let history_tokens_before = ctx_before.slot_tokens.history;
    assert!(
        history_tokens_before > 100,
        "history should be substantial before summarise"
    );

    engine
        .trigger_compaction(run_id, CompactionStage::Summarise)
        .await
        .unwrap();

    let ctx_after = engine.assemble_context(run_id).await.unwrap();

    // History should be replaced with the LLM summary.
    assert_eq!(ctx_after.history, summary);
    assert!(
        ctx_after.slot_tokens.history < history_tokens_before,
        "summarised history should have fewer tokens: before={history_tokens_before}, after={}",
        ctx_after.slot_tokens.history
    );
}

// ---------------------------------------------------------------------------
// 6. Error tool results preserved during compaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_error_tool_results_preserved_during_compaction() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 10, // very low to force offload on non-error entries
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    let large_ok = large_text(50);
    let large_error = format!("ERROR: compilation failed\n{}", large_text(50));

    // Push a normal result and an error result, both large enough to trigger offload.
    engine.push_tool_result(run_id, large_ok, false).await;
    engine
        .push_tool_result(run_id, large_error.clone(), true)
        .await;

    // Stage 1: Offload
    engine
        .trigger_compaction(run_id, CompactionStage::Offload)
        .await
        .unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    // Normal entry should be offloaded.
    assert!(
        ctx.tool_results[0].starts_with("[Offloaded to "),
        "normal entry should be offloaded"
    );
    // Error entry should be preserved in full.
    assert!(
        ctx.tool_results[1].starts_with("ERROR:"),
        "error entry should be preserved, got: {}",
        &ctx.tool_results[1][..ctx.tool_results[1].len().min(60)]
    );
    assert_eq!(
        ctx.tool_results[1], large_error,
        "error entry content should be unchanged"
    );

    // Stage 2: Truncate — error entries still protected.
    engine
        .trigger_compaction(run_id, CompactionStage::Truncate)
        .await
        .unwrap();

    let ctx2 = engine.assemble_context(run_id).await.unwrap();
    // Normal offloaded entry should be truncated to reference-only (single line).
    assert_eq!(
        ctx2.tool_results[0].lines().count(),
        1,
        "normal offloaded entry should be truncated to one line"
    );
    // Error entry should still be fully intact.
    assert_eq!(
        ctx2.tool_results[1], large_error,
        "error entry should survive truncation unchanged"
    );
}

// ---------------------------------------------------------------------------
// 7. Hygiene score degrades with off-topic content
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_hygiene_score_degrades_with_offtopic() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 10, // high limit so re-anchor doesn't fire
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    // Task goal is "Build a REST API with CRUD endpoints" (set in setup_with_config).
    // Push completely off-topic history.
    for _ in 0..10 {
        engine
            .append_history(
                run_id,
                "The weather in Antarctica is extremely cold with penguins \
                 swimming in the freezing ocean currents near the ice shelves."
                    .to_string(),
            )
            .await;
    }

    let score = engine.compute_hygiene_score(run_id, 20).await.unwrap();
    assert!(
        score.score < 0.5,
        "off-topic content should produce a low hygiene score, got {}",
        score.score
    );
    // With threshold 0.6, score < 0.5 means it degraded.
    assert!(
        score.consecutive_degraded >= 1,
        "should have at least 1 consecutive degradation"
    );
}

// ---------------------------------------------------------------------------
// 8. Re-anchor injection after consecutive degradation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reanchor_injection_after_consecutive_degradation() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.99, // very high threshold so almost everything degrades
        hygiene_consecutive_limit: 3, // trigger re-anchor after 3 consecutive
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, trajectory) = setup_with_config(config).await;

    // Push off-topic history.
    engine
        .append_history(
            run_id,
            "Unrelated discussion about pizza toppings and pineapple preferences.".to_string(),
        )
        .await;

    // Compute hygiene 3 times to hit the consecutive limit.
    let s1 = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    assert_eq!(s1.consecutive_degraded, 1);

    let s2 = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    assert_eq!(s2.consecutive_degraded, 2);

    // Third call should trigger re-anchor (consecutive_degraded reaches 3).
    let s3 = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    // After re-anchor, consecutive is reset to 0 inside the function,
    // but the returned value reflects the count *at* the trigger point (3),
    // then reset happens. The returned consecutive_degraded is post-reset = 0
    // because compute_hygiene_score resets to 0 when needs_reanchor is true.
    // Let's just verify the re-anchor block is in history.
    let _ = s3;

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(
        ctx.history.contains("RE-ANCHOR"),
        "re-anchor block should be injected into history"
    );
    assert!(
        ctx.history.contains("Build a REST API with CRUD endpoints"),
        "re-anchor block should contain the original task goal"
    );

    // Verify ReanchorInjected event was emitted.
    let events = trajectory
        .query_events(Some(run_id), Some(EventType::ReanchorInjected), None, None)
        .await
        .unwrap();
    assert!(
        !events.is_empty(),
        "ReanchorInjected event should be emitted"
    );
}

// ---------------------------------------------------------------------------
// 9. Todo recitation at interval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_todo_recitation_at_interval() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, trajectory) = setup_with_config(config).await;

    // Set specific progress content.
    engine
        .set_progress(
            run_id,
            "- [x] Set up project\n- [ ] Add endpoints\n- [ ] Write tests".to_string(),
        )
        .await;

    // Advance to turn 4 — should NOT trigger (interval = 5).
    for _ in 0..4 {
        engine.advance_turn(run_id).await;
    }
    engine.inject_todo_recitation(run_id).await.unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(
        !ctx.history.contains("TODO Recitation"),
        "should not inject at turn 4 (interval is 5)"
    );

    // Advance to turn 5 — should trigger.
    engine.advance_turn(run_id).await;
    engine.inject_todo_recitation(run_id).await.unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(
        ctx.history.contains("TODO Recitation (turn 5)"),
        "should inject at turn 5"
    );
    assert!(
        ctx.history.contains("Add endpoints"),
        "recitation should contain progress content"
    );
    assert!(
        ctx.history.contains("Write tests"),
        "recitation should contain progress content"
    );

    // Verify TodoRecitationInjected event was emitted.
    let events = trajectory
        .query_events(
            Some(run_id),
            Some(EventType::TodoRecitationInjected),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(events.len(), 1, "exactly one recitation event expected");

    // Advance to turn 10 — should trigger again.
    for _ in 0..5 {
        engine.advance_turn(run_id).await;
    }
    engine.inject_todo_recitation(run_id).await.unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(
        ctx.history.contains("TODO Recitation (turn 10)"),
        "should inject again at turn 10"
    );
}

// ---------------------------------------------------------------------------
// 10. Compaction events emitted for all stages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_compaction_events_emitted() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 10,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };

    let llm = Arc::new(MockLlmClient {
        responses: Mutex::new(vec![LlmResponse {
            content: "Summarised history.".to_string(),
            tool_calls: vec![],
            usage: LlmUsage {
                input_tokens: 100,
                output_tokens: 20,
                cached_tokens: 0,
            },
            stop_reason: Some("end_turn".to_string()),
        }]),
    });

    let (engine, run_id, trajectory) = setup_engine(config, llm).await;

    // Push large tool results for offload/truncate and history for summarise.
    for i in 0..3 {
        engine
            .push_tool_result(run_id, format!("Result {i}:\n{}", large_text(30)), false)
            .await;
    }
    engine
        .append_history(
            run_id,
            "Long conversation about building REST APIs with detailed discussion.".to_string(),
        )
        .await;

    // Stage 1: Offload
    engine
        .trigger_compaction(run_id, CompactionStage::Offload)
        .await
        .unwrap();

    let stage1_events = trajectory
        .query_events(
            Some(run_id),
            Some(EventType::CompactionStage1Triggered),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        stage1_events.len(),
        1,
        "CompactionStage1Triggered event should be emitted"
    );
    // Verify payload contains expected fields.
    let payload = &stage1_events[0].payload;
    assert!(
        payload.get("tokens_freed").is_some(),
        "payload should contain tokens_freed"
    );
    assert!(
        payload.get("files_offloaded").is_some(),
        "payload should contain files_offloaded"
    );

    // Stage 2: Truncate
    engine
        .trigger_compaction(run_id, CompactionStage::Truncate)
        .await
        .unwrap();

    let stage2_events = trajectory
        .query_events(
            Some(run_id),
            Some(EventType::CompactionStage2Triggered),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        stage2_events.len(),
        1,
        "CompactionStage2Triggered event should be emitted"
    );
    let payload2 = &stage2_events[0].payload;
    assert!(
        payload2.get("tokens_freed").is_some(),
        "truncate payload should contain tokens_freed"
    );

    // Stage 3: Summarise
    engine
        .trigger_compaction(run_id, CompactionStage::Summarise)
        .await
        .unwrap();

    let stage3_events = trajectory
        .query_events(
            Some(run_id),
            Some(EventType::CompactionStage3Triggered),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        stage3_events.len(),
        1,
        "CompactionStage3Triggered event should be emitted"
    );
    let payload3 = &stage3_events[0].payload;
    assert!(
        payload3.get("history_tokens").is_some(),
        "summarise payload should contain history_tokens"
    );
    assert!(
        payload3.get("action").is_some(),
        "summarise payload should contain action"
    );
}

// ---------------------------------------------------------------------------
// 11. Slot budget exceeded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_slot_budget_exceeded() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 100,
        todo_recitation_interval: 5,
        hygiene_threshold: 0.6,
        hygiene_consecutive_limit: 3,
        summarisation_model: "test-model".to_string(),
    };
    let (engine, run_id, _trajectory) = setup_with_config(config).await;

    // Push enough tool results to exceed the tool_results slot budget,
    // even though total_ceiling is generous.
    for i in 0..50 {
        engine
            .push_tool_result(
                run_id,
                format!(
                    "Tool result {i}: detailed output with lots of context about the operation \
                     performed including file paths, line numbers, and diagnostic information."
                ),
                false,
            )
            .await;
    }

    let ctx = engine.assemble_context(run_id).await.unwrap();

    // Set a very low tool_results slot budget but high everything else.
    let budget = ContextBudget {
        system_anchor: 50_000,
        task_manifest: 50_000,
        injected_knowledge: 50_000,
        tool_results: 10, // extremely low — will be exceeded
        history: 50_000,
        total_ceiling: 500_000,
        compaction_threshold: 0.85,
    };

    let status = engine.check_budget(&ctx, &budget);
    match status {
        BudgetStatus::OverBudget { overage_tokens } => {
            assert!(
                overage_tokens > 0,
                "slot overage should be positive when tool_results slot is exceeded"
            );
        }
        other => panic!("expected OverBudget due to slot limit, got {:?}", other),
    }
}
