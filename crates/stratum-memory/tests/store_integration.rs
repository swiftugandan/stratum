//! Integration tests for DefaultMemoryStore.

use std::sync::Arc;

use chrono::Utc;
use stratum_core::{MemoryStore, TrajectoryStore};
use stratum_memory::DefaultMemoryStore;
use stratum_test_utils::mocks::MockTrajectoryStore;
use stratum_types::*;

fn make_store() -> DefaultMemoryStore<MockTrajectoryStore> {
    let trajectory = Arc::new(MockTrajectoryStore::default());
    DefaultMemoryStore::in_memory(trajectory).unwrap()
}

fn make_store_with_trajectory(
    trajectory: Arc<MockTrajectoryStore>,
) -> DefaultMemoryStore<MockTrajectoryStore> {
    DefaultMemoryStore::in_memory(trajectory).unwrap()
}

fn make_entry(id: &str, tier: MemoryTier, content: &str) -> MemoryEntry {
    MemoryEntry {
        id: id.to_string(),
        tier,
        content: content.to_string(),
        metadata: serde_json::json!({"source": "test"}),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

// --- Write/Read per tier ---

#[tokio::test]
async fn test_working_write_read() {
    let store = make_store();
    let entry = make_entry("w1", MemoryTier::Working, "working memory data");

    store.write(MemoryTier::Working, &entry).await.unwrap();
    let read = store
        .read(MemoryTier::Working, "w1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.content, "working memory data");
}

#[tokio::test]
async fn test_episodic_write_read() {
    let store = make_store();
    let entry = make_entry("e1", MemoryTier::Episodic, "episodic data");

    store.write(MemoryTier::Episodic, &entry).await.unwrap();
    let read = store
        .read(MemoryTier::Episodic, "e1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.content, "episodic data");
}

#[tokio::test]
async fn test_project_write_read() {
    let store = make_store();
    let entry = make_entry("p1", MemoryTier::Project, "project knowledge");

    store.write(MemoryTier::Project, &entry).await.unwrap();
    let read = store
        .read(MemoryTier::Project, "p1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.content, "project knowledge");
}

#[tokio::test]
async fn test_global_write_read() {
    let store = make_store();
    let entry = make_entry("g1", MemoryTier::Global, "global truth");

    store.write(MemoryTier::Global, &entry).await.unwrap();
    let read = store.read(MemoryTier::Global, "g1").await.unwrap().unwrap();
    assert_eq!(read.content, "global truth");
}

// --- BM25 search ---

#[tokio::test]
async fn test_episodic_bm25_search() {
    let store = make_store();
    store
        .write(
            MemoryTier::Episodic,
            &make_entry("e1", MemoryTier::Episodic, "rust programming language"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Episodic,
            &make_entry("e2", MemoryTier::Episodic, "python scripting"),
        )
        .await
        .unwrap();

    let results = store
        .search(MemoryTier::Episodic, "rust", 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.id, "e1");
}

#[tokio::test]
async fn test_working_search() {
    let store = make_store();
    store
        .write(
            MemoryTier::Working,
            &make_entry("w1", MemoryTier::Working, "current task context"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Working,
            &make_entry("w2", MemoryTier::Working, "unrelated data"),
        )
        .await
        .unwrap();

    let results = store.search(MemoryTier::Working, "task", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.id, "w1");
}

// --- Promotion chain ---

#[tokio::test]
async fn test_promotion_working_to_episodic() {
    let store = make_store();
    let entry = make_entry("w1", MemoryTier::Working, "promote me");

    store.write(MemoryTier::Working, &entry).await.unwrap();
    store
        .promote("w1", MemoryTier::Working, MemoryTier::Episodic)
        .await
        .unwrap();

    let read = store
        .read(MemoryTier::Episodic, "w1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.content, "promote me");
    assert_eq!(read.tier, MemoryTier::Episodic);
}

#[tokio::test]
async fn test_promotion_episodic_to_project() {
    let store = make_store();
    let entry = make_entry("e1", MemoryTier::Episodic, "important insight");

    store.write(MemoryTier::Episodic, &entry).await.unwrap();
    store
        .promote("e1", MemoryTier::Episodic, MemoryTier::Project)
        .await
        .unwrap();

    let read = store
        .read(MemoryTier::Project, "e1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.content, "important insight");
}

// --- Global promotion queuing ---

#[tokio::test]
async fn test_global_promotion_queued() {
    let store = make_store();
    let entry = make_entry("p1", MemoryTier::Project, "should be queued");

    store.write(MemoryTier::Project, &entry).await.unwrap();
    store
        .promote("p1", MemoryTier::Project, MemoryTier::Global)
        .await
        .unwrap();

    // Should NOT be directly readable in global (it's queued)
    let read = store.read(MemoryTier::Global, "p1").await.unwrap();
    assert!(read.is_none());

    // Should appear in pending promotions
    let pending = store.pending_promotions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entry_id, "p1");

    // Approve it
    let queue_id = pending[0].id.clone();
    store.approve_promotion(&queue_id).unwrap();

    // Now should be readable
    let read = store.read(MemoryTier::Global, "p1").await.unwrap().unwrap();
    assert_eq!(read.content, "should be queued");
}

// --- Trajectory event verification ---

#[tokio::test]
async fn test_trajectory_events_emitted() {
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let store = make_store_with_trajectory(trajectory.clone());

    store
        .write(
            MemoryTier::Working,
            &make_entry("w1", MemoryTier::Working, "test"),
        )
        .await
        .unwrap();

    let events = trajectory
        .query_events(None, Some(EventType::MemoryWritten), None, None)
        .await
        .unwrap();
    assert!(!events.is_empty());
    assert_eq!(events[0].event_type, EventType::MemoryWritten);
    assert_eq!(events[0].stratum_layer, StratumLayer::MemoryHierarchy);
}

#[tokio::test]
async fn test_search_emits_event() {
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let store = make_store_with_trajectory(trajectory.clone());

    store
        .write(
            MemoryTier::Working,
            &make_entry("w1", MemoryTier::Working, "searchable"),
        )
        .await
        .unwrap();
    store
        .search(MemoryTier::Working, "searchable", 5)
        .await
        .unwrap();

    let events = trajectory
        .query_events(None, Some(EventType::MemorySearched), None, None)
        .await
        .unwrap();
    assert!(!events.is_empty());
}

#[tokio::test]
async fn test_promotion_emits_event() {
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let store = make_store_with_trajectory(trajectory.clone());

    store
        .write(
            MemoryTier::Working,
            &make_entry("w1", MemoryTier::Working, "data"),
        )
        .await
        .unwrap();
    store
        .promote("w1", MemoryTier::Working, MemoryTier::Episodic)
        .await
        .unwrap();

    let events = trajectory
        .query_events(None, Some(EventType::MemoryPromoted), None, None)
        .await
        .unwrap();
    assert!(!events.is_empty());
}

// --- Auto-approve global promotion ---

#[tokio::test]
async fn test_auto_approve_global_writes_directly() {
    let trajectory = Arc::new(MockTrajectoryStore::default());
    let store = DefaultMemoryStore::in_memory_with_config(trajectory, |cfg| {
        cfg.auto_approve_global = true;
    })
    .unwrap();

    // Write to project tier first
    store
        .write(
            MemoryTier::Project,
            &make_entry("p1", MemoryTier::Project, "promote me directly"),
        )
        .await
        .unwrap();

    // Promote project → global (should skip queue and write directly)
    store
        .promote("p1", MemoryTier::Project, MemoryTier::Global)
        .await
        .unwrap();

    // Should be readable in global tier immediately (no queue)
    let entry = store
        .read(MemoryTier::Global, "p1")
        .await
        .unwrap()
        .expect("entry should exist in global tier");
    assert_eq!(entry.content, "promote me directly");

    // No pending promotions (skipped the queue)
    let pending = store.pending_promotions().unwrap();
    assert!(pending.is_empty());
}

// --- Invalid promotion errors ---

#[tokio::test]
async fn test_invalid_promotion_backward() {
    let store = make_store();
    store
        .write(
            MemoryTier::Episodic,
            &make_entry("e1", MemoryTier::Episodic, "data"),
        )
        .await
        .unwrap();

    let result = store
        .promote("e1", MemoryTier::Episodic, MemoryTier::Working)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_invalid_promotion_skip_tier() {
    let store = make_store();
    store
        .write(
            MemoryTier::Working,
            &make_entry("w1", MemoryTier::Working, "data"),
        )
        .await
        .unwrap();

    let result = store
        .promote("w1", MemoryTier::Working, MemoryTier::Project)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_promote_nonexistent_entry() {
    let store = make_store();

    let result = store
        .promote("nonexistent", MemoryTier::Working, MemoryTier::Episodic)
        .await;
    assert!(result.is_err());
}

// --- Clear working ---

#[tokio::test]
async fn test_clear_working() {
    let store = make_store();
    store
        .write(
            MemoryTier::Working,
            &make_entry("w1", MemoryTier::Working, "temp"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Working,
            &make_entry("w2", MemoryTier::Working, "temp2"),
        )
        .await
        .unwrap();

    store.clear_working();

    assert!(store
        .read(MemoryTier::Working, "w1")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .read(MemoryTier::Working, "w2")
        .await
        .unwrap()
        .is_none());
}

// --- Read nonexistent ---

#[tokio::test]
async fn test_read_nonexistent() {
    let store = make_store();
    assert!(store
        .read(MemoryTier::Working, "nope")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .read(MemoryTier::Episodic, "nope")
        .await
        .unwrap()
        .is_none());
}
