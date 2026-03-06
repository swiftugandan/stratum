//! Tests for cross-session memory persistence, promotion flow, and search quality.

use std::sync::Arc;

use chrono::Utc;
use stratum_core::MemoryStore;
use stratum_memory::store::DefaultMemoryStore;
use stratum_test_utils::mocks::MockTrajectoryStore;
use stratum_types::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_store() -> DefaultMemoryStore<MockTrajectoryStore> {
    let trajectory = Arc::new(MockTrajectoryStore::default());
    DefaultMemoryStore::in_memory(trajectory).unwrap()
}

fn make_entry(id: &str, tier: MemoryTier, content: &str) -> MemoryEntry {
    MemoryEntry {
        id: id.to_string(),
        tier,
        content: content.to_string(),
        metadata: serde_json::json!({"run_id": "abc"}),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

// ===========================================================================
// Cross-session memory persistence
// ===========================================================================

#[tokio::test]
async fn test_episodic_write_read_roundtrip() {
    let store = make_store();
    let entry = make_entry("ep-1", MemoryTier::Episodic, "episodic roundtrip content");

    store.write(MemoryTier::Episodic, &entry).await.unwrap();

    let read = store
        .read(MemoryTier::Episodic, "ep-1")
        .await
        .unwrap()
        .expect("entry should exist after write");

    assert_eq!(read.id, "ep-1");
    assert_eq!(read.content, "episodic roundtrip content");
    assert_eq!(read.tier, MemoryTier::Episodic);
}

#[tokio::test]
async fn test_project_write_read_roundtrip() {
    let store = make_store();
    let entry = make_entry("proj-1", MemoryTier::Project, "project roundtrip content");

    store.write(MemoryTier::Project, &entry).await.unwrap();

    let read = store
        .read(MemoryTier::Project, "proj-1")
        .await
        .unwrap()
        .expect("entry should exist after write");

    assert_eq!(read.id, "proj-1");
    assert_eq!(read.content, "project roundtrip content");
    assert_eq!(read.tier, MemoryTier::Project);
}

#[tokio::test]
async fn test_working_memory_cleared() {
    let store = make_store();
    let entry = make_entry("w-clear", MemoryTier::Working, "ephemeral data");

    store.write(MemoryTier::Working, &entry).await.unwrap();

    // Confirm it exists first
    let before = store.read(MemoryTier::Working, "w-clear").await.unwrap();
    assert!(before.is_some(), "entry should exist before clear");

    store.clear_working();

    let after = store.read(MemoryTier::Working, "w-clear").await.unwrap();
    assert!(
        after.is_none(),
        "entry should be gone after clear_working()"
    );
}

#[tokio::test]
async fn test_global_write_read_roundtrip() {
    let store = make_store();
    let entry = make_entry("glob-1", MemoryTier::Global, "global roundtrip content");

    store.write(MemoryTier::Global, &entry).await.unwrap();

    let read = store
        .read(MemoryTier::Global, "glob-1")
        .await
        .unwrap()
        .expect("entry should exist after direct write to Global");

    assert_eq!(read.id, "glob-1");
    assert_eq!(read.content, "global roundtrip content");
}

// ===========================================================================
// Promotion flow
// ===========================================================================

#[tokio::test]
async fn test_promote_working_to_episodic() {
    let store = make_store();
    let entry = make_entry("pw-1", MemoryTier::Working, "promote working to episodic");

    store.write(MemoryTier::Working, &entry).await.unwrap();
    store
        .promote("pw-1", MemoryTier::Working, MemoryTier::Episodic)
        .await
        .unwrap();

    let read = store
        .read(MemoryTier::Episodic, "pw-1")
        .await
        .unwrap()
        .expect("entry should be in Episodic after promotion");

    assert_eq!(read.content, "promote working to episodic");
    assert_eq!(read.tier, MemoryTier::Episodic);
}

#[tokio::test]
async fn test_promote_episodic_to_project() {
    let store = make_store();
    let entry = make_entry("pe-1", MemoryTier::Episodic, "promote episodic to project");

    store.write(MemoryTier::Episodic, &entry).await.unwrap();
    store
        .promote("pe-1", MemoryTier::Episodic, MemoryTier::Project)
        .await
        .unwrap();

    let read = store
        .read(MemoryTier::Project, "pe-1")
        .await
        .unwrap()
        .expect("entry should be in Project after promotion");

    assert_eq!(read.content, "promote episodic to project");
    assert_eq!(read.tier, MemoryTier::Project);
}

#[tokio::test]
async fn test_promote_project_to_global_queues() {
    let store = make_store();
    let entry = make_entry("pg-1", MemoryTier::Project, "promote project to global");

    store.write(MemoryTier::Project, &entry).await.unwrap();
    store
        .promote("pg-1", MemoryTier::Project, MemoryTier::Global)
        .await
        .unwrap();

    // Should NOT be directly readable in Global yet (queued)
    let read = store.read(MemoryTier::Global, "pg-1").await.unwrap();
    assert!(read.is_none(), "entry should be queued, not yet in Global");

    // Should appear in pending promotions
    let pending = store.pending_promotions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entry_id, "pg-1");

    // Approve it
    let queue_id = pending[0].id.clone();
    store.approve_promotion(&queue_id).unwrap();

    // Now should be readable in Global
    let read = store
        .read(MemoryTier::Global, "pg-1")
        .await
        .unwrap()
        .expect("entry should be in Global after approval");

    assert_eq!(read.content, "promote project to global");
}

#[tokio::test]
async fn test_promote_backward_fails() {
    let store = make_store();
    let entry = make_entry("back-1", MemoryTier::Episodic, "backward promotion attempt");

    store.write(MemoryTier::Episodic, &entry).await.unwrap();

    let result = store
        .promote("back-1", MemoryTier::Episodic, MemoryTier::Working)
        .await;

    assert!(result.is_err(), "backward promotion should fail");
}

#[tokio::test]
async fn test_promote_skip_tier_fails() {
    let store = make_store();
    let entry = make_entry("skip-1", MemoryTier::Working, "skip tier promotion attempt");

    store.write(MemoryTier::Working, &entry).await.unwrap();

    let result = store
        .promote("skip-1", MemoryTier::Working, MemoryTier::Project)
        .await;

    assert!(
        result.is_err(),
        "skipping a tier during promotion should fail"
    );
}

#[tokio::test]
async fn test_full_promotion_pipeline() {
    let store = make_store();
    let entry = make_entry("full-1", MemoryTier::Working, "full pipeline content");

    // Stage 1: Write to Working
    store.write(MemoryTier::Working, &entry).await.unwrap();
    let read = store
        .read(MemoryTier::Working, "full-1")
        .await
        .unwrap()
        .expect("should be in Working");
    assert_eq!(read.tier, MemoryTier::Working);

    // Stage 2: Promote Working -> Episodic
    store
        .promote("full-1", MemoryTier::Working, MemoryTier::Episodic)
        .await
        .unwrap();
    let read = store
        .read(MemoryTier::Episodic, "full-1")
        .await
        .unwrap()
        .expect("should be in Episodic");
    assert_eq!(read.tier, MemoryTier::Episodic);

    // Stage 3: Promote Episodic -> Project
    store
        .promote("full-1", MemoryTier::Episodic, MemoryTier::Project)
        .await
        .unwrap();
    let read = store
        .read(MemoryTier::Project, "full-1")
        .await
        .unwrap()
        .expect("should be in Project");
    assert_eq!(read.tier, MemoryTier::Project);

    // Stage 4: Promote Project -> Global (queued)
    store
        .promote("full-1", MemoryTier::Project, MemoryTier::Global)
        .await
        .unwrap();

    // Not in Global yet
    assert!(store
        .read(MemoryTier::Global, "full-1")
        .await
        .unwrap()
        .is_none());

    // Approve
    let pending = store.pending_promotions().unwrap();
    assert_eq!(pending.len(), 1);
    store.approve_promotion(&pending[0].id).unwrap();

    // Now in Global
    let read = store
        .read(MemoryTier::Global, "full-1")
        .await
        .unwrap()
        .expect("should be in Global after approval");
    assert_eq!(read.content, "full pipeline content");
}

#[tokio::test]
async fn test_global_promotion_rejected() {
    let store = make_store();
    let entry = make_entry("rej-1", MemoryTier::Project, "will be rejected");

    store.write(MemoryTier::Project, &entry).await.unwrap();
    store
        .promote("rej-1", MemoryTier::Project, MemoryTier::Global)
        .await
        .unwrap();

    // Verify it's pending
    let pending = store.pending_promotions().unwrap();
    assert_eq!(pending.len(), 1);

    // Reject it
    let queue_id = pending[0].id.clone();
    store.reject_promotion(&queue_id).unwrap();

    // Should NOT be in Global
    let read = store.read(MemoryTier::Global, "rej-1").await.unwrap();
    assert!(read.is_none(), "rejected entry should not appear in Global");

    // Queue should be empty (no pending left)
    let pending_after = store.pending_promotions().unwrap();
    assert!(
        pending_after.is_empty(),
        "no pending promotions after rejection"
    );
}

// ===========================================================================
// Search quality
// ===========================================================================

#[tokio::test]
async fn test_episodic_search_finds_matching() {
    let store = make_store();

    store
        .write(
            MemoryTier::Episodic,
            &make_entry("es-1", MemoryTier::Episodic, "rust async programming guide"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Episodic,
            &make_entry("es-2", MemoryTier::Episodic, "python data science tutorial"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Episodic,
            &make_entry("es-3", MemoryTier::Episodic, "javascript web development"),
        )
        .await
        .unwrap();

    let results = store
        .search(MemoryTier::Episodic, "rust", 10)
        .await
        .unwrap();

    assert_eq!(results.len(), 1, "only the rust entry should match");
    assert_eq!(results[0].entry.id, "es-1");
    assert!(
        results[0].relevance_score > 0.0,
        "relevance score should be positive"
    );
}

#[tokio::test]
async fn test_working_search_substring() {
    let store = make_store();

    store
        .write(
            MemoryTier::Working,
            &make_entry("ws-1", MemoryTier::Working, "database connection pooling"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Working,
            &make_entry("ws-2", MemoryTier::Working, "file system watcher setup"),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Working,
            &make_entry("ws-3", MemoryTier::Working, "network socket handling"),
        )
        .await
        .unwrap();

    let results = store
        .search(MemoryTier::Working, "database", 10)
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        1,
        "only the database entry should match via substring"
    );
    assert_eq!(results[0].entry.id, "ws-1");
}

#[tokio::test]
async fn test_search_respects_limit() {
    let store = make_store();

    // Write 10 entries that all contain the word "common"
    for i in 0..10 {
        let entry = make_entry(
            &format!("lim-{i}"),
            MemoryTier::Episodic,
            &format!("common keyword entry number {i}"),
        );
        store.write(MemoryTier::Episodic, &entry).await.unwrap();
    }

    let results = store
        .search(MemoryTier::Episodic, "common", 3)
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        3,
        "search should return at most the requested limit"
    );
}

#[tokio::test]
async fn test_search_empty_tier() {
    let store = make_store();

    let results = store
        .search(MemoryTier::Episodic, "anything", 10)
        .await
        .unwrap();

    assert!(
        results.is_empty(),
        "searching an empty tier should return no results"
    );
}

#[tokio::test]
async fn test_project_search_fts5() {
    let store = make_store();

    store
        .write(
            MemoryTier::Project,
            &make_entry(
                "ps-1",
                MemoryTier::Project,
                "hexagonal architecture patterns in rust",
            ),
        )
        .await
        .unwrap();
    store
        .write(
            MemoryTier::Project,
            &make_entry(
                "ps-2",
                MemoryTier::Project,
                "microservices deployment strategies",
            ),
        )
        .await
        .unwrap();

    let results = store
        .search(MemoryTier::Project, "hexagonal", 10)
        .await
        .unwrap();

    assert_eq!(results.len(), 1, "FTS5 should match the hexagonal entry");
    assert_eq!(results[0].entry.id, "ps-1");
    assert!(results[0].relevance_score > 0.0);
}

// ===========================================================================
// Edge cases
// ===========================================================================

#[tokio::test]
async fn test_write_and_read_different_tiers_isolated() {
    let store = make_store();

    let working_entry = make_entry("iso-1", MemoryTier::Working, "working version of content");
    let episodic_entry = make_entry("iso-1", MemoryTier::Episodic, "episodic version of content");

    store
        .write(MemoryTier::Working, &working_entry)
        .await
        .unwrap();
    store
        .write(MemoryTier::Episodic, &episodic_entry)
        .await
        .unwrap();

    let from_working = store
        .read(MemoryTier::Working, "iso-1")
        .await
        .unwrap()
        .expect("should exist in Working");
    let from_episodic = store
        .read(MemoryTier::Episodic, "iso-1")
        .await
        .unwrap()
        .expect("should exist in Episodic");

    assert_eq!(from_working.content, "working version of content");
    assert_eq!(from_episodic.content, "episodic version of content");
    assert_ne!(
        from_working.content, from_episodic.content,
        "tiers should be isolated — same ID, different content"
    );
}

#[tokio::test]
async fn test_promote_nonexistent_entry_fails() {
    let store = make_store();

    let result = store
        .promote("does-not-exist", MemoryTier::Working, MemoryTier::Episodic)
        .await;

    assert!(
        result.is_err(),
        "promoting a nonexistent entry should return an error"
    );
}
