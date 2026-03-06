use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::sync::Mutex;

use stratum_core::{SessionManager, TrajectoryStore};
use stratum_types::*;

use crate::error::AdapterError;
use crate::util::{deserialize_enum, parse_utc, serialize_enum};

/// Valid state transitions for the run state machine.
///
/// State diagram:
///   Initialising -> Running | Failed | Aborted
///   Running -> Paused | Checkpointed | Completed | Failed | Aborted
///   Paused -> Running | Checkpointed | Aborted
///   Checkpointed -> Resuming | Aborted
///   Resuming -> Running | Failed
fn is_valid_transition(from: RunState, to: RunState) -> bool {
    matches!(
        (from, to),
        (RunState::Initialising, RunState::Running)
            | (RunState::Initialising, RunState::Failed)
            | (RunState::Initialising, RunState::Aborted)
            | (RunState::Running, RunState::Paused)
            | (RunState::Running, RunState::Checkpointed)
            | (RunState::Running, RunState::Completed)
            | (RunState::Running, RunState::Failed)
            | (RunState::Running, RunState::Aborted)
            | (RunState::Paused, RunState::Running)
            | (RunState::Paused, RunState::Checkpointed)
            | (RunState::Paused, RunState::Aborted)
            | (RunState::Checkpointed, RunState::Resuming)
            | (RunState::Checkpointed, RunState::Aborted)
            | (RunState::Resuming, RunState::Running)
            | (RunState::Resuming, RunState::Failed)
    )
}

fn event_type_for_state(state: RunState) -> EventType {
    match state {
        RunState::Initialising => EventType::RunCreated,
        RunState::Running => EventType::RunStarted,
        RunState::Paused => EventType::RunPaused,
        RunState::Checkpointed => EventType::CheckpointWritten,
        RunState::Resuming => EventType::RunResumed,
        RunState::Completed => EventType::RunCompleted,
        RunState::Failed => EventType::RunFailed,
        RunState::Aborted => EventType::RunAborted,
    }
}

/// Result of an atomic state transition: previous state + parent_run_id for event emission.
struct TransitionResult {
    from_state: RunState,
    parent_run_id: Option<RunId>,
}

pub struct SqliteSessionManager {
    conn: Arc<Mutex<Connection>>,
    trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
}

impl SqliteSessionManager {
    pub fn new(
        path: &str,
        trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
    ) -> Result<Self, AdapterError> {
        Self::from_connection(Connection::open(path)?, trajectory)
    }

    pub fn in_memory(
        trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
    ) -> Result<Self, AdapterError> {
        Self::from_connection(Connection::open_in_memory()?, trajectory)
    }

    fn from_connection(
        conn: Connection,
        trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
    ) -> Result<Self, AdapterError> {
        let mgr = Self {
            conn: Arc::new(Mutex::new(conn)),
            trajectory,
        };
        mgr.init_schema()?;
        Ok(mgr)
    }

    fn init_schema(&self) -> Result<(), AdapterError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                id              TEXT PRIMARY KEY,
                parent_run_id   TEXT,
                model_ref       TEXT NOT NULL,
                trust_level     TEXT NOT NULL,
                tool_manifest   TEXT NOT NULL DEFAULT '[]',
                memory_config   TEXT NOT NULL DEFAULT '{}',
                hitl_policy     TEXT NOT NULL DEFAULT '{}',
                context_budget  TEXT NOT NULL DEFAULT '{}',
                spawn_depth_limit INTEGER NOT NULL DEFAULT 2,
                state           TEXT NOT NULL DEFAULT 'Initialising',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS checkpoints (
                id              TEXT PRIMARY KEY,
                run_id          TEXT NOT NULL,
                state           TEXT NOT NULL,
                task_manifest   TEXT NOT NULL DEFAULT '{}',
                context_summary TEXT NOT NULL DEFAULT '',
                tool_call_log   TEXT NOT NULL DEFAULT '[]',
                sub_agent_tree  TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL,
                FOREIGN KEY (run_id) REFERENCES runs(id)
            );
            CREATE INDEX IF NOT EXISTS idx_checkpoints_run_id ON checkpoints(run_id);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_created_at ON checkpoints(created_at);",
        )?;
        Ok(())
    }

    async fn emit_event(
        &self,
        run_id: RunId,
        parent_run_id: Option<RunId>,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> Result<(), AdapterError> {
        let event = TrajectoryEvent::new(
            run_id,
            parent_run_id,
            event_type,
            StratumLayer::SessionLifecycle,
            payload,
        );
        self.trajectory.emit_event(event).await?;
        Ok(())
    }
}

impl std::fmt::Debug for SqliteSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSessionManager").finish()
    }
}

type RawRunRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
);

fn parse_run_row(row: RawRunRow) -> Result<StratumRun, AdapterError> {
    let (id, parent, model_ref, trust, tools, memory, hitl, budget, depth, state, created) = row;
    Ok(StratumRun {
        id: id
            .parse()
            .map_err(|e| AdapterError::InvalidState(format!("invalid run id: {e}")))?,
        parent_run_id: parent
            .map(|p| {
                p.parse()
                    .map_err(|e| AdapterError::InvalidState(format!("invalid parent_run_id: {e}")))
            })
            .transpose()?,
        model_ref,
        trust_level: deserialize_enum(&trust)?,
        tool_manifest: serde_json::from_str(&tools)?,
        memory_config: serde_json::from_str(&memory)?,
        hitl_policy: serde_json::from_str(&hitl)?,
        context_budget: serde_json::from_str(&budget)?,
        spawn_depth_limit: depth as u32,
        state: deserialize_enum(&state)?,
        created_at: parse_utc(&created)?,
    })
}

#[async_trait]
impl SessionManager for SqliteSessionManager {
    type Error = AdapterError;

    async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error> {
        let conn = Arc::clone(&self.conn);
        let run = config.clone();
        tokio::task::spawn_blocking(move || {
            let now = Utc::now().to_rfc3339();
            let id = run.id.to_string();
            let parent = run.parent_run_id.map(|p| p.to_string());
            let trust = serialize_enum(&run.trust_level)?;
            let tools = serde_json::to_string(&run.tool_manifest)?;
            let memory = serde_json::to_string(&run.memory_config)?;
            let hitl = serde_json::to_string(&run.hitl_policy)?;
            let budget = serde_json::to_string(&run.context_budget)?;
            let state = serialize_enum(&run.state)?;

            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO runs (id, parent_run_id, model_ref, trust_level, tool_manifest,
                    memory_config, hitl_policy, context_budget, spawn_depth_limit, state,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id,
                    parent,
                    run.model_ref,
                    trust,
                    tools,
                    memory,
                    hitl,
                    budget,
                    run.spawn_depth_limit,
                    state,
                    run.created_at.to_rfc3339(),
                    now,
                ],
            )?;
            Ok::<(), AdapterError>(())
        })
        .await??;

        self.emit_event(
            config.id,
            config.parent_run_id,
            EventType::RunCreated,
            serde_json::json!({
                "model_ref": config.model_ref,
                "trust_level": serialize_enum(&config.trust_level)?
            }),
        )
        .await?;

        Ok(config)
    }

    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        let cp = checkpoint.clone();
        let run_id = checkpoint.run_id;

        // Fetch parent_run_id in the same spawn_blocking as the insert
        let parent_run_id = tokio::task::spawn_blocking(move || {
            let id = cp.id.to_string();
            let rid = cp.run_id.to_string();
            let state = serialize_enum(&cp.state)?;
            let manifest = serde_json::to_string(&cp.task_manifest)?;
            let tool_log = serde_json::to_string(&cp.tool_call_log)?;
            let agents: Vec<String> = cp.sub_agent_tree.iter().map(|id| id.to_string()).collect();
            let agents_json = serde_json::to_string(&agents)?;

            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO checkpoints (id, run_id, state, task_manifest, context_summary,
                    tool_call_log, sub_agent_tree, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    rid,
                    state,
                    manifest,
                    cp.context_summary,
                    tool_log,
                    agents_json,
                    cp.created_at.to_rfc3339(),
                ],
            )?;

            // Fetch parent_run_id from the same connection lock
            let parent: Option<String> = conn
                .query_row(
                    "SELECT parent_run_id FROM runs WHERE id = ?1",
                    params![rid],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap_or(None);

            let parent_run_id = parent
                .map(|p| p.parse::<RunId>())
                .transpose()
                .map_err(|e| AdapterError::InvalidState(format!("parent_run_id: {e}")))?;

            Ok::<Option<RunId>, AdapterError>(parent_run_id)
        })
        .await??;

        self.emit_event(
            run_id,
            parent_run_id,
            EventType::CheckpointWritten,
            serde_json::json!({"checkpoint_id": checkpoint.id.to_string()}),
        )
        .await?;

        Ok(())
    }

    async fn resume(&self, run_id: RunId) -> Result<StratumRun, Self::Error> {
        // Atomic: read state, validate, update to Resuming, return full run — all in one lock
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();

        let run = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();

            // Read current state
            let state_str: String = conn
                .query_row(
                    "SELECT state FROM runs WHERE id = ?1",
                    params![rid],
                    |row| row.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        AdapterError::NotFound(format!("run {run_id}"))
                    }
                    other => AdapterError::Sqlite(other),
                })?;

            let current_state: RunState = deserialize_enum(&state_str)?;

            if !is_valid_transition(current_state, RunState::Resuming) {
                return Err(AdapterError::InvalidState(format!(
                    "cannot resume from state {:?}",
                    current_state
                )));
            }

            // Update to Resuming
            let new_state_str = serialize_enum(&RunState::Resuming)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE runs SET state = ?1, updated_at = ?2 WHERE id = ?3",
                params![new_state_str, now, rid],
            )?;

            // Read full row for return
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, model_ref, trust_level, tool_manifest,
                        memory_config, hitl_policy, context_budget, spawn_depth_limit,
                        state, created_at
                 FROM runs WHERE id = ?1",
            )?;

            stmt.query_row(params![rid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            })
            .map_err(AdapterError::Sqlite)
            .and_then(parse_run_row)
        })
        .await??;

        self.emit_event(
            run.id,
            run.parent_run_id,
            EventType::RunResumed,
            serde_json::json!({
                "from": serialize_enum(&RunState::Checkpointed)?,
                "to": serialize_enum(&RunState::Resuming)?
            }),
        )
        .await?;

        Ok(run)
    }

    async fn transition_state(
        &self,
        run_id: RunId,
        new_state: RunState,
    ) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();
        let new_state_str = serialize_enum(&new_state)?;

        // Atomic: read current state, validate, update, fetch parent_run_id — single lock
        let result = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();

            // Read current state and parent_run_id in one query
            let (state_str, parent_str): (String, Option<String>) = conn
                .query_row(
                    "SELECT state, parent_run_id FROM runs WHERE id = ?1",
                    params![rid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        AdapterError::NotFound(format!("run {run_id}"))
                    }
                    other => AdapterError::Sqlite(other),
                })?;

            let current_state: RunState = deserialize_enum(&state_str)?;

            if !is_valid_transition(current_state, new_state) {
                return Err(AdapterError::InvalidState(format!(
                    "invalid transition: {:?} -> {:?}",
                    current_state, new_state
                )));
            }

            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE runs SET state = ?1, updated_at = ?2 WHERE id = ?3",
                params![new_state_str, now, rid],
            )?;

            let parent_run_id = parent_str
                .map(|p| p.parse::<RunId>())
                .transpose()
                .map_err(|e| AdapterError::InvalidState(format!("parent_run_id: {e}")))?;

            Ok(TransitionResult {
                from_state: current_state,
                parent_run_id,
            })
        })
        .await??;

        self.emit_event(
            run_id,
            result.parent_run_id,
            event_type_for_state(new_state),
            serde_json::json!({
                "from": serialize_enum(&result.from_state)?,
                "to": serialize_enum(&new_state)?
            }),
        )
        .await?;

        Ok(())
    }

    async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, model_ref, trust_level, tool_manifest,
                        memory_config, hitl_policy, context_budget, spawn_depth_limit,
                        state, created_at
                 FROM runs WHERE id = ?1",
            )?;

            let result = stmt.query_row(params![rid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            });

            match result {
                Ok(row) => parse_run_row(row).map(Some),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(AdapterError::Sqlite(e)),
            }
        })
        .await?
    }

    async fn get_latest_checkpoint(
        &self,
        run_id: RunId,
    ) -> Result<Option<Checkpoint>, Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, run_id, state, task_manifest, context_summary,
                        tool_call_log, sub_agent_tree, created_at
                 FROM checkpoints WHERE run_id = ?1
                 ORDER BY created_at DESC LIMIT 1",
            )?;

            let result = stmt.query_row(params![rid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            });

            match result {
                Ok((id, _run_id, state, manifest, summary, tool_log, agents, created)) => {
                    let agent_strs: Vec<String> = serde_json::from_str(&agents)?;
                    let sub_agent_tree: Result<Vec<RunId>, _> =
                        agent_strs.iter().map(|s| s.parse()).collect();

                    let cp = Checkpoint {
                        id: id.parse().map_err(|e| {
                            AdapterError::InvalidState(format!("checkpoint id: {e}"))
                        })?,
                        run_id,
                        state: deserialize_enum(&state)?,
                        task_manifest: serde_json::from_str(&manifest)?,
                        context_summary: summary,
                        tool_call_log: serde_json::from_str(&tool_log)?,
                        sub_agent_tree: sub_agent_tree.map_err(|e| {
                            AdapterError::InvalidState(format!("sub_agent_tree: {e}"))
                        })?,
                        created_at: parse_utc(&created)?,
                    };
                    Ok(Some(cp))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(AdapterError::Sqlite(e)),
            }
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trajectory::SqliteTrajectoryStore;
    use stratum_core::TrajectoryStore;
    use stratum_test_utils::mocks::make_test_run;
    use uuid::Uuid;

    fn make_checkpoint(run_id: RunId) -> Checkpoint {
        Checkpoint {
            id: Uuid::new_v4(),
            run_id,
            state: RunState::Checkpointed,
            task_manifest: TaskManifest {
                goal: "Build a feature".to_string(),
                acceptance_criteria: vec![],
                progress: vec![],
                decisions: vec![],
                blockers: vec![],
            },
            context_summary: "Working on step 2 of 5".to_string(),
            tool_call_log: vec![],
            sub_agent_tree: vec![],
            created_at: Utc::now(),
        }
    }

    async fn setup() -> (SqliteSessionManager, Arc<SqliteTrajectoryStore>) {
        let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
        let mgr = SqliteSessionManager::in_memory(traj.clone()).unwrap();
        (mgr, traj)
    }

    #[tokio::test]
    async fn test_create_and_get_run() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        let created = mgr.create_run(run.clone()).await.unwrap();
        assert_eq!(created.id, id);

        let loaded = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.model_ref, "test-model");
        assert_eq!(loaded.trust_level, TrustLevel::Supervised);
        assert_eq!(loaded.state, RunState::Initialising);
        assert_eq!(loaded.tool_manifest, vec!["read_file"]);
    }

    #[tokio::test]
    async fn test_get_nonexistent_run() {
        let (mgr, _traj) = setup().await;
        let result = mgr.get_run(Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_full_lifecycle() {
        let (mgr, traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        // Create
        mgr.create_run(run).await.unwrap();

        // Initialising -> Running
        mgr.transition_state(id, RunState::Running).await.unwrap();
        let r = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(r.state, RunState::Running);

        // Running -> Checkpointed
        mgr.transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();

        // Write checkpoint
        let cp = make_checkpoint(id);
        mgr.checkpoint(&cp).await.unwrap();

        // Checkpointed -> Resuming -> Running
        mgr.transition_state(id, RunState::Resuming).await.unwrap();
        mgr.transition_state(id, RunState::Running).await.unwrap();

        // Running -> Completed
        mgr.transition_state(id, RunState::Completed).await.unwrap();
        let r = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(r.state, RunState::Completed);

        // Verify trajectory events were emitted
        let events = traj.query_events(Some(id), None, None, None).await.unwrap();
        assert!(events.len() >= 5);
    }

    #[tokio::test]
    async fn test_invalid_state_transitions() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();

        // Initialising -> Completed is invalid
        let err = mgr
            .transition_state(id, RunState::Completed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));

        // Initialising -> Paused is invalid
        let err = mgr
            .transition_state(id, RunState::Paused)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));

        // Initialising -> Checkpointed is invalid
        let err = mgr
            .transition_state(id, RunState::Checkpointed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    #[tokio::test]
    async fn test_checkpoint_roundtrip() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        mgr.transition_state(id, RunState::Running).await.unwrap();
        mgr.transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();

        let cp = make_checkpoint(id);
        let cp_id = cp.id;
        mgr.checkpoint(&cp).await.unwrap();

        let loaded = mgr.get_latest_checkpoint(id).await.unwrap().unwrap();
        assert_eq!(loaded.id, cp_id);
        assert_eq!(loaded.run_id, id);
        assert_eq!(loaded.context_summary, "Working on step 2 of 5");
        assert_eq!(loaded.task_manifest.goal, "Build a feature");
    }

    #[tokio::test]
    async fn test_resume_from_checkpoint() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        mgr.transition_state(id, RunState::Running).await.unwrap();
        mgr.transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();

        let cp = make_checkpoint(id);
        mgr.checkpoint(&cp).await.unwrap();

        // Resume should transition to Resuming
        let resumed = mgr.resume(id).await.unwrap();
        assert_eq!(resumed.state, RunState::Resuming);
    }

    #[tokio::test]
    async fn test_resume_invalid_state() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();

        // Can't resume from Initialising
        let err = mgr.resume(id).await.unwrap_err();
        assert!(err.to_string().contains("cannot resume"));
    }

    #[tokio::test]
    async fn test_pause_and_resume() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        mgr.transition_state(id, RunState::Running).await.unwrap();

        // Running -> Paused (HITL gate)
        mgr.transition_state(id, RunState::Paused).await.unwrap();
        let r = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(r.state, RunState::Paused);

        // Paused -> Running (decision received)
        mgr.transition_state(id, RunState::Running).await.unwrap();
        let r = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(r.state, RunState::Running);
    }

    #[tokio::test]
    async fn test_multiple_checkpoints_returns_latest() {
        let (mgr, _traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        mgr.transition_state(id, RunState::Running).await.unwrap();
        mgr.transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();

        // First checkpoint
        let cp1 = Checkpoint {
            context_summary: "step 1".to_string(),
            ..make_checkpoint(id)
        };
        mgr.checkpoint(&cp1).await.unwrap();

        // Small delay to ensure distinct timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Second checkpoint
        let cp2 = Checkpoint {
            context_summary: "step 2".to_string(),
            ..make_checkpoint(id)
        };
        let cp2_id = cp2.id;
        mgr.checkpoint(&cp2).await.unwrap();

        let latest = mgr.get_latest_checkpoint(id).await.unwrap().unwrap();
        assert_eq!(latest.id, cp2_id);
        assert_eq!(latest.context_summary, "step 2");
    }

    #[tokio::test]
    async fn test_parent_run_id_persisted() {
        let (mgr, _traj) = setup().await;
        let parent_id = Uuid::new_v4();
        let mut run = make_test_run();
        run.parent_run_id = Some(parent_id);
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        let loaded = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(loaded.parent_run_id, Some(parent_id));
    }

    #[tokio::test]
    async fn test_trajectory_events_emitted() {
        let (mgr, traj) = setup().await;
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();

        let events = traj
            .query_events(Some(id), Some(EventType::RunCreated), None, None)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stratum_layer, StratumLayer::SessionLifecycle);
    }

    #[tokio::test]
    async fn test_checkpoint_emits_event_with_parent_run_id() {
        let (mgr, traj) = setup().await;
        let parent_id = Uuid::new_v4();
        let mut run = make_test_run();
        run.parent_run_id = Some(parent_id);
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        mgr.transition_state(id, RunState::Running).await.unwrap();
        mgr.transition_state(id, RunState::Checkpointed)
            .await
            .unwrap();

        let cp = make_checkpoint(id);
        mgr.checkpoint(&cp).await.unwrap();

        let events = traj
            .query_events(Some(id), Some(EventType::CheckpointWritten), None, None)
            .await
            .unwrap();
        // Two CheckpointWritten events: one from transition_state, one from checkpoint()
        let checkpoint_events: Vec<_> = events
            .iter()
            .filter(|e| e.payload.get("checkpoint_id").is_some())
            .collect();
        assert_eq!(checkpoint_events.len(), 1);
        assert_eq!(checkpoint_events[0].parent_run_id, Some(parent_id));
    }
}
