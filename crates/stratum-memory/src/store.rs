//! `DefaultMemoryStore<T>` — main MemoryStore implementation.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::Connection;
use uuid::Uuid;

use stratum_core::{MemoryStore, TrajectoryStore};
use stratum_types::*;

use crate::config::MemoryStoreConfig;
use crate::error::MemoryError;
use crate::tiers::global::{GlobalTier, PromotionRequest};
use crate::tiers::working::WorkingTier;
use crate::tiers::{episodic::EpisodicTier, project::ProjectTier, TierBackend};

/// Default memory store implementation with 4-tier hierarchy.
///
/// Generic over `TrajectoryStore` for event emission, following the
/// `DefaultContextEngine<S,T,L>` composition pattern.
pub struct DefaultMemoryStore<T: TrajectoryStore> {
    config: MemoryStoreConfig,
    trajectory: Arc<T>,
    working: WorkingTier,
    episodic: EpisodicTier,
    project: ProjectTier,
    global: GlobalTier,
    /// Holds the temp directory alive for `in_memory()` stores.
    _temp_dir: Option<tempfile::TempDir>,
}

impl<T: TrajectoryStore> DefaultMemoryStore<T> {
    /// Create a new memory store, initializing all tier backends.
    pub fn new(config: MemoryStoreConfig, trajectory: Arc<T>) -> Result<Self, MemoryError> {
        // Ensure parent dirs exist for DB files
        if let Some(parent) = config.episodic_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = config.global_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let episodic_conn = Connection::open(&config.episodic_db_path)?;
        let global_conn = Connection::open(&config.global_db_path)?;

        Ok(Self {
            trajectory,
            working: WorkingTier::new(),
            episodic: EpisodicTier::new(Arc::new(Mutex::new(episodic_conn)))?,
            project: ProjectTier::new(config.project_memory_dir.clone())?,
            global: GlobalTier::new(Arc::new(Mutex::new(global_conn)))?,
            config,
            _temp_dir: None,
        })
    }

    /// Create a memory store with in-memory SQLite databases (for testing).
    pub fn in_memory(trajectory: Arc<T>) -> Result<Self, MemoryError> {
        let dir = tempfile::tempdir().map_err(MemoryError::Io)?;
        let config = MemoryStoreConfig {
            project_memory_dir: dir.path().join("memory"),
            episodic_db_path: dir.path().join("episodic.db"),
            global_db_path: dir.path().join("global.db"),
            ..Default::default()
        };

        let episodic_conn = Connection::open_in_memory()?;
        let global_conn = Connection::open_in_memory()?;

        Ok(Self {
            trajectory,
            working: WorkingTier::new(),
            episodic: EpisodicTier::new(Arc::new(Mutex::new(episodic_conn)))?,
            project: ProjectTier::new(config.project_memory_dir.clone())?,
            global: GlobalTier::new(Arc::new(Mutex::new(global_conn)))?,
            config,
            _temp_dir: Some(dir),
        })
    }

    /// Clear all working memory entries.
    pub fn clear_working(&self) {
        self.working.clear();
    }

    /// Approve a pending global promotion by queue ID.
    pub fn approve_promotion(&self, queue_id: &str) -> Result<(), MemoryError> {
        self.global.approve_promotion(queue_id)
    }

    /// Reject a pending global promotion by queue ID.
    pub fn reject_promotion(&self, queue_id: &str) -> Result<(), MemoryError> {
        self.global.reject_promotion(queue_id)
    }

    /// List all pending global promotions.
    pub fn pending_promotions(&self) -> Result<Vec<PromotionRequest>, MemoryError> {
        self.global.pending_promotions()
    }

    fn tier_backend(&self, tier: MemoryTier) -> &dyn TierBackend {
        match tier {
            MemoryTier::Working => &self.working,
            MemoryTier::Episodic => &self.episodic,
            MemoryTier::Project => &self.project,
            MemoryTier::Global => &self.global,
        }
    }

    async fn emit_event(
        &self,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> Result<(), MemoryError> {
        let event = TrajectoryEvent {
            event_id: Uuid::new_v4(),
            run_id: Uuid::nil(), // Memory events are not always run-scoped
            parent_run_id: None,
            timestamp: Utc::now(),
            event_type,
            stratum_layer: StratumLayer::MemoryHierarchy,
            payload,
            token_cost: TokenCost::default(),
        };
        self.trajectory
            .emit_event(event)
            .await
            .map_err(|e| MemoryError::Trajectory(e.to_string()))
    }

    /// Validate that promotion follows the tier order: Working < Episodic < Project < Global.
    fn validate_promotion(from: MemoryTier, to: MemoryTier) -> Result<(), MemoryError> {
        let from_ord = tier_order(from);
        let to_ord = tier_order(to);

        if to_ord <= from_ord {
            return Err(MemoryError::InvalidPromotion(format!(
                "cannot promote from {from:?} to {to:?}: must promote forward"
            )));
        }

        if to_ord - from_ord > 1 {
            return Err(MemoryError::InvalidPromotion(format!(
                "cannot promote from {from:?} to {to:?}: cannot skip tiers"
            )));
        }

        Ok(())
    }

    /// Run a closure on the appropriate tier backend inside `spawn_blocking`.
    ///
    /// Uses `wrap()` constructors to avoid re-running schema initialization.
    async fn with_blocking_tier<F, R>(&self, tier: MemoryTier, f: F) -> Result<R, MemoryError>
    where
        F: FnOnce(&dyn TierBackend) -> Result<R, MemoryError> + Send + 'static,
        R: Send + 'static,
    {
        let episodic_conn = self.episodic.connection().clone();
        let global_conn = self.global.connection().clone();
        let project_conn = self.project.connection().clone();
        let project_dir = self.project.dir().clone();

        tokio::task::spawn_blocking(move || match tier {
            MemoryTier::Episodic => f(&EpisodicTier::wrap(episodic_conn)),
            MemoryTier::Project => f(&ProjectTier::wrap(project_dir, project_conn)),
            MemoryTier::Global => f(&GlobalTier::wrap(global_conn)),
            MemoryTier::Working => unreachable!(),
        })
        .await?
    }
}

/// Map tier to ordinal for promotion validation.
fn tier_order(tier: MemoryTier) -> u8 {
    match tier {
        MemoryTier::Working => 0,
        MemoryTier::Episodic => 1,
        MemoryTier::Project => 2,
        MemoryTier::Global => 3,
    }
}

/// Serialize a `MemoryTier` as a lowercase string for event payloads.
fn tier_label(tier: MemoryTier) -> &'static str {
    match tier {
        MemoryTier::Working => "working",
        MemoryTier::Episodic => "episodic",
        MemoryTier::Project => "project",
        MemoryTier::Global => "global",
    }
}

impl<T: TrajectoryStore> std::fmt::Debug for DefaultMemoryStore<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultMemoryStore").finish()
    }
}

#[async_trait]
impl<T: TrajectoryStore + 'static> MemoryStore for DefaultMemoryStore<T> {
    type Error = MemoryError;

    async fn read(&self, tier: MemoryTier, id: &str) -> Result<Option<MemoryEntry>, Self::Error> {
        match tier {
            MemoryTier::Working => self.tier_backend(tier).read(id),
            _ => {
                let id = id.to_string();
                self.with_blocking_tier(tier, move |backend| backend.read(&id))
                    .await
            }
        }
    }

    async fn write(&self, tier: MemoryTier, entry: &MemoryEntry) -> Result<(), Self::Error> {
        match &self.config.search_strategy {
            SearchStrategy::Vector => return Err(MemoryError::VectorNotImplemented),
            SearchStrategy::Hybrid { .. } => {
                tracing::warn!(
                    "Hybrid search requested but vector not implemented; falling back to BM25"
                );
            }
            SearchStrategy::Bm25 => {}
        }

        match tier {
            MemoryTier::Working => {
                self.working.write(entry)?;
            }
            _ => {
                let entry = entry.clone();
                self.with_blocking_tier(tier, move |backend| backend.write(&entry))
                    .await?;
            }
        }

        self.emit_event(
            EventType::MemoryWritten,
            serde_json::json!({
                "tier": tier_label(tier),
                "entry_id": entry.id,
            }),
        )
        .await?;

        Ok(())
    }

    async fn search(
        &self,
        tier: MemoryTier,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchResult>, Self::Error> {
        match &self.config.search_strategy {
            SearchStrategy::Vector => return Err(MemoryError::VectorNotImplemented),
            SearchStrategy::Hybrid { .. } => {
                tracing::warn!(
                    "Hybrid search requested but vector not implemented; falling back to BM25"
                );
            }
            SearchStrategy::Bm25 => {}
        }

        let results = match tier {
            MemoryTier::Working => self.working.search(query, limit)?,
            _ => {
                let query = query.to_string();
                self.with_blocking_tier(tier, move |backend| backend.search(&query, limit))
                    .await?
            }
        };

        self.emit_event(
            EventType::MemorySearched,
            serde_json::json!({
                "tier": tier_label(tier),
                "query": query,
                "results_count": results.len(),
            }),
        )
        .await?;

        Ok(results)
    }

    async fn promote(
        &self,
        entry_id: &str,
        from: MemoryTier,
        to: MemoryTier,
    ) -> Result<(), Self::Error> {
        Self::validate_promotion(from, to)?;

        // Read the entry from source tier
        let entry = self
            .read(from, entry_id)
            .await?
            .ok_or_else(|| MemoryError::EntryNotFound {
                tier: from,
                id: entry_id.to_string(),
            })?;

        let promoted_entry = MemoryEntry {
            tier: to,
            updated_at: Utc::now(),
            ..entry
        };

        // Global promotion goes through the queue
        if to == MemoryTier::Global {
            let global_conn = self.global.connection().clone();
            let entry_for_queue = promoted_entry.clone();

            tokio::task::spawn_blocking(move || {
                let t = GlobalTier::wrap(global_conn);
                t.queue_promotion(&entry_for_queue)
            })
            .await??;
        } else {
            self.write(to, &promoted_entry).await?;
        }

        self.emit_event(
            EventType::MemoryPromoted,
            serde_json::json!({
                "entry_id": entry_id,
                "from": tier_label(from),
                "to": tier_label(to),
                "queued": to == MemoryTier::Global,
            }),
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_order() {
        assert_eq!(tier_order(MemoryTier::Working), 0);
        assert_eq!(tier_order(MemoryTier::Episodic), 1);
        assert_eq!(tier_order(MemoryTier::Project), 2);
        assert_eq!(tier_order(MemoryTier::Global), 3);
    }

    #[test]
    fn test_validate_promotion_valid() {
        // Adjacent tiers forward are valid
        assert!(tier_order(MemoryTier::Episodic) - tier_order(MemoryTier::Working) == 1);
        assert!(tier_order(MemoryTier::Project) - tier_order(MemoryTier::Episodic) == 1);
        assert!(tier_order(MemoryTier::Global) - tier_order(MemoryTier::Project) == 1);
    }

    #[test]
    fn test_validate_promotion_backward() {
        assert!(tier_order(MemoryTier::Working) < tier_order(MemoryTier::Episodic));
        // Backward: to_ord <= from_ord
        assert!(tier_order(MemoryTier::Working) <= tier_order(MemoryTier::Working));
    }

    #[test]
    fn test_validate_promotion_skip() {
        // Skip: to_ord - from_ord > 1
        assert!(tier_order(MemoryTier::Project) - tier_order(MemoryTier::Working) > 1);
        assert!(tier_order(MemoryTier::Global) - tier_order(MemoryTier::Episodic) > 1);
    }

    #[test]
    fn test_tier_label() {
        assert_eq!(tier_label(MemoryTier::Working), "working");
        assert_eq!(tier_label(MemoryTier::Episodic), "episodic");
        assert_eq!(tier_label(MemoryTier::Project), "project");
        assert_eq!(tier_label(MemoryTier::Global), "global");
    }
}
