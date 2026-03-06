//! Integration tests for DefaultContextEngine.
//!
//! Uses mock implementations from stratum-test-utils to exercise the full
//! ContextEngine trait through assemble_context, check_budget, compaction,
//! hygiene scoring, and todo recitation.

use std::sync::Arc;

use stratum_context::{ContextEngineConfig, DefaultContextEngine};
use stratum_core::{ContextEngine, SessionManager, TrajectoryStore};
use stratum_test_utils::mocks::*;
use stratum_types::*;
use uuid::Uuid;

fn make_run() -> StratumRun {
    StratumRun {
        id: Uuid::new_v4(),
        parent_run_id: None,
        model_ref: "claude-sonnet-4-20250514".to_string(),
        trust_level: TrustLevel::Supervised,
        tool_manifest: vec!["read_file".to_string()],
        memory_config: MemoryConfig::default(),
        hitl_policy: HitlPolicy::default(),
        context_budget: ContextBudget::default(),
        spawn_depth_limit: 2,
        state: RunState::Running,
        created_at: chrono::Utc::now(),
    }
}

async fn setup() -> (
    DefaultContextEngine<MockSessionManager, MockTrajectoryStore, MockLlmClient>,
    RunId,
) {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let llm = Arc::new(MockLlmClient::default());

    let run = make_run();
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let config = ContextEngineConfig {
        offload_dir: std::env::temp_dir().join("stratum-test-offload"),
        todo_recitation_interval: 3,
        ..Default::default()
    };

    let engine = DefaultContextEngine::new(config, session, trajectory, llm);
    engine
        .init_run(
            run_id,
            "You are a helpful assistant.".to_string(),
            "Build a REST API".to_string(),
            "- [ ] Set up project\n- [ ] Add endpoints".to_string(),
        )
        .await;

    (engine, run_id)
}

#[tokio::test]
async fn test_assemble_empty_context() {
    let (engine, run_id) = setup().await;

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert_eq!(ctx.system_anchor, "You are a helpful assistant.");
    assert!(ctx.tool_results.is_empty());
    assert!(ctx.injected_knowledge.is_empty());
    assert!(ctx.history.is_empty());
    assert!(ctx.total_tokens > 0); // system_anchor has tokens
}

#[tokio::test]
async fn test_assemble_with_content() {
    let (engine, run_id) = setup().await;

    engine
        .set_task_manifest(run_id, "# Goal\nBuild a REST API".to_string())
        .await;
    engine
        .push_tool_result(run_id, "read_file result: 200 OK".to_string(), false)
        .await;
    engine
        .append_history(run_id, "User: Build the API".to_string())
        .await;
    engine
        .push_knowledge(run_id, "REST best practices doc".to_string())
        .await;

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert_eq!(ctx.task_manifest, "# Goal\nBuild a REST API");
    assert_eq!(ctx.tool_results.len(), 1);
    assert_eq!(ctx.injected_knowledge.len(), 1);
    assert!(!ctx.history.is_empty());
    assert!(ctx.total_tokens > 10);
}

#[tokio::test]
async fn test_budget_within() {
    let (engine, run_id) = setup().await;
    let ctx = engine.assemble_context(run_id).await.unwrap();
    let budget = ContextBudget::default();

    let status = engine.check_budget(&ctx, &budget);
    assert!(matches!(status, BudgetStatus::WithinBudget));
}

#[tokio::test]
async fn test_budget_over() {
    let (engine, run_id) = setup().await;

    // Fill history with enough text to blow the budget
    let large_history = "word ".repeat(200_000); // ~50K tokens
    engine.append_history(run_id, large_history).await;

    let ctx = engine.assemble_context(run_id).await.unwrap();
    let budget = ContextBudget {
        total_ceiling: 1_000,
        history: 500,
        ..Default::default()
    };

    let status = engine.check_budget(&ctx, &budget);
    assert!(matches!(status, BudgetStatus::OverBudget { .. }));
}

#[tokio::test]
async fn test_budget_approaching_threshold() {
    let (engine, run_id) = setup().await;

    // Create history that's ~87% of a small ceiling
    let history = "The quick brown fox jumps over the lazy dog. ".repeat(40);
    engine.append_history(run_id, history).await;

    let ctx = engine.assemble_context(run_id).await.unwrap();
    // Set a budget where total_tokens is between threshold (85%) and ceiling
    let total = ctx.total_tokens;
    let ceiling = (total as f32 / 0.87) as u32; // ~87% utilisation

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
    assert!(matches!(status, BudgetStatus::ApproachingThreshold { .. }));
}

#[tokio::test]
async fn test_compaction_offload() {
    let dir = tempfile::TempDir::new().unwrap();
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let llm = Arc::new(MockLlmClient::default());

    let run = make_run();
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 10, // very low threshold to force offload
        ..Default::default()
    };

    let engine = DefaultContextEngine::new(config, session, trajectory.clone(), llm);
    engine
        .init_run(
            run_id,
            "anchor".to_string(),
            "goal".to_string(),
            String::new(),
        )
        .await;

    // Push a tool result that exceeds 10 tokens
    let large_result = "The quick brown fox jumps over the lazy dog. ".repeat(20);
    engine.push_tool_result(run_id, large_result, false).await;

    engine
        .trigger_compaction(run_id, CompactionStage::Offload)
        .await
        .unwrap();

    // Verify tool result was replaced with reference
    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert_eq!(ctx.tool_results.len(), 1);
    assert!(ctx.tool_results[0].starts_with("[Offloaded to "));

    // Verify trajectory event was emitted
    let events = trajectory
        .query_events(
            Some(run_id),
            Some(EventType::CompactionStage1Triggered),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn test_compaction_truncate() {
    let (engine, run_id) = setup().await;

    // Simulate an already-offloaded entry
    engine
        .push_tool_result(
            run_id,
            "[Offloaded to /tmp/0.md] (5000 tokens)\n\npreview line 1\npreview line 2".to_string(),
            false,
        )
        .await;

    engine
        .trigger_compaction(run_id, CompactionStage::Truncate)
        .await
        .unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert_eq!(
        ctx.tool_results[0],
        "[Offloaded to /tmp/0.md] (5000 tokens)"
    );
}

#[tokio::test]
async fn test_compaction_summarise() {
    let (engine, run_id) = setup().await;

    engine
        .append_history(
            run_id,
            "User: Build a REST API\nAssistant: I'll start by setting up the project.".to_string(),
        )
        .await;

    engine
        .trigger_compaction(run_id, CompactionStage::Summarise)
        .await
        .unwrap();

    // After summarise, history should be replaced with LLM response
    let ctx = engine.assemble_context(run_id).await.unwrap();
    // MockLlmClient returns "Hello from mock LLM."
    assert_eq!(ctx.history, "Hello from mock LLM.");
}

#[tokio::test]
async fn test_hygiene_score_on_task() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let llm = Arc::new(MockLlmClient::default());

    let run = make_run();
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let config = ContextEngineConfig {
        hygiene_threshold: 0.1, // Low threshold — most related content should pass
        ..Default::default()
    };

    let engine = DefaultContextEngine::new(config, session, trajectory, llm);
    engine
        .init_run(
            run_id,
            "anchor".to_string(),
            "Build a REST API".to_string(),
            String::new(),
        )
        .await;

    // History closely related to the goal
    engine
        .append_history(
            run_id,
            "I am building a REST API with endpoints for CRUD operations.".to_string(),
        )
        .await;

    let score = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    assert!(score.score > 0.0);
    assert_eq!(score.consecutive_degraded, 0);
}

#[tokio::test]
async fn test_hygiene_score_off_task() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let llm = Arc::new(MockLlmClient::default());

    let run = make_run();
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let config = ContextEngineConfig {
        hygiene_threshold: 0.99, // Very high threshold to trigger degradation
        ..Default::default()
    };

    let engine = DefaultContextEngine::new(config, session, trajectory, llm);
    engine
        .init_run(
            run_id,
            "anchor".to_string(),
            "Build a REST API".to_string(),
            String::new(),
        )
        .await;

    // Completely unrelated history
    engine
        .append_history(
            run_id,
            "The weather today is sunny and warm in Tokyo.".to_string(),
        )
        .await;

    let score = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    assert_eq!(score.consecutive_degraded, 1);
}

#[tokio::test]
async fn test_todo_recitation_injection() {
    let (engine, run_id) = setup().await;

    // Recitation interval is 3. Advance to turn 3.
    engine.advance_turn(run_id).await; // turn 1
    engine.advance_turn(run_id).await; // turn 2
    engine.advance_turn(run_id).await; // turn 3

    engine.inject_todo_recitation(run_id).await.unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(ctx.history.contains("TODO Recitation (turn 3)"));
    assert!(ctx.history.contains("Set up project"));
}

#[tokio::test]
async fn test_todo_recitation_skipped_when_not_interval() {
    let (engine, run_id) = setup().await;

    engine.advance_turn(run_id).await; // turn 1
    engine.advance_turn(run_id).await; // turn 2

    engine.inject_todo_recitation(run_id).await.unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(!ctx.history.contains("TODO Recitation"));
}

#[tokio::test]
async fn test_nonexistent_run_returns_error() {
    let (engine, _) = setup().await;
    let fake_id = Uuid::new_v4();

    let result = engine.assemble_context(fake_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_reanchor_injected_after_consecutive_degradation() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let llm = Arc::new(MockLlmClient::default());

    let run = make_run();
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let config = ContextEngineConfig {
        hygiene_threshold: 0.99,      // Almost everything degrades
        hygiene_consecutive_limit: 2, // Trigger after 2 consecutive
        ..Default::default()
    };

    let engine = DefaultContextEngine::new(config, session, trajectory.clone(), llm);
    engine
        .init_run(
            run_id,
            "anchor".to_string(),
            "Build a REST API".to_string(),
            String::new(),
        )
        .await;

    engine
        .append_history(run_id, "Unrelated content about weather.".to_string())
        .await;

    // First degraded score
    let s1 = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    assert_eq!(s1.consecutive_degraded, 1);

    // Second degraded score — should trigger re-anchor injection
    let s2 = engine.compute_hygiene_score(run_id, 10).await.unwrap();
    assert_eq!(s2.consecutive_degraded, 2);

    // Verify re-anchor block was injected into history
    let ctx = engine.assemble_context(run_id).await.unwrap();
    assert!(ctx.history.contains("RE-ANCHOR"));
    assert!(ctx.history.contains("Build a REST API"));

    // Verify ReanchorInjected event was emitted
    let events = trajectory
        .query_events(Some(run_id), Some(EventType::ReanchorInjected), None, None)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);

    // After re-anchor, consecutive count should reset — next non-degraded check should be 0
    // (but since our threshold is 0.99, it will still degrade — that's fine,
    // the point is the counter was reset to 0 and starts counting again)
}

#[tokio::test]
async fn test_error_tool_results_protected_from_compaction() {
    let dir = tempfile::TempDir::new().unwrap();
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let llm = Arc::new(MockLlmClient::default());

    let run = make_run();
    let run_id = run.id;
    session.create_run(run).await.unwrap();

    let config = ContextEngineConfig {
        offload_dir: dir.path().to_path_buf(),
        offload_token_threshold: 10,
        ..Default::default()
    };

    let engine = DefaultContextEngine::new(config, session, trajectory, llm);
    engine
        .init_run(
            run_id,
            "anchor".to_string(),
            "goal".to_string(),
            String::new(),
        )
        .await;

    // Push a normal tool result and an error tool result, both large
    let large = "The quick brown fox jumps over the lazy dog. ".repeat(20);
    engine.push_tool_result(run_id, large.clone(), false).await;
    engine
        .push_tool_result(run_id, format!("ERROR: {large}"), true)
        .await;

    engine
        .trigger_compaction(run_id, CompactionStage::Offload)
        .await
        .unwrap();

    let ctx = engine.assemble_context(run_id).await.unwrap();
    // Normal entry should be offloaded
    assert!(ctx.tool_results[0].starts_with("[Offloaded to "));
    // Error entry should be preserved in full
    assert!(ctx.tool_results[1].starts_with("ERROR:"));
    assert!(!ctx.tool_results[1].starts_with("[Offloaded to "));
}
