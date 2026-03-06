//! Concurrent sub-agent tests for the DefaultOrchestrator.

use std::sync::Arc;

use stratum_core::{Orchestrator, SessionManager};
use stratum_orchestrator::config::OrchestratorConfig;
use stratum_orchestrator::orchestrator::DefaultOrchestrator;
use stratum_test_utils::mocks::{make_test_run, MockSessionManager, MockTrajectoryStore};
use stratum_types::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_root_run(depth_limit: u32) -> StratumRun {
    StratumRun {
        state: RunState::Running,
        spawn_depth_limit: depth_limit,
        ..make_test_run()
    }
}

fn make_spawn_config(pattern: SpawnPattern) -> SpawnConfig {
    SpawnConfig {
        pattern,
        model_ref: "child".to_string(),
        trust_level: TrustLevel::Sandboxed,
        tool_manifest: vec!["read_file".to_string()],
        task_goal: "task".to_string(),
        shared_filesystem_scope: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Spawn 10 sub-agents from the same parent concurrently using tokio::spawn.
/// Verify all get unique IDs and all are present in the session manager.
#[tokio::test]
async fn test_concurrent_spawn_isolation() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: 20,
        ..Default::default()
    };
    let orch = Arc::new(DefaultOrchestrator::new(
        config,
        Arc::clone(&session),
        Arc::clone(&trajectory),
    ));

    let parent = make_root_run(20);
    let parent_id = parent.id;
    session.create_run(parent).await.unwrap();

    // Spawn 10 sub-agents concurrently
    let mut handles = Vec::new();
    for _ in 0..10 {
        let orch = Arc::clone(&orch);
        let handle = tokio::spawn(async move {
            orch.spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
                .await
                .unwrap()
        });
        handles.push(handle);
    }

    let mut child_ids = Vec::new();
    for handle in handles {
        child_ids.push(handle.await.unwrap());
    }

    // All IDs must be unique
    let mut unique_ids = child_ids.clone();
    unique_ids.sort();
    unique_ids.dedup();
    assert_eq!(unique_ids.len(), 10, "all 10 child IDs must be unique");

    // All children must exist in session manager (parent + 10 children = 11 runs)
    let runs = session.runs.lock().unwrap();
    assert_eq!(runs.len(), 11);
    for child_id in &child_ids {
        assert!(
            runs.iter().any(|r| r.id == *child_id),
            "child {child_id} must exist in session"
        );
    }
}

/// With hard_max_depth=2, spawn a child from root, then concurrently try to
/// spawn 5 grandchildren from the child. All should succeed at depth 2. Then
/// try to spawn from any grandchild — should fail with DepthLimitExceeded.
#[tokio::test]
async fn test_concurrent_spawn_depth_limit() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: 2,
        poll_interval: std::time::Duration::from_millis(10),
        await_timeout: std::time::Duration::from_secs(5),
        ..Default::default()
    };
    let orch = Arc::new(DefaultOrchestrator::new(
        config,
        Arc::clone(&session),
        Arc::clone(&trajectory),
    ));

    // Root at depth 0
    let root = make_root_run(5);
    let root_id = root.id;
    session.create_run(root).await.unwrap();

    // Child at depth 1
    let child_id = orch
        .spawn(root_id, make_spawn_config(SpawnPattern::Delegate))
        .await
        .unwrap();
    assert_eq!(orch.current_depth(child_id).unwrap(), 1);

    // Concurrently spawn 5 grandchildren from the child (depth 2, at limit)
    let mut handles = Vec::new();
    for _ in 0..5 {
        let orch = Arc::clone(&orch);
        let handle = tokio::spawn(async move {
            orch.spawn(child_id, make_spawn_config(SpawnPattern::Delegate))
                .await
                .unwrap()
        });
        handles.push(handle);
    }

    let mut grandchild_ids = Vec::new();
    for handle in handles {
        grandchild_ids.push(handle.await.unwrap());
    }

    assert_eq!(
        grandchild_ids.len(),
        5,
        "all 5 grandchildren should succeed"
    );

    // Verify all grandchildren are at depth 2
    for gc_id in &grandchild_ids {
        assert_eq!(orch.current_depth(*gc_id).unwrap(), 2);
    }

    // Try to spawn from any grandchild — should fail (depth 2 >= hard_max_depth 2)
    let result = orch
        .spawn(grandchild_ids[0], make_spawn_config(SpawnPattern::Delegate))
        .await;
    assert!(
        matches!(
            result,
            Err(stratum_orchestrator::OrchestratorError::DepthLimitExceeded { .. })
        ),
        "spawning from grandchild should exceed depth limit"
    );
}

/// Create 3 root runs. Spawn sub-agents from each in parallel. Verify correct
/// parent-child relationships.
#[tokio::test]
async fn test_parallel_sub_agents_different_parents() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: 5,
        ..Default::default()
    };
    let orch = Arc::new(DefaultOrchestrator::new(
        config,
        Arc::clone(&session),
        Arc::clone(&trajectory),
    ));

    // Create 3 root runs
    let mut root_ids = Vec::new();
    for _ in 0..3 {
        let root = make_root_run(5);
        let root_id = root.id;
        session.create_run(root).await.unwrap();
        root_ids.push(root_id);
    }

    // Spawn 2 children from each root in parallel (6 spawns total)
    let mut handles = Vec::new();
    for &root_id in &root_ids {
        for _ in 0..2 {
            let orch = Arc::clone(&orch);
            let handle = tokio::spawn(async move {
                let child_id = orch
                    .spawn(root_id, make_spawn_config(SpawnPattern::Delegate))
                    .await
                    .unwrap();
                (root_id, child_id)
            });
            handles.push(handle);
        }
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.unwrap());
    }

    assert_eq!(results.len(), 6, "6 children spawned total");

    // Verify parent-child relationships
    let runs = session.runs.lock().unwrap();
    for (parent_id, child_id) in &results {
        let child = runs.iter().find(|r| r.id == *child_id).unwrap();
        assert_eq!(
            child.parent_run_id,
            Some(*parent_id),
            "child {child_id} must have parent {parent_id}"
        );
    }

    // Each root should have exactly 2 children
    for root_id in &root_ids {
        let children_count = results.iter().filter(|(p, _)| p == root_id).count();
        assert_eq!(
            children_count, 2,
            "root {root_id} should have exactly 2 children"
        );
    }
}

/// Spawn multiple sub-agents, transition them to Completed in parallel,
/// await_result on each. All should succeed.
#[tokio::test]
async fn test_await_result_concurrent() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: 10,
        poll_interval: std::time::Duration::from_millis(5),
        await_timeout: std::time::Duration::from_secs(5),
        ..Default::default()
    };
    let orch = Arc::new(DefaultOrchestrator::new(
        config,
        Arc::clone(&session),
        Arc::clone(&trajectory),
    ));

    let parent = make_root_run(10);
    let parent_id = parent.id;
    session.create_run(parent).await.unwrap();

    // Spawn 5 sub-agents
    let mut child_ids = Vec::new();
    for _ in 0..5 {
        let child_id = orch
            .spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();
        child_ids.push(child_id);
    }

    // Transition all children to Completed in parallel
    let mut transition_handles = Vec::new();
    for &child_id in &child_ids {
        let session = Arc::clone(&session);
        let handle = tokio::spawn(async move {
            session
                .transition_state(child_id, RunState::Completed)
                .await
                .unwrap();
        });
        transition_handles.push(handle);
    }
    for handle in transition_handles {
        handle.await.unwrap();
    }

    // Await results concurrently
    let mut await_handles = Vec::new();
    for &child_id in &child_ids {
        let orch = Arc::clone(&orch);
        let handle = tokio::spawn(async move { orch.await_result(child_id).await.unwrap() });
        await_handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in await_handles {
        results.push(handle.await.unwrap());
    }

    assert_eq!(results.len(), 5);
    for result in &results {
        assert_eq!(result.status, RunState::Completed);
        assert!(child_ids.contains(&result.run_id));
    }
}

/// Create a chain: root -> child -> grandchild -> ... up to hard_max_depth.
/// Verify that the final spawn at the limit fails.
#[tokio::test]
async fn test_max_depth_chain() {
    let hard_max: u32 = 4;
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: hard_max,
        ..Default::default()
    };
    let orch = DefaultOrchestrator::new(config, Arc::clone(&session), Arc::clone(&trajectory));

    // Root at depth 0
    let root = make_root_run(hard_max + 10); // per-run limit is high; hard_max controls
    let mut current_id = root.id;
    session.create_run(root).await.unwrap();

    // Build chain up to hard_max_depth
    for expected_depth in 1..=hard_max {
        let child_id = orch
            .spawn(current_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();
        assert_eq!(
            orch.current_depth(child_id).unwrap(),
            expected_depth,
            "depth should be {expected_depth}"
        );
        current_id = child_id;
    }

    // The next spawn should fail — current_id is at hard_max_depth
    let result = orch
        .spawn(current_id, make_spawn_config(SpawnPattern::Delegate))
        .await;
    assert!(
        matches!(
            result,
            Err(stratum_orchestrator::OrchestratorError::DepthLimitExceeded { .. })
        ),
        "spawn beyond hard_max_depth should fail"
    );

    // Total runs: 1 root + hard_max children
    let runs = session.runs.lock().unwrap();
    assert_eq!(runs.len(), (hard_max + 1) as usize);
}

/// Spawn Delegate, Pipeline, Parallel, and Janitor patterns from the same
/// parent concurrently. Verify the correct event types are emitted.
#[tokio::test]
async fn test_concurrent_spawn_different_patterns() {
    let session = Arc::new(MockSessionManager::default());
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let config = OrchestratorConfig {
        hard_max_depth: 10,
        ..Default::default()
    };
    let orch = Arc::new(DefaultOrchestrator::new(
        config,
        Arc::clone(&session),
        Arc::clone(&trajectory),
    ));

    let parent = make_root_run(10);
    let parent_id = parent.id;
    session.create_run(parent).await.unwrap();

    let patterns = vec![
        SpawnPattern::Delegate,
        SpawnPattern::Pipeline,
        SpawnPattern::Parallel,
        SpawnPattern::Janitor,
    ];

    // Spawn all 4 patterns concurrently
    let mut handles = Vec::new();
    for pattern in patterns {
        let orch = Arc::clone(&orch);
        let handle = tokio::spawn(async move {
            orch.spawn(parent_id, make_spawn_config(pattern))
                .await
                .unwrap()
        });
        handles.push(handle);
    }

    let mut child_ids = Vec::new();
    for handle in handles {
        child_ids.push(handle.await.unwrap());
    }

    assert_eq!(child_ids.len(), 4);

    // Check emitted events
    let events = trajectory.events.lock().unwrap();

    // Delegate, Pipeline, and Parallel emit SubagentSpawned
    let subagent_spawned_count = events
        .iter()
        .filter(|e| e.event_type == EventType::SubagentSpawned)
        .count();
    assert_eq!(
        subagent_spawned_count, 3,
        "Delegate, Pipeline, and Parallel should emit SubagentSpawned"
    );

    // Janitor emits JanitorRunStarted
    let janitor_count = events
        .iter()
        .filter(|e| e.event_type == EventType::JanitorRunStarted)
        .count();
    assert_eq!(janitor_count, 1, "Janitor should emit JanitorRunStarted");

    // All events should reference the parent
    for event in events.iter() {
        assert_eq!(event.parent_run_id, Some(parent_id));
        assert_eq!(event.stratum_layer, StratumLayer::SubAgentOrchestrator);
    }
}
