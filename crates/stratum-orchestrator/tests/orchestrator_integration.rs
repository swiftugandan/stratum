//! Integration tests for the Sub-Agent Orchestrator.

use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;

use stratum_core::{Orchestrator, SessionManager, TaskDispatch};
use stratum_orchestrator::{
    DefaultOrchestrator, OrchestratorConfig, OrchestratorError, RfbmqDispatcher,
};
use stratum_test_utils::mocks::{MockSessionManager, MockTrajectoryStore};
use stratum_types::*;

fn make_parent_run(depth_limit: u32) -> StratumRun {
    StratumRun {
        id: Uuid::new_v4(),
        parent_run_id: None,
        model_ref: "test-model".to_string(),
        trust_level: TrustLevel::Supervised,
        tool_manifest: vec![],
        memory_config: MemoryConfig::default(),
        hitl_policy: HitlPolicy::default(),
        context_budget: ContextBudget::default(),
        spawn_depth_limit: depth_limit,
        state: RunState::Running,
        created_at: Utc::now(),
    }
}

fn make_spawn_config(pattern: SpawnPattern) -> SpawnConfig {
    SpawnConfig {
        pattern,
        model_ref: "child-model".to_string(),
        trust_level: TrustLevel::Sandboxed,
        tool_manifest: vec!["read_file".to_string()],
        task_goal: "Integration test task".to_string(),
        shared_filesystem_scope: None,
    }
}

#[tokio::test]
async fn test_delegate_end_to_end() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        poll_interval: std::time::Duration::from_millis(10),
        await_timeout: std::time::Duration::from_secs(5),
        ..Default::default()
    };
    let orch = DefaultOrchestrator::new(config, Arc::clone(&session), Arc::clone(&trajectory));

    // Create parent
    let parent = make_parent_run(3);
    let parent_id = parent.id;
    session.create_run(parent).await.unwrap();

    // Spawn child
    let child_id = orch
        .spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();

    // Simulate child completing
    session
        .transition_state(child_id, RunState::Completed)
        .await
        .unwrap();

    // Await result
    let result = orch.await_result(child_id).await.unwrap();
    assert_eq!(result.status, RunState::Completed);
    assert_eq!(result.run_id, child_id);

    // Verify events emitted
    let events = trajectory.events.lock().unwrap();
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert!(types.contains(&EventType::SubagentSpawned));
    assert!(types.contains(&EventType::SubagentCompleted));
}

#[tokio::test]
async fn test_pipeline_dependency_chain() {
    let dir = TempDir::new().unwrap();
    let d = RfbmqDispatcher::init(dir.path(), 1000).unwrap();

    // Enqueue 3 tasks in a chain: A -> B -> C
    let id_a = d.enqueue("task A", DispatchOptions::default()).unwrap();
    let id_b = d
        .enqueue(
            "task B",
            DispatchOptions {
                depends_on: vec![id_a.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    let _id_c = d
        .enqueue(
            "task C",
            DispatchOptions {
                depends_on: vec![id_b.clone()],
                ..Default::default()
            },
        )
        .unwrap();

    // Only A should be ready
    let ready = d.list_ready().unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0], id_a);

    // Complete A
    let task_a = d.dequeue().unwrap().unwrap();
    assert_eq!(task_a.body, "task A");
    d.complete(&task_a).unwrap();

    // Now B should be ready (A is done)
    let ready = d.list_ready().unwrap();
    assert!(ready.contains(&id_b));
}

#[tokio::test]
async fn test_parallel_independent_tasks() {
    let dir = TempDir::new().unwrap();
    let d = RfbmqDispatcher::init(dir.path(), 1000).unwrap();

    // Enqueue 3 independent tasks
    let id_a = d.enqueue("task A", DispatchOptions::default()).unwrap();
    let id_b = d.enqueue("task B", DispatchOptions::default()).unwrap();
    let id_c = d.enqueue("task C", DispatchOptions::default()).unwrap();

    // All should be ready
    let ready = d.list_ready().unwrap();
    assert_eq!(ready.len(), 3);
    assert!(ready.contains(&id_a));
    assert!(ready.contains(&id_b));
    assert!(ready.contains(&id_c));
}

#[tokio::test]
async fn test_depth_enforcement_multi_gen() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: 3,
        ..Default::default()
    };
    let orch = DefaultOrchestrator::new(config, Arc::clone(&session), Arc::clone(&trajectory));

    // Root (depth 0)
    let root = make_parent_run(5);
    let root_id = root.id;
    session.create_run(root).await.unwrap();

    // Child (depth 1)
    let child_id = orch
        .spawn(root_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();
    assert_eq!(orch.current_depth(child_id).unwrap(), 1);

    // Grandchild (depth 2)
    let grandchild_id = orch
        .spawn(child_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();
    assert_eq!(orch.current_depth(grandchild_id).unwrap(), 2);

    // Great-grandchild (depth 3) — at limit
    let ggchild_id = orch
        .spawn(grandchild_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();
    assert_eq!(orch.current_depth(ggchild_id).unwrap(), 3);

    // Depth 4 — exceeds hard_max_depth=3
    let result = orch
        .spawn(ggchild_id, make_spawn_config(SpawnPattern::Delegate))
        .await;
    assert!(matches!(
        result,
        Err(OrchestratorError::DepthLimitExceeded { .. })
    ));
}

#[tokio::test]
async fn test_queue_topology() {
    let dir = TempDir::new().unwrap();
    let _d = RfbmqDispatcher::init_or_open(dir.path(), 100).unwrap();

    // Verify expected directories exist
    assert!(dir.path().join("pending").exists());
    assert!(dir.path().join("processing").exists());
}

#[tokio::test]
async fn test_event_payload_correctness() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let orch = DefaultOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::clone(&session),
        Arc::clone(&trajectory),
    );

    let parent = make_parent_run(3);
    let parent_id = parent.id;
    session.create_run(parent).await.unwrap();

    let child_id = orch
        .spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();

    let events = trajectory.events.lock().unwrap();
    let spawn_event = events
        .iter()
        .find(|e| e.event_type == EventType::SubagentSpawned)
        .unwrap();

    // Verify payload fields
    let payload = &spawn_event.payload;
    assert_eq!(payload["pattern"], "Delegate");
    assert_eq!(payload["parent_id"], parent_id.to_string());
    assert_eq!(payload["child_id"], child_id.to_string());
    assert_eq!(payload["task_goal"], "Integration test task");
    assert_eq!(payload["depth"], 1);

    // Verify event metadata
    assert_eq!(spawn_event.run_id, child_id);
    assert_eq!(spawn_event.parent_run_id, Some(parent_id));
    assert_eq!(
        spawn_event.stratum_layer,
        StratumLayer::SubAgentOrchestrator
    );
}

#[tokio::test]
async fn test_janitor_separate_flow() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let orch = DefaultOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::clone(&session),
        Arc::clone(&trajectory),
    );

    let parent = make_parent_run(3);
    let parent_id = parent.id;
    session.create_run(parent).await.unwrap();

    // Spawn janitor
    orch.spawn(parent_id, make_spawn_config(SpawnPattern::Janitor))
        .await
        .unwrap();

    // Spawn delegate
    orch.spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();

    let events = trajectory.events.lock().unwrap();
    let janitor_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == EventType::JanitorRunStarted)
        .collect();
    let spawn_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == EventType::SubagentSpawned)
        .collect();

    assert_eq!(janitor_events.len(), 1);
    assert_eq!(spawn_events.len(), 1);
}
