//! Performance benchmark tests for stratum-adapters.
//!
//! These are timing-based tests that assert reasonable upper bounds,
//! not criterion microbenchmarks.

use std::sync::Arc;
use std::time::Instant;

use stratum_adapters::session::SqliteSessionManager;
use stratum_adapters::trajectory::SqliteTrajectoryStore;
use stratum_core::{SessionManager, TrajectoryStore};
use stratum_test_utils::mocks::{make_test_checkpoint, make_test_event, make_test_run};
use stratum_types::*;
use uuid::Uuid;

#[tokio::test]
async fn test_event_emission_throughput() {
    let store = SqliteTrajectoryStore::in_memory().unwrap();
    let run_id = Uuid::new_v4();

    let start = Instant::now();
    for _ in 0..10_000 {
        store
            .emit_event(make_test_event(run_id, EventType::ToolCalled))
            .await
            .unwrap();
    }
    let elapsed = start.elapsed();

    let throughput = 10_000.0 / elapsed.as_secs_f64();
    println!(
        "Event emission throughput: {:.0} events/sec ({:.2?} total for 10,000 events)",
        throughput, elapsed
    );

    assert!(
        elapsed.as_secs() < 30,
        "10,000 event emissions took {:?}, expected < 30s",
        elapsed
    );
}

#[tokio::test]
async fn test_event_query_latency() {
    let store = SqliteTrajectoryStore::in_memory().unwrap();
    let run_id = Uuid::new_v4();

    // Emit 1000 events
    for _ in 0..1_000 {
        store
            .emit_event(make_test_event(run_id, EventType::ToolCalled))
            .await
            .unwrap();
    }

    // Time the query
    let start = Instant::now();
    let events = store
        .query_events(Some(run_id), None, None, None)
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(events.len(), 1_000);
    println!(
        "Query latency for 1,000 events: {:.2?} ({:.0} events/sec)",
        elapsed,
        1_000.0 / elapsed.as_secs_f64()
    );

    assert!(
        elapsed.as_secs() < 1,
        "Querying 1,000 events took {:?}, expected < 1s",
        elapsed
    );
}

#[tokio::test]
async fn test_concurrent_event_emission() {
    let store = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let run_id = Uuid::new_v4();

    let start = Instant::now();
    let mut handles = Vec::new();

    for _ in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            for _ in 0..1_000 {
                store
                    .emit_event(make_test_event(run_id, EventType::ToolCalled))
                    .await
                    .unwrap();
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
    let elapsed = start.elapsed();

    let all = store
        .query_events(Some(run_id), None, None, None)
        .await
        .unwrap();

    let throughput = 10_000.0 / elapsed.as_secs_f64();
    println!(
        "Concurrent emission: {:.0} events/sec ({:.2?} total for 10,000 events across 10 tasks)",
        throughput, elapsed
    );

    assert_eq!(
        all.len(),
        10_000,
        "Expected 10,000 events, got {}",
        all.len()
    );
    assert!(
        elapsed.as_secs() < 30,
        "Concurrent 10,000 event emissions took {:?}, expected < 30s",
        elapsed
    );
}

#[tokio::test]
async fn test_session_create_throughput() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj).unwrap();

    let start = Instant::now();
    for _ in 0..1_000 {
        let run = make_test_run();
        session.create_run(run).await.unwrap();
    }
    let elapsed = start.elapsed();

    let throughput = 1_000.0 / elapsed.as_secs_f64();
    println!(
        "Session create throughput: {:.0} runs/sec ({:.2?} total for 1,000 runs)",
        throughput, elapsed
    );

    assert!(
        elapsed.as_secs() < 30,
        "Creating 1,000 runs took {:?}, expected < 30s",
        elapsed
    );
}

#[tokio::test]
async fn test_checkpoint_write_throughput() {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = SqliteSessionManager::in_memory(traj).unwrap();

    // Create a run and transition to Checkpointed
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

    let start = Instant::now();
    for _ in 0..100 {
        let cp = make_test_checkpoint(id, "checkpoint context");
        session.checkpoint(&cp).await.unwrap();
    }
    let elapsed = start.elapsed();

    let throughput = 100.0 / elapsed.as_secs_f64();
    println!(
        "Checkpoint write throughput: {:.0} checkpoints/sec ({:.2?} total for 100 checkpoints)",
        throughput, elapsed
    );

    assert!(
        elapsed.as_secs() < 30,
        "Writing 100 checkpoints took {:?}, expected < 30s",
        elapsed
    );
}
