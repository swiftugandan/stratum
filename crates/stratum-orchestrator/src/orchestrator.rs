//! DefaultOrchestrator: the main Orchestrator implementation.
//!
//! Manages sub-agent spawning, depth enforcement, and result polling.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;

use stratum_core::{Orchestrator, SessionManager, TrajectoryStore};
use stratum_types::*;

use crate::config::OrchestratorConfig;
use crate::error::OrchestratorError;

/// Default implementation of the Sub-Agent Orchestrator.
///
/// Generic over `S: SessionManager` and `T: TrajectoryStore`.
pub struct DefaultOrchestrator<S: SessionManager, T: TrajectoryStore> {
    config: OrchestratorConfig,
    session: Arc<S>,
    trajectory: Arc<T>,
    depth_cache: RwLock<HashMap<RunId, u32>>,
}

impl<S: SessionManager, T: TrajectoryStore> std::fmt::Debug for DefaultOrchestrator<S, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultOrchestrator")
            .field("config", &self.config)
            .finish()
    }
}

impl<S: SessionManager, T: TrajectoryStore> DefaultOrchestrator<S, T> {
    pub fn new(config: OrchestratorConfig, session: Arc<S>, trajectory: Arc<T>) -> Self {
        Self {
            config,
            session,
            trajectory,
            depth_cache: RwLock::new(HashMap::new()),
        }
    }

    async fn emit_event(
        &self,
        run_id: RunId,
        parent_run_id: Option<RunId>,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> Result<(), OrchestratorError> {
        let event = TrajectoryEvent {
            event_id: Uuid::new_v4(),
            run_id,
            parent_run_id,
            timestamp: Utc::now(),
            event_type,
            stratum_layer: StratumLayer::SubAgentOrchestrator,
            payload,
            token_cost: TokenCost::default(),
        };
        self.trajectory
            .emit_event(event)
            .await
            .map_err(|e| OrchestratorError::Trajectory(e.to_string()))
    }

    async fn compute_depth(&self, run_id: RunId) -> Result<u32, OrchestratorError> {
        // Fast path: check cache
        {
            let cache = self.depth_cache.read().await;
            if let Some(&depth) = cache.get(&run_id) {
                return Ok(depth);
            }
        }

        // Slow path: walk parent chain
        let mut depth = 0u32;
        let mut current_id = run_id;
        loop {
            let run = self
                .session
                .get_run(current_id)
                .await
                .map_err(|e| OrchestratorError::Session(e.to_string()))?
                .ok_or(OrchestratorError::RunNotFound(current_id))?;
            match run.parent_run_id {
                Some(parent_id) => {
                    depth += 1;
                    // Check if parent depth is cached
                    let cache = self.depth_cache.read().await;
                    if let Some(&parent_depth) = cache.get(&parent_id) {
                        depth += parent_depth;
                        break;
                    }
                    drop(cache);
                    current_id = parent_id;
                }
                None => break,
            }
        }

        // Cache result
        let mut cache = self.depth_cache.write().await;
        cache.insert(run_id, depth);
        Ok(depth)
    }

    fn effective_limit(&self, spawn_depth_limit: u32) -> u32 {
        spawn_depth_limit.min(self.config.hard_max_depth)
    }
}

#[async_trait]
impl<S: SessionManager, T: TrajectoryStore> Orchestrator for DefaultOrchestrator<S, T> {
    type Error = OrchestratorError;

    async fn spawn(&self, parent_run_id: RunId, config: SpawnConfig) -> Result<RunId, Self::Error> {
        // Fetch parent run once (used for both depth check and inheriting settings)
        let parent_run = self
            .session
            .get_run(parent_run_id)
            .await
            .map_err(|e| OrchestratorError::Session(e.to_string()))?
            .ok_or(OrchestratorError::RunNotFound(parent_run_id))?;

        // Check depth limit
        let parent_depth = self.compute_depth(parent_run_id).await?;
        let limit = self.effective_limit(parent_run.spawn_depth_limit);
        if parent_depth >= limit {
            return Err(OrchestratorError::DepthLimitExceeded {
                current: parent_depth,
                limit,
            });
        }

        // Create child run
        let child_id = Uuid::new_v4();
        let child_run = StratumRun {
            id: child_id,
            parent_run_id: Some(parent_run_id),
            model_ref: config.model_ref,
            trust_level: config.trust_level,
            tool_manifest: config.tool_manifest,
            memory_config: MemoryConfig::default(),
            hitl_policy: HitlPolicy::default(),
            context_budget: ContextBudget::default(),
            spawn_depth_limit: parent_run.spawn_depth_limit,
            state: RunState::Initialising,
            created_at: Utc::now(),
        };

        self.session
            .create_run(child_run)
            .await
            .map_err(|e| OrchestratorError::Session(e.to_string()))?;

        // Cache child depth (parent_depth already computed above)
        let child_depth = parent_depth + 1;
        {
            let mut cache = self.depth_cache.write().await;
            cache.insert(child_id, child_depth);
        }

        // Emit appropriate event
        let event_type = match config.pattern {
            SpawnPattern::Janitor => EventType::JanitorRunStarted,
            _ => EventType::SubagentSpawned,
        };

        debug!(
            parent = %parent_run_id,
            child = %child_id,
            pattern = ?config.pattern,
            depth = child_depth,
            "sub-agent spawned"
        );

        self.emit_event(
            child_id,
            Some(parent_run_id),
            event_type,
            serde_json::json!({
                "pattern": format!("{:?}", config.pattern),
                "parent_id": parent_run_id.to_string(),
                "child_id": child_id.to_string(),
                "task_goal": config.task_goal,
                "depth": child_depth,
            }),
        )
        .await?;

        Ok(child_id)
    }

    async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error> {
        tokio::time::timeout(self.config.await_timeout, async {
            loop {
                let run = self
                    .session
                    .get_run(sub_run_id)
                    .await
                    .map_err(|e| OrchestratorError::Session(e.to_string()))?
                    .ok_or(OrchestratorError::RunNotFound(sub_run_id))?;

                let (event_type, summary) = match run.state {
                    RunState::Completed => (
                        EventType::SubagentCompleted,
                        "Sub-agent completed successfully.",
                    ),
                    RunState::Failed => (EventType::SubagentFailed, "Sub-agent failed."),
                    RunState::Aborted => (EventType::SubagentFailed, "Sub-agent aborted."),
                    _ => {
                        tokio::time::sleep(self.config.poll_interval).await;
                        continue;
                    }
                };

                self.emit_event(
                    sub_run_id,
                    run.parent_run_id,
                    event_type,
                    serde_json::json!({
                        "run_id": sub_run_id.to_string(),
                        "status": format!("{:?}", run.state),
                    }),
                )
                .await?;

                return Ok(SubAgentResult {
                    run_id: sub_run_id,
                    status: run.state,
                    summary: summary.to_string(),
                    artefacts: vec![],
                });
            }
        })
        .await
        .map_err(|_| OrchestratorError::Timeout(sub_run_id))?
    }

    fn current_depth(&self, run_id: RunId) -> Result<u32, Self::Error> {
        // Synchronous wrapper — check cache only for the sync trait method.
        // For full computation, use `compute_depth` async method.
        let cache = self.depth_cache.try_read();
        match cache {
            Ok(c) => Ok(c.get(&run_id).copied().unwrap_or(0)),
            Err(_) => Ok(0),
        }
    }

    fn can_spawn(&self, parent_run_id: RunId) -> Result<bool, Self::Error> {
        let depth = self.current_depth(parent_run_id)?;
        // We need the parent run's spawn_depth_limit, but this is sync.
        // Use the hard_max_depth as the limit since we can't do async here.
        // The actual per-run limit is checked in spawn() which is async.
        Ok(depth < self.config.hard_max_depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stratum_test_utils::mocks::{MockSessionManager, MockTrajectoryStore};

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
            task_goal: "Do the thing".to_string(),
            shared_filesystem_scope: None,
        }
    }

    fn make_orchestrator(
        session: Arc<MockSessionManager>,
        trajectory: Arc<MockTrajectoryStore>,
    ) -> DefaultOrchestrator<MockSessionManager, MockTrajectoryStore> {
        DefaultOrchestrator::new(
            OrchestratorConfig {
                hard_max_depth: 5,
                ..Default::default()
            },
            session,
            trajectory,
        )
    }

    #[tokio::test]
    async fn test_spawn_creates_child_run() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let orch = make_orchestrator(Arc::clone(&session), Arc::clone(&trajectory));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        let child_id = orch
            .spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();

        let runs = session.runs.lock().unwrap();
        let child = runs.iter().find(|r| r.id == child_id).unwrap();
        assert_eq!(child.parent_run_id, Some(parent_id));
        assert_eq!(child.state, RunState::Initialising);
        assert_eq!(child.model_ref, "child-model");
    }

    #[tokio::test]
    async fn test_spawn_emits_subagent_spawned() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let orch = make_orchestrator(Arc::clone(&session), Arc::clone(&trajectory));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        orch.spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();

        let events = trajectory.events.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::SubagentSpawned));
    }

    #[tokio::test]
    async fn test_spawn_respects_depth_limit() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let config = OrchestratorConfig {
            hard_max_depth: 1,
            ..Default::default()
        };
        let orch = DefaultOrchestrator::new(config, Arc::clone(&session), Arc::clone(&trajectory));

        // Create root run
        let root = make_parent_run(1);
        let root_id = root.id;
        session.create_run(root).await.unwrap();

        // Spawn child (depth 0 -> 1, at limit)
        let child_id = orch
            .spawn(root_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();

        // Try to spawn grandchild (depth 1 -> 2, exceeds hard_max_depth=1)
        let result = orch
            .spawn(child_id, make_spawn_config(SpawnPattern::Delegate))
            .await;
        assert!(matches!(
            result,
            Err(OrchestratorError::DepthLimitExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn test_can_spawn_within_limit() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let orch = make_orchestrator(Arc::clone(&session), Arc::clone(&trajectory));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        assert!(orch.can_spawn(parent_id).unwrap());
    }

    #[tokio::test]
    async fn test_can_spawn_at_limit() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let config = OrchestratorConfig {
            hard_max_depth: 1,
            ..Default::default()
        };
        let orch = DefaultOrchestrator::new(config, Arc::clone(&session), Arc::clone(&trajectory));

        let root = make_parent_run(1);
        let root_id = root.id;
        session.create_run(root).await.unwrap();

        // Spawn child at depth 1
        let child_id = orch
            .spawn(root_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();

        // can_spawn should be false for child (depth 1, hard_max=1)
        assert!(!orch.can_spawn(child_id).unwrap());
    }

    #[tokio::test]
    async fn test_current_depth_root_zero() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let orch = make_orchestrator(Arc::clone(&session), Arc::clone(&trajectory));

        let root = make_parent_run(3);
        let root_id = root.id;
        session.create_run(root).await.unwrap();

        // Root run has depth 0 (not yet cached, so returns default 0)
        assert_eq!(orch.current_depth(root_id).unwrap(), 0);
    }

    #[tokio::test]
    async fn test_current_depth_child_one() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let orch = make_orchestrator(Arc::clone(&session), Arc::clone(&trajectory));

        let root = make_parent_run(3);
        let root_id = root.id;
        session.create_run(root).await.unwrap();

        let child_id = orch
            .spawn(root_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();

        assert_eq!(orch.current_depth(child_id).unwrap(), 1);
    }

    #[tokio::test]
    async fn test_janitor_emits_janitor_event() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let orch = make_orchestrator(Arc::clone(&session), Arc::clone(&trajectory));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        orch.spawn(parent_id, make_spawn_config(SpawnPattern::Janitor))
            .await
            .unwrap();

        let events = trajectory.events.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == EventType::JanitorRunStarted));
    }

    #[tokio::test]
    async fn test_await_result_completed() {
        let session = Arc::new(MockSessionManager::default());
        let trajectory = Arc::new(MockTrajectoryStore::default());
        let config = OrchestratorConfig {
            poll_interval: std::time::Duration::from_millis(10),
            await_timeout: std::time::Duration::from_secs(5),
            ..Default::default()
        };
        let orch = DefaultOrchestrator::new(config, Arc::clone(&session), Arc::clone(&trajectory));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        let child_id = orch
            .spawn(parent_id, make_spawn_config(SpawnPattern::Delegate))
            .await
            .unwrap();

        // Transition child to Completed
        session
            .transition_state(child_id, RunState::Completed)
            .await
            .unwrap();

        let result = orch.await_result(child_id).await.unwrap();
        assert_eq!(result.status, RunState::Completed);
        assert_eq!(result.run_id, child_id);
    }
}
