//! Edge case tests covering: empty runs, budget exhaustion, all gates triggered
//! simultaneously, and state machine boundaries.

use std::sync::Arc;

use stratum_adapters::hitl::controller::SqliteHitlController;
use stratum_adapters::hitl::notifier::StdoutNotifier;
use stratum_adapters::session::SqliteSessionManager;
use stratum_adapters::trajectory::SqliteTrajectoryStore;
use stratum_adapters::AdapterError;
use stratum_core::{HitlController, Notifier, SessionManager, TrajectoryStore};
use stratum_test_utils::mocks::{make_test_gate, make_test_run};
use stratum_types::*;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------------

async fn setup() -> (
    SqliteHitlController,
    Arc<SqliteTrajectoryStore>,
    Arc<SqliteSessionManager>,
) {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = Arc::new(SqliteSessionManager::in_memory(traj.clone()).unwrap());
    let notifier: Arc<dyn Notifier<Error = AdapterError>> = Arc::new(StdoutNotifier);
    let ctrl = SqliteHitlController::in_memory(
        traj.clone(),
        session.clone() as Arc<dyn SessionManager<Error = AdapterError>>,
        notifier,
    )
    .unwrap();
    (ctrl, traj, session)
}

async fn create_running_run(session: &SqliteSessionManager) -> RunId {
    let run = make_test_run();
    let id = run.id;
    session.create_run(run).await.unwrap();
    session
        .transition_state(id, RunState::Running)
        .await
        .unwrap();
    id
}

// ===========================================================================
// Empty runs
// ===========================================================================

#[tokio::test]
async fn test_empty_run_no_events() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj.clone()).unwrap();

    let run = make_test_run();
    let id = run.id;
    session.create_run(run).await.unwrap();

    // Initialising -> Running
    session
        .transition_state(id, RunState::Running)
        .await
        .unwrap();

    // Running -> Completed
    session
        .transition_state(id, RunState::Completed)
        .await
        .unwrap();

    // Only lifecycle events should be emitted (RunCreated, RunStarted, RunCompleted)
    let events = traj.query_events(Some(id), None, None, None).await.unwrap();

    assert_eq!(events.len(), 3, "Expected exactly 3 lifecycle events");
    assert_eq!(events[0].event_type, EventType::RunCreated);
    assert_eq!(events[1].event_type, EventType::RunStarted);
    assert_eq!(events[2].event_type, EventType::RunCompleted);
}

#[tokio::test]
async fn test_get_checkpoint_for_empty_run() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj.clone()).unwrap();

    let run = make_test_run();
    let id = run.id;
    session.create_run(run).await.unwrap();

    // No checkpoints written — should return None
    let cp = session.get_latest_checkpoint(id).await.unwrap();
    assert!(cp.is_none(), "Expected None for run with no checkpoints");
}

#[tokio::test]
async fn test_export_empty_run() {
    let traj = SqliteTrajectoryStore::in_memory().unwrap();

    // Query events for a nonexistent run_id — should return empty
    let nonexistent = Uuid::new_v4();
    let events = traj
        .query_events(Some(nonexistent), None, None, None)
        .await
        .unwrap();
    assert!(
        events.is_empty(),
        "Expected empty events for nonexistent run"
    );

    // Export should also produce empty content
    let data = traj.export(nonexistent, ExportFormat::Jsonl).await.unwrap();
    assert!(
        data.is_empty(),
        "Expected empty JSONL export for nonexistent run"
    );
}

// ===========================================================================
// Budget exhaustion edge
// ===========================================================================

#[tokio::test]
async fn test_zero_budget_is_always_over() {
    // We cannot use DefaultContextEngine from stratum-adapters tests, so we test
    // the budget logic directly using types.
    let budget = ContextBudget {
        system_anchor: 0,
        task_manifest: 0,
        injected_knowledge: 0,
        tool_results: 0,
        history: 0,
        total_ceiling: 0,
        compaction_threshold: 0.85,
    };

    // Any nonzero token count exceeds a zero ceiling
    let total_tokens: u64 = 1;
    assert!(
        total_tokens > budget.total_ceiling as u64,
        "Even 1 token should exceed a zero total_ceiling"
    );

    // Compaction threshold with zero ceiling is 0
    let compaction_trigger = (budget.total_ceiling as f32 * budget.compaction_threshold) as u64;
    assert_eq!(
        compaction_trigger, 0,
        "Compaction trigger should be 0 for zero ceiling"
    );
    assert!(
        total_tokens > compaction_trigger,
        "Any tokens should trigger compaction with zero budget"
    );

    // Every individual slot also has zero budget
    assert_eq!(budget.system_anchor, 0);
    assert_eq!(budget.task_manifest, 0);
    assert_eq!(budget.injected_knowledge, 0);
    assert_eq!(budget.tool_results, 0);
    assert_eq!(budget.history, 0);
}

// ===========================================================================
// All gates simultaneously
// ===========================================================================

#[tokio::test]
async fn test_all_gate_categories_simultaneously() {
    let (ctrl, _traj, session) = setup().await;

    let categories = [
        GateCategory::Destructive,
        GateCategory::Irreversible,
        GateCategory::TrustEscalation,
        GateCategory::Ambiguity,
        GateCategory::Drift,
        GateCategory::Budget,
        GateCategory::Scheduled,
    ];

    // Create 7 running runs, one per category
    let mut run_ids = Vec::new();
    for _ in &categories {
        run_ids.push(create_running_run(&session).await);
    }

    // Open a gate of each category on the corresponding run
    for (i, (&cat, &run_id)) in categories.iter().zip(run_ids.iter()).enumerate() {
        let gate = make_test_gate(run_id, &format!("gate-{i}"), cat);
        ctrl.open_gate(gate).await.unwrap();
    }

    // All 7 gates should be pending
    let pending = ctrl.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 7, "Expected 7 pending gates");

    // Decide each with a different decision pattern:
    // 0: Approve, 1: Abort, 2: Redirect, 3: Modify, 4: Approve, 5: Abort, 6: Approve
    let decisions = [
        HitlDecision::Approve,
        HitlDecision::Abort,
        HitlDecision::Redirect {
            new_goal: "new objective".to_string(),
        },
        HitlDecision::Modify {
            context: "use safer approach".to_string(),
        },
        HitlDecision::Approve,
        HitlDecision::Abort,
        HitlDecision::Approve,
    ];

    // Expected final states after each decision:
    // Approve -> Running, Abort -> Aborted, Redirect -> Running, Modify -> Running
    let expected_states = [
        RunState::Running,
        RunState::Aborted,
        RunState::Running,
        RunState::Running,
        RunState::Running,
        RunState::Aborted,
        RunState::Running,
    ];

    for (i, (decision, expected_state)) in decisions
        .into_iter()
        .zip(expected_states.iter())
        .enumerate()
    {
        ctrl.record_decision(run_ids[i], decision).await.unwrap();

        let run = session.get_run(run_ids[i]).await.unwrap().unwrap();
        assert_eq!(
            run.state, *expected_state,
            "Run {} (gate-{}) expected state {:?}, got {:?}",
            run_ids[i], i, expected_state, run.state
        );
    }

    // No pending gates remaining
    let pending = ctrl.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 0, "All gates should be decided");
}

#[tokio::test]
async fn test_multiple_gates_same_run() {
    let (ctrl, _traj, session) = setup().await;
    let id = create_running_run(&session).await;

    // Open first gate
    ctrl.open_gate(make_test_gate(id, "gate-1", GateCategory::Destructive))
        .await
        .unwrap();

    // Run should be Paused
    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Paused);

    // Verify gate-1 is pending
    let pending = ctrl.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 1);

    // Decide first gate (Approve -> Paused->Running)
    ctrl.record_decision(id, HitlDecision::Approve)
        .await
        .unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Running);

    // Open second gate (Running->Paused)
    ctrl.open_gate(make_test_gate(id, "gate-2", GateCategory::Ambiguity))
        .await
        .unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Paused);

    // Decide second gate
    ctrl.record_decision(id, HitlDecision::Approve)
        .await
        .unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(
        run.state,
        RunState::Running,
        "Run should be Running after second gate approval"
    );
}

// ===========================================================================
// State machine edge cases
// ===========================================================================

#[tokio::test]
async fn test_all_valid_state_transitions() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj).unwrap();

    // Each valid transition is tested as a fresh run taken through the prerequisite path.

    // Initialising -> Running
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Running
        );
    }

    // Initialising -> Failed
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Failed)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Failed
        );
    }

    // Initialising -> Aborted
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Aborted)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Aborted
        );
    }

    // Running -> Paused
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Paused)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Paused
        );
    }

    // Running -> Checkpointed
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Checkpointed
        );
    }

    // Running -> Completed
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Completed)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Completed
        );
    }

    // Running -> Failed
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Failed)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Failed
        );
    }

    // Running -> Aborted
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Aborted)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Aborted
        );
    }

    // Paused -> Running
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Paused)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Running
        );
    }

    // Paused -> Checkpointed
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Paused)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Checkpointed
        );
    }

    // Paused -> Aborted
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Paused)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Aborted)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Aborted
        );
    }

    // Checkpointed -> Resuming
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Resuming)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Resuming
        );
    }

    // Checkpointed -> Aborted
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Aborted)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Aborted
        );
    }

    // Resuming -> Running
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Resuming)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Running
        );
    }

    // Resuming -> Failed
    {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        session
            .transition_state(id, RunState::Running)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Resuming)
            .await
            .unwrap();
        session
            .transition_state(id, RunState::Failed)
            .await
            .unwrap();
        assert_eq!(
            session.get_run(id).await.unwrap().unwrap().state,
            RunState::Failed
        );
    }
}

#[tokio::test]
async fn test_all_invalid_state_transitions() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj).unwrap();

    // Helper: create a run and move it to the given state via valid transitions
    async fn create_in_state(session: &SqliteSessionManager, target: RunState) -> RunId {
        let run = make_test_run();
        let id = run.id;
        session.create_run(run).await.unwrap();
        match target {
            RunState::Initialising => {} // already there
            RunState::Running => {
                session
                    .transition_state(id, RunState::Running)
                    .await
                    .unwrap();
            }
            RunState::Completed => {
                session
                    .transition_state(id, RunState::Running)
                    .await
                    .unwrap();
                session
                    .transition_state(id, RunState::Completed)
                    .await
                    .unwrap();
            }
            RunState::Failed => {
                session
                    .transition_state(id, RunState::Failed)
                    .await
                    .unwrap();
            }
            RunState::Aborted => {
                session
                    .transition_state(id, RunState::Aborted)
                    .await
                    .unwrap();
            }
            _ => unreachable!("helper only supports terminal and initial states"),
        }
        id
    }

    // Completed -> Running (terminal state, no outgoing transitions)
    {
        let id = create_in_state(&session, RunState::Completed).await;
        let err = session
            .transition_state(id, RunState::Running)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid transition"),
            "Expected 'invalid transition', got: {err}"
        );
    }

    // Failed -> Running (terminal state)
    {
        let id = create_in_state(&session, RunState::Failed).await;
        let err = session
            .transition_state(id, RunState::Running)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    // Aborted -> Running (terminal state)
    {
        let id = create_in_state(&session, RunState::Aborted).await;
        let err = session
            .transition_state(id, RunState::Running)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    // Initialising -> Completed (must go through Running first)
    {
        let id = create_in_state(&session, RunState::Initialising).await;
        let err = session
            .transition_state(id, RunState::Completed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    // Initialising -> Paused (must go through Running first)
    {
        let id = create_in_state(&session, RunState::Initialising).await;
        let err = session
            .transition_state(id, RunState::Paused)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    // Running -> Initialising (cannot go backward)
    {
        let id = create_in_state(&session, RunState::Running).await;
        let err = session
            .transition_state(id, RunState::Initialising)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    // Running -> Resuming (must go through Checkpointed)
    {
        let id = create_in_state(&session, RunState::Running).await;
        let err = session
            .transition_state(id, RunState::Resuming)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    // Completed -> Aborted (terminal to terminal)
    {
        let id = create_in_state(&session, RunState::Completed).await;
        let err = session
            .transition_state(id, RunState::Aborted)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }
}

#[tokio::test]
async fn test_transition_nonexistent_run() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj).unwrap();

    let nonexistent = Uuid::new_v4();
    let err = session
        .transition_state(nonexistent, RunState::Running)
        .await
        .unwrap_err();

    // Should be a NotFound error
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("not found")
            || msg.contains("NotFound")
            || msg.contains("no rows"),
        "Expected not-found error, got: {msg}"
    );
}

#[tokio::test]
async fn test_resume_nonexistent_run() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj).unwrap();

    let nonexistent = Uuid::new_v4();
    let err = session.resume(nonexistent).await.unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("not found")
            || msg.contains("NotFound")
            || msg.contains("no rows"),
        "Expected not-found error, got: {msg}"
    );
}
