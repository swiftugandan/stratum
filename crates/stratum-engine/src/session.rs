//! SqliteSessionManager: Simplified session lifecycle (4 states).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use stratum_core::ports::SessionManager;
use stratum_core::*;

use crate::error::EngineError;
use crate::util::{deserialize_enum, parse_utc, serialize_enum};

/// Valid transitions for the simplified 4-state machine.
///
/// Running -> Completed | Failed | Aborted
fn is_valid_transition(from: RunState, to: RunState) -> bool {
    matches!(
        (from, to),
        (RunState::Running, RunState::Completed)
            | (RunState::Running, RunState::Failed)
            | (RunState::Running, RunState::Aborted)
    )
}

pub struct SqliteSessionManager {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSessionManager {
    pub fn new(path: &str) -> Result<Self, EngineError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, EngineError> {
        let mgr = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        mgr.init_schema()?;
        Ok(mgr)
    }

    fn init_schema(&self) -> Result<(), EngineError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                id                TEXT PRIMARY KEY,
                parent_run_id     TEXT,
                model_ref         TEXT NOT NULL,
                tool_manifest     TEXT NOT NULL DEFAULT '[]',
                context_budget    TEXT NOT NULL DEFAULT '{}',
                spawn_depth_limit INTEGER NOT NULL DEFAULT 2,
                state             TEXT NOT NULL DEFAULT 'Running',
                created_at        TEXT NOT NULL,
                updated_at        TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS checkpoints (
                id              TEXT PRIMARY KEY,
                run_id          TEXT NOT NULL,
                state           TEXT NOT NULL,
                goal            TEXT NOT NULL DEFAULT '',
                context_summary TEXT NOT NULL DEFAULT '',
                tool_call_log   TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL,
                FOREIGN KEY (run_id) REFERENCES runs(id)
            );
            CREATE INDEX IF NOT EXISTS idx_checkpoints_run_id ON checkpoints(run_id);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_created_at ON checkpoints(created_at);",
        )?;
        Ok(())
    }
}

impl std::fmt::Debug for SqliteSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSessionManager").finish()
    }
}

type RawRunRow = (
    String,         // id
    Option<String>, // parent_run_id
    String,         // model_ref
    String,         // tool_manifest
    String,         // context_budget
    i64,            // spawn_depth_limit
    String,         // state
    String,         // created_at
);

fn parse_run_row(row: RawRunRow) -> Result<StratumRun, EngineError> {
    let (id, parent, model_ref, tools, budget, depth, state, created) = row;
    Ok(StratumRun {
        id: id
            .parse()
            .map_err(|e| EngineError::InvalidState(format!("invalid run id: {e}")))?,
        parent_run_id: parent
            .map(|p| {
                p.parse()
                    .map_err(|e| EngineError::InvalidState(format!("invalid parent_run_id: {e}")))
            })
            .transpose()?,
        model_ref,
        tool_manifest: serde_json::from_str(&tools)?,
        context_budget: serde_json::from_str(&budget)?,
        spawn_depth_limit: depth as u32,
        state: deserialize_enum(&state)?,
        created_at: parse_utc(&created)?,
    })
}

#[async_trait]
impl SessionManager for SqliteSessionManager {
    type Error = EngineError;

    async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error> {
        let conn = Arc::clone(&self.conn);
        let run = config.clone();
        tokio::task::spawn_blocking(move || {
            let now = Utc::now().to_rfc3339();
            let id = run.id.to_string();
            let parent = run.parent_run_id.map(|p| p.to_string());
            let tools = serde_json::to_string(&run.tool_manifest)?;
            let budget = serde_json::to_string(&run.context_budget)?;
            let state = serialize_enum(&run.state)?;

            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO runs (id, parent_run_id, model_ref, tool_manifest,
                    context_budget, spawn_depth_limit, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    parent,
                    run.model_ref,
                    tools,
                    budget,
                    run.spawn_depth_limit,
                    state,
                    run.created_at.to_rfc3339(),
                    now,
                ],
            )?;
            Ok::<(), EngineError>(())
        })
        .await??;

        Ok(config)
    }

    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        let cp = checkpoint.clone();
        tokio::task::spawn_blocking(move || {
            let id = cp.id.to_string();
            let rid = cp.run_id.to_string();
            let state = serialize_enum(&cp.state)?;
            let tool_log = serde_json::to_string(&cp.tool_call_log)?;

            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO checkpoints (id, run_id, state, goal, context_summary,
                    tool_call_log, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    rid,
                    state,
                    cp.goal,
                    cp.context_summary,
                    tool_log,
                    cp.created_at.to_rfc3339(),
                ],
            )?;
            Ok::<(), EngineError>(())
        })
        .await??;
        Ok(())
    }

    async fn transition_state(
        &self,
        run_id: RunId,
        new_state: RunState,
    ) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();
        let new_state_str = serialize_enum(&new_state)?;

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();

            let state_str: String = conn
                .query_row(
                    "SELECT state FROM runs WHERE id = ?1",
                    params![rid],
                    |row| row.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        EngineError::NotFound(format!("run {run_id}"))
                    }
                    other => EngineError::Sqlite(other),
                })?;

            let current_state: RunState = deserialize_enum(&state_str)?;

            if !is_valid_transition(current_state, new_state) {
                return Err(EngineError::InvalidState(format!(
                    "invalid transition: {:?} -> {:?}",
                    current_state, new_state
                )));
            }

            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE runs SET state = ?1, updated_at = ?2 WHERE id = ?3",
                params![new_state_str, now, rid],
            )?;

            Ok(())
        })
        .await??;

        Ok(())
    }

    async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, model_ref, tool_manifest,
                        context_budget, spawn_depth_limit, state, created_at
                 FROM runs WHERE id = ?1",
            )?;

            let result = stmt.query_row(params![rid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            });

            match result {
                Ok(row) => parse_run_row(row).map(Some),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(EngineError::Sqlite(e)),
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
                "SELECT id, run_id, state, goal, context_summary,
                        tool_call_log, created_at
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
                ))
            });

            match result {
                Ok((id, _run_id, state, goal, summary, tool_log, created)) => {
                    let cp = Checkpoint {
                        id: id.parse().map_err(|e| {
                            EngineError::InvalidState(format!("checkpoint id: {e}"))
                        })?,
                        run_id,
                        state: deserialize_enum(&state)?,
                        goal,
                        context_summary: summary,
                        tool_call_log: serde_json::from_str(&tool_log)?,
                        created_at: parse_utc(&created)?,
                    };
                    Ok(Some(cp))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(EngineError::Sqlite(e)),
            }
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_test_run() -> StratumRun {
        StratumRun {
            id: Uuid::new_v4(),
            parent_run_id: None,
            model_ref: "test-model".to_string(),
            tool_manifest: vec!["bash".to_string()],
            context_budget: ContextBudget::default(),
            spawn_depth_limit: 2,
            state: RunState::Running,
            created_at: Utc::now(),
        }
    }

    fn make_checkpoint(run_id: RunId) -> Checkpoint {
        Checkpoint {
            id: Uuid::new_v4(),
            run_id,
            state: RunState::Running,
            goal: "Build a feature".to_string(),
            context_summary: "Working on step 2 of 5".to_string(),
            tool_call_log: vec![],
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_and_get_run() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        let loaded = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.model_ref, "test-model");
        assert_eq!(loaded.state, RunState::Running);
    }

    #[tokio::test]
    async fn get_nonexistent_run() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let result = mgr.get_run(Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn full_lifecycle() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let run = make_test_run();
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        mgr.transition_state(id, RunState::Completed).await.unwrap();
        let r = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(r.state, RunState::Completed);
    }

    #[tokio::test]
    async fn invalid_state_transitions() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let run = make_test_run();
        let id = run.id;
        mgr.create_run(run).await.unwrap();

        // Running -> Running is invalid
        let err = mgr
            .transition_state(id, RunState::Running)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    #[tokio::test]
    async fn checkpoint_roundtrip() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let run = make_test_run();
        let id = run.id;
        mgr.create_run(run).await.unwrap();

        let cp = make_checkpoint(id);
        let cp_id = cp.id;
        mgr.checkpoint(&cp).await.unwrap();

        let loaded = mgr.get_latest_checkpoint(id).await.unwrap().unwrap();
        assert_eq!(loaded.id, cp_id);
        assert_eq!(loaded.goal, "Build a feature");
        assert_eq!(loaded.context_summary, "Working on step 2 of 5");
    }

    #[tokio::test]
    async fn multiple_checkpoints_returns_latest() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let run = make_test_run();
        let id = run.id;
        mgr.create_run(run).await.unwrap();

        let cp1 = Checkpoint {
            context_summary: "step 1".to_string(),
            ..make_checkpoint(id)
        };
        mgr.checkpoint(&cp1).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

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
    async fn parent_run_id_persisted() {
        let mgr = SqliteSessionManager::in_memory().unwrap();
        let parent_id = Uuid::new_v4();
        let mut run = make_test_run();
        run.parent_run_id = Some(parent_id);
        let id = run.id;

        mgr.create_run(run).await.unwrap();
        let loaded = mgr.get_run(id).await.unwrap().unwrap();
        assert_eq!(loaded.parent_run_id, Some(parent_id));
    }
}
