//! Crash recovery and HITL durability integration tests.
//!
//! These tests verify that state persisted in file-based SQLite databases
//! survives simulated process restarts (drop + reopen from same path).

use std::sync::Arc;

use stratum_adapters::hitl::controller::SqliteHitlController;
use stratum_adapters::hitl::notifier::StdoutNotifier;
use stratum_adapters::session::SqliteSessionManager;
use stratum_adapters::trajectory::SqliteTrajectoryStore;
use stratum_adapters::AdapterError;
use stratum_core::{HitlController, Notifier, SessionManager, TrajectoryStore};
use stratum_test_utils::mocks::{make_test_checkpoint, make_test_gate, make_test_run};
use stratum_types::*;
use tempfile::TempDir;

/// Create a file-based trajectory store + session manager pair.
fn open_session(dir: &TempDir) -> (Arc<SqliteTrajectoryStore>, Arc<SqliteSessionManager>) {
    let traj_path = dir.path().join("trajectory.db");
    let session_path = dir.path().join("session.db");

    let traj = Arc::new(SqliteTrajectoryStore::new(traj_path.to_str().unwrap()).unwrap());
    let session = Arc::new(
        SqliteSessionManager::new(
            session_path.to_str().unwrap(),
            traj.clone() as Arc<dyn TrajectoryStore<Error = AdapterError>>,
        )
        .unwrap(),
    );
    (traj, session)
}

/// Create a file-based HITL controller that shares the given trajectory + session stores.
fn open_hitl(
    dir: &TempDir,
    traj: Arc<SqliteTrajectoryStore>,
    session: Arc<SqliteSessionManager>,
) -> SqliteHitlController {
    let hitl_path = dir.path().join("hitl.db");
    let notifier: Arc<dyn Notifier<Error = AdapterError>> = Arc::new(StdoutNotifier);
    SqliteHitlController::new(
        hitl_path.to_str().unwrap(),
        traj as Arc<dyn TrajectoryStore<Error = AdapterError>>,
        session as Arc<dyn SessionManager<Error = AdapterError>>,
        notifier,
    )
    .unwrap()
}

/// Create a run and advance it to Running.
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
// Crash Recovery Tests
// ===========================================================================

#[tokio::test]
async fn test_checkpoint_survives_file_db_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);
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

    let cp = make_test_checkpoint(id, "Working on step 2 of 5");
    let cp_id = cp.id;
    session.checkpoint(&cp).await.unwrap();

    // Drop everything — simulates crash / process exit
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (_traj2, session2) = open_session(&dir);

    let loaded = session2.get_latest_checkpoint(id).await.unwrap();
    assert!(loaded.is_some(), "checkpoint should survive restart");
    let loaded = loaded.unwrap();
    assert_eq!(loaded.id, cp_id);
    assert_eq!(loaded.run_id, id);
    assert_eq!(loaded.context_summary, "Working on step 2 of 5");
    assert_eq!(loaded.task_manifest.goal, "Test task");
}

#[tokio::test]
async fn test_run_state_persisted_across_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);
    let run = make_test_run();
    let id = run.id;
    session.create_run(run).await.unwrap();
    session
        .transition_state(id, RunState::Running)
        .await
        .unwrap();

    // Drop — simulates crash
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (_traj2, session2) = open_session(&dir);

    let loaded = session2.get_run(id).await.unwrap();
    assert!(loaded.is_some(), "run should survive restart");
    assert_eq!(loaded.unwrap().state, RunState::Running);
}

#[tokio::test]
async fn test_resume_after_simulated_crash() {
    let dir = TempDir::new().unwrap();

    // --- First "process": create run, checkpoint ---
    let (traj, session) = open_session(&dir);
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

    let cp = make_test_checkpoint(id, "mid-flight checkpoint");
    session.checkpoint(&cp).await.unwrap();

    // Crash
    drop(session);
    drop(traj);

    // --- Second "process": resume ---
    let (_traj2, session2) = open_session(&dir);

    // State should be Checkpointed (as left before crash)
    let run_before = session2.get_run(id).await.unwrap().unwrap();
    assert_eq!(run_before.state, RunState::Checkpointed);

    // Resume transitions Checkpointed -> Resuming
    let resumed = session2.resume(id).await.unwrap();
    assert_eq!(resumed.state, RunState::Resuming);

    // Resuming -> Running
    session2
        .transition_state(id, RunState::Running)
        .await
        .unwrap();
    let run_after = session2.get_run(id).await.unwrap().unwrap();
    assert_eq!(run_after.state, RunState::Running);
}

#[tokio::test]
async fn test_multiple_checkpoints_latest_survives_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);
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

    // First checkpoint
    let cp1 = make_test_checkpoint(id, "step 1");
    session.checkpoint(&cp1).await.unwrap();

    // Small delay so timestamps are distinct
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Second checkpoint (latest)
    let cp2 = make_test_checkpoint(id, "step 2");
    let cp2_id = cp2.id;
    session.checkpoint(&cp2).await.unwrap();

    // Crash
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (_traj2, session2) = open_session(&dir);

    let latest = session2.get_latest_checkpoint(id).await.unwrap();
    assert!(latest.is_some(), "latest checkpoint should survive restart");
    let latest = latest.unwrap();
    assert_eq!(latest.id, cp2_id);
    assert_eq!(latest.context_summary, "step 2");
}

#[tokio::test]
async fn test_trajectory_events_survive_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);
    let run = make_test_run();
    let id = run.id;

    // create_run emits RunCreated; transition emits RunStarted
    session.create_run(run).await.unwrap();
    session
        .transition_state(id, RunState::Running)
        .await
        .unwrap();

    // Also emit a custom event directly on the trajectory store
    let custom_event = TrajectoryEvent::new(
        id,
        None,
        EventType::ToolCalled,
        StratumLayer::ToolGateway,
        serde_json::json!({"tool": "read_file"}),
    );
    traj.emit_event(custom_event).await.unwrap();

    // Crash
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (traj2, _session2) = open_session(&dir);

    // Query all events for the run
    let events = traj2
        .query_events(Some(id), None, None, None)
        .await
        .unwrap();
    // At minimum: RunCreated + RunStarted + ToolCalled = 3
    assert!(
        events.len() >= 3,
        "expected at least 3 events, got {}",
        events.len()
    );

    // Verify specific event types survived
    let run_created = traj2
        .query_events(Some(id), Some(EventType::RunCreated), None, None)
        .await
        .unwrap();
    assert_eq!(run_created.len(), 1);

    let tool_called = traj2
        .query_events(Some(id), Some(EventType::ToolCalled), None, None)
        .await
        .unwrap();
    assert_eq!(tool_called.len(), 1);
    assert_eq!(
        tool_called[0].payload["tool"].as_str().unwrap(),
        "read_file"
    );
}

// ===========================================================================
// HITL Durability Tests
// ===========================================================================

#[tokio::test]
async fn test_hitl_gate_survives_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);
    let id = create_running_run(&session).await;

    let ctrl = open_hitl(&dir, traj.clone(), session.clone());
    let gate = make_test_gate(id, "gate-durable", GateCategory::Destructive);
    ctrl.open_gate(gate).await.unwrap();

    // Verify paused
    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Paused);

    // Crash — drop everything
    drop(ctrl);
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (traj2, session2) = open_session(&dir);
    let ctrl2 = open_hitl(&dir, traj2, session2.clone());

    // Gate should still be pending
    let pending = ctrl2.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 1, "gate should survive restart");
    assert_eq!(pending[0].id, "gate-durable");
    assert_eq!(pending[0].run_id, id);
    assert_eq!(pending[0].gate_category, GateCategory::Destructive);

    // Run should still be Paused
    let run = session2.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Paused);
}

#[tokio::test]
async fn test_hitl_decision_after_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);
    let id = create_running_run(&session).await;

    let ctrl = open_hitl(&dir, traj.clone(), session.clone());
    ctrl.open_gate(make_test_gate(id, "gate-decide", GateCategory::Ambiguity))
        .await
        .unwrap();

    // Crash
    drop(ctrl);
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (traj2, session2) = open_session(&dir);
    let ctrl2 = open_hitl(&dir, traj2, session2.clone());

    // Record decision on the new controller instance
    ctrl2
        .record_decision(id, HitlDecision::Approve)
        .await
        .unwrap();

    // Gate should no longer be pending
    let pending = ctrl2.pending_gates().await.unwrap();
    assert!(pending.is_empty(), "gate should be resolved after decision");

    // Decision should be queryable
    let decision = ctrl2.get_decision(id, "gate-decide").await.unwrap();
    assert!(
        matches!(decision, Some(HitlDecision::Approve)),
        "decision should be Approve"
    );

    // Run should be back to Running (Paused -> Running on Approve)
    let run = session2.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Running);
}

#[tokio::test]
async fn test_multiple_gates_survive_restart() {
    let dir = TempDir::new().unwrap();

    // --- First "process" ---
    let (traj, session) = open_session(&dir);

    // Create three runs, each with a gate
    let id1 = create_running_run(&session).await;
    let id2 = create_running_run(&session).await;
    let id3 = create_running_run(&session).await;

    let ctrl = open_hitl(&dir, traj.clone(), session.clone());

    ctrl.open_gate(make_test_gate(id1, "gate-1", GateCategory::Destructive))
        .await
        .unwrap();
    ctrl.open_gate(make_test_gate(id2, "gate-2", GateCategory::Irreversible))
        .await
        .unwrap();
    ctrl.open_gate(make_test_gate(id3, "gate-3", GateCategory::TrustEscalation))
        .await
        .unwrap();

    // Crash
    drop(ctrl);
    drop(session);
    drop(traj);

    // --- Second "process" ---
    let (traj2, session2) = open_session(&dir);
    let ctrl2 = open_hitl(&dir, traj2, session2.clone());

    // All three gates should be pending
    let pending = ctrl2.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 3, "all 3 gates should survive restart");

    let pending_ids: Vec<&str> = pending.iter().map(|g| g.id.as_str()).collect();
    assert!(pending_ids.contains(&"gate-1"));
    assert!(pending_ids.contains(&"gate-2"));
    assert!(pending_ids.contains(&"gate-3"));

    // Verify each gate's run_id and category are correct
    for gate in &pending {
        match gate.id.as_str() {
            "gate-1" => {
                assert_eq!(gate.run_id, id1);
                assert_eq!(gate.gate_category, GateCategory::Destructive);
            }
            "gate-2" => {
                assert_eq!(gate.run_id, id2);
                assert_eq!(gate.gate_category, GateCategory::Irreversible);
            }
            "gate-3" => {
                assert_eq!(gate.run_id, id3);
                assert_eq!(gate.gate_category, GateCategory::TrustEscalation);
            }
            _ => panic!("unexpected gate id: {}", gate.id),
        }
    }

    // All runs should be Paused
    for &run_id in &[id1, id2, id3] {
        let run = session2.get_run(run_id).await.unwrap().unwrap();
        assert_eq!(run.state, RunState::Paused);
    }
}
