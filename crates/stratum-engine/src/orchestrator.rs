//! DefaultOrchestrator: sub-agent spawning, depth enforcement, result polling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;

use stratum_core::ports::{Orchestrator, SessionManager};
use stratum_core::*;

use crate::error::EngineError;

#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub hard_max_depth: u32,
    pub poll_interval: Duration,
    pub await_timeout: Duration,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            hard_max_depth: 5,
            poll_interval: Duration::from_millis(500),
            await_timeout: Duration::from_secs(300),
        }
    }
}

pub struct DefaultOrchestrator<S: SessionManager> {
    config: OrchestratorConfig,
    session: Arc<S>,
    depth_cache: RwLock<HashMap<RunId, u32>>,
}

impl<S: SessionManager> std::fmt::Debug for DefaultOrchestrator<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultOrchestrator")
            .field("config", &self.config)
            .finish()
    }
}

impl<S: SessionManager> DefaultOrchestrator<S> {
    pub fn new(config: OrchestratorConfig, session: Arc<S>) -> Self {
        Self {
            config,
            session,
            depth_cache: RwLock::new(HashMap::new()),
        }
    }

    async fn compute_depth(&self, run_id: RunId) -> Result<u32, EngineError> {
        {
            let cache = self.depth_cache.read().await;
            if let Some(&depth) = cache.get(&run_id) {
                return Ok(depth);
            }
        }

        let mut depth = 0u32;
        let mut current_id = run_id;
        loop {
            let run = self
                .session
                .get_run(current_id)
                .await
                .map_err(|e| EngineError::InvalidState(e.to_string()))?
                .ok_or(EngineError::NotFound(format!("run {current_id}")))?;
            match run.parent_run_id {
                Some(parent_id) => {
                    depth += 1;
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

        let mut cache = self.depth_cache.write().await;
        cache.insert(run_id, depth);
        Ok(depth)
    }

    fn effective_limit(&self, spawn_depth_limit: u32) -> u32 {
        spawn_depth_limit.min(self.config.hard_max_depth)
    }
}

#[async_trait]
impl<S: SessionManager> Orchestrator for DefaultOrchestrator<S> {
    type Error = EngineError;

    async fn spawn(&self, parent_run_id: RunId, config: SpawnConfig) -> Result<RunId, Self::Error> {
        let parent_run = self
            .session
            .get_run(parent_run_id)
            .await
            .map_err(|e| EngineError::InvalidState(e.to_string()))?
            .ok_or(EngineError::NotFound(format!("run {parent_run_id}")))?;

        let parent_depth = self.compute_depth(parent_run_id).await?;
        let limit = self.effective_limit(parent_run.spawn_depth_limit);
        if parent_depth >= limit {
            return Err(EngineError::DepthLimitExceeded {
                current: parent_depth,
                limit,
            });
        }

        let child_id = Uuid::new_v4();
        let child_run = StratumRun {
            id: child_id,
            parent_run_id: Some(parent_run_id),
            model_ref: config.model_ref,
            tool_manifest: config.tool_manifest,
            context_budget: ContextBudget::default(),
            spawn_depth_limit: parent_run.spawn_depth_limit,
            state: RunState::Running,
            created_at: Utc::now(),
        };

        self.session
            .create_run(child_run)
            .await
            .map_err(|e| EngineError::InvalidState(e.to_string()))?;

        let child_depth = parent_depth + 1;
        {
            let mut cache = self.depth_cache.write().await;
            cache.insert(child_id, child_depth);
        }

        debug!(
            parent = %parent_run_id,
            child = %child_id,
            pattern = ?config.pattern,
            depth = child_depth,
            "sub-agent spawned"
        );

        Ok(child_id)
    }

    async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error> {
        tokio::time::timeout(self.config.await_timeout, async {
            loop {
                let run = self
                    .session
                    .get_run(sub_run_id)
                    .await
                    .map_err(|e| EngineError::InvalidState(e.to_string()))?
                    .ok_or(EngineError::NotFound(format!("run {sub_run_id}")))?;

                let summary = match run.state {
                    RunState::Completed => "Sub-agent completed successfully.",
                    RunState::Failed => "Sub-agent failed.",
                    RunState::Aborted => "Sub-agent aborted.",
                    _ => {
                        tokio::time::sleep(self.config.poll_interval).await;
                        continue;
                    }
                };

                return Ok(SubAgentResult {
                    run_id: sub_run_id,
                    status: run.state,
                    summary: summary.to_string(),
                    artefacts: vec![],
                });
            }
        })
        .await
        .map_err(|_| EngineError::Timeout(sub_run_id))?
    }

    fn current_depth(&self, run_id: RunId) -> Result<u32, Self::Error> {
        let cache = self.depth_cache.try_read();
        match cache {
            Ok(c) => Ok(c.get(&run_id).copied().unwrap_or(0)),
            Err(_) => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SqliteSessionManager;

    fn make_parent_run(depth_limit: u32) -> StratumRun {
        StratumRun {
            id: Uuid::new_v4(),
            parent_run_id: None,
            model_ref: "test-model".to_string(),
            tool_manifest: vec![],
            context_budget: ContextBudget::default(),
            spawn_depth_limit: depth_limit,
            state: RunState::Running,
            created_at: Utc::now(),
        }
    }

    fn make_spawn_config() -> SpawnConfig {
        SpawnConfig {
            pattern: SpawnPattern::Delegate,
            model_ref: "child-model".to_string(),
            tool_manifest: vec!["read_file".to_string()],
            task_goal: "Do the thing".to_string(),
            shared_filesystem_scope: None,
        }
    }

    #[tokio::test]
    async fn spawn_creates_child_run() {
        let session = Arc::new(SqliteSessionManager::in_memory().unwrap());
        let orch = DefaultOrchestrator::new(OrchestratorConfig::default(), Arc::clone(&session));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        let child_id = orch.spawn(parent_id, make_spawn_config()).await.unwrap();

        let child = session.get_run(child_id).await.unwrap().unwrap();
        assert_eq!(child.parent_run_id, Some(parent_id));
        assert_eq!(child.state, RunState::Running);
    }

    #[tokio::test]
    async fn spawn_respects_depth_limit() {
        let session = Arc::new(SqliteSessionManager::in_memory().unwrap());
        let config = OrchestratorConfig {
            hard_max_depth: 1,
            ..Default::default()
        };
        let orch = DefaultOrchestrator::new(config, Arc::clone(&session));

        let root = make_parent_run(1);
        let root_id = root.id;
        session.create_run(root).await.unwrap();

        let child_id = orch.spawn(root_id, make_spawn_config()).await.unwrap();

        let result = orch.spawn(child_id, make_spawn_config()).await;
        assert!(matches!(
            result,
            Err(EngineError::DepthLimitExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn await_result_completed() {
        let session = Arc::new(SqliteSessionManager::in_memory().unwrap());
        let config = OrchestratorConfig {
            poll_interval: Duration::from_millis(10),
            await_timeout: Duration::from_secs(5),
            ..Default::default()
        };
        let orch = DefaultOrchestrator::new(config, Arc::clone(&session));

        let parent = make_parent_run(3);
        let parent_id = parent.id;
        session.create_run(parent).await.unwrap();

        let child_id = orch.spawn(parent_id, make_spawn_config()).await.unwrap();
        session
            .transition_state(child_id, RunState::Completed)
            .await
            .unwrap();

        let result = orch.await_result(child_id).await.unwrap();
        assert_eq!(result.status, RunState::Completed);
    }
}
