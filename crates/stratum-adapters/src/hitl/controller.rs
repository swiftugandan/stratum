//! `SqliteHitlController` — SQLite-backed HITL gate management with durable pause/resume.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::sync::Mutex;

use stratum_core::{HitlController, Notifier, SessionManager, TrajectoryStore};
use stratum_types::*;

use crate::error::AdapterError;
use crate::util::{deserialize_enum, serialize_enum};

/// SQL schema for the `hitl_gates` table and its indexes.
pub const HITL_GATES_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS hitl_gates (
        id               TEXT PRIMARY KEY,
        run_id           TEXT NOT NULL,
        gate_category    TEXT NOT NULL,
        action_attempted TEXT NOT NULL,
        alternatives     TEXT NOT NULL DEFAULT '[]',
        context_summary  TEXT NOT NULL DEFAULT '',
        decision         TEXT,
        created_at       TEXT NOT NULL,
        decided_at       TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_hitl_gates_run_id ON hitl_gates(run_id);
    CREATE INDEX IF NOT EXISTS idx_hitl_gates_pending ON hitl_gates(run_id, decision) WHERE decision IS NULL;
";

/// SQLite-backed HITL controller implementing gate management, decision recording,
/// and durable pause/resume.
pub struct SqliteHitlController {
    conn: Arc<Mutex<Connection>>,
    trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
    session: Arc<dyn SessionManager<Error = AdapterError>>,
    notifier: Arc<dyn Notifier<Error = AdapterError>>,
}

impl SqliteHitlController {
    /// Create a new controller backed by a file-based SQLite database.
    pub fn new(
        path: &str,
        trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
        session: Arc<dyn SessionManager<Error = AdapterError>>,
        notifier: Arc<dyn Notifier<Error = AdapterError>>,
    ) -> Result<Self, AdapterError> {
        Self::from_connection(Connection::open(path)?, trajectory, session, notifier)
    }

    /// Create a new controller backed by an in-memory SQLite database (for testing).
    pub fn in_memory(
        trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
        session: Arc<dyn SessionManager<Error = AdapterError>>,
        notifier: Arc<dyn Notifier<Error = AdapterError>>,
    ) -> Result<Self, AdapterError> {
        Self::from_connection(Connection::open_in_memory()?, trajectory, session, notifier)
    }

    fn from_connection(
        conn: Connection,
        trajectory: Arc<dyn TrajectoryStore<Error = AdapterError>>,
        session: Arc<dyn SessionManager<Error = AdapterError>>,
        notifier: Arc<dyn Notifier<Error = AdapterError>>,
    ) -> Result<Self, AdapterError> {
        let ctrl = Self {
            conn: Arc::new(Mutex::new(conn)),
            trajectory,
            session,
            notifier,
        };
        ctrl.init_schema()?;
        Ok(ctrl)
    }

    fn init_schema(&self) -> Result<(), AdapterError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(HITL_GATES_SCHEMA)?;
        Ok(())
    }

    async fn emit_event(
        &self,
        run_id: RunId,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> Result<(), AdapterError> {
        let event = TrajectoryEvent::new(
            run_id,
            None,
            event_type,
            StratumLayer::HitlController,
            payload,
        );
        self.trajectory.emit_event(event).await?;
        Ok(())
    }
}

impl std::fmt::Debug for SqliteHitlController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteHitlController").finish()
    }
}

#[async_trait]
impl HitlController for SqliteHitlController {
    type Error = AdapterError;

    async fn open_gate(&self, record: HitlRecord) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rec = record.clone();

        tokio::task::spawn_blocking(move || {
            let alternatives = serde_json::to_string(&rec.alternatives)?;
            let category_str = serialize_enum(&rec.gate_category)?;
            let now = Utc::now().to_rfc3339();

            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO hitl_gates (id, run_id, gate_category, action_attempted,
                    alternatives, context_summary, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    rec.id,
                    rec.run_id.to_string(),
                    category_str,
                    rec.action_attempted,
                    alternatives,
                    rec.context_summary,
                    now,
                ],
            )?;
            Ok::<(), AdapterError>(())
        })
        .await??;

        // Transition run to Paused
        self.session
            .transition_state(record.run_id, RunState::Paused)
            .await?;

        // Emit GateOpened event
        self.emit_event(
            record.run_id,
            EventType::GateOpened,
            serde_json::json!({
                "gate_id": record.id,
                "gate_category": record.gate_category.to_string(),
                "action_attempted": record.action_attempted,
            }),
        )
        .await?;

        // Notify
        self.notifier.notify(&record).await?;

        Ok(())
    }

    async fn record_decision(
        &self,
        run_id: RunId,
        decision: HitlDecision,
    ) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        // Serialize once, reuse for both DB storage and event payload
        let decision_value = serde_json::to_value(&decision)?;
        let decision_json = decision_value.to_string();
        let rid = run_id.to_string();

        // Update the first pending gate for this run
        let gate_id = tokio::task::spawn_blocking(move || {
            let now = Utc::now().to_rfc3339();
            let conn = conn.lock().unwrap();

            // Find the pending gate
            let gate_id: String = conn
                .query_row(
                    "SELECT id FROM hitl_gates WHERE run_id = ?1 AND decision IS NULL
                     ORDER BY created_at ASC LIMIT 1",
                    params![rid],
                    |row| row.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        AdapterError::NotFound(format!("no pending gate for run {rid}"))
                    }
                    other => AdapterError::Sqlite(other),
                })?;

            conn.execute(
                "UPDATE hitl_gates SET decision = ?1, decided_at = ?2
                 WHERE id = ?3",
                params![decision_json, now, gate_id],
            )?;

            Ok::<String, AdapterError>(gate_id)
        })
        .await??;

        // Determine target state and emit decision-specific events
        let target_state = match &decision {
            HitlDecision::Abort => RunState::Aborted,
            _ => RunState::Running,
        };

        self.session.transition_state(run_id, target_state).await?;

        if let HitlDecision::Redirect { new_goal } = &decision {
            self.emit_event(
                run_id,
                EventType::RunRedirected,
                serde_json::json!({
                    "gate_id": gate_id,
                    "new_goal": new_goal,
                }),
            )
            .await?;
        }

        // Emit GateDecisionReceived event
        self.emit_event(
            run_id,
            EventType::GateDecisionReceived,
            serde_json::json!({
                "gate_id": gate_id,
                "decision": decision_value,
            }),
        )
        .await?;

        Ok(())
    }

    async fn pending_gates(&self) -> Result<Vec<HitlRecord>, Self::Error> {
        let conn = Arc::clone(&self.conn);

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, run_id, gate_category, action_attempted, alternatives,
                        context_summary
                 FROM hitl_gates WHERE decision IS NULL
                 ORDER BY created_at ASC",
            )?;

            let mut records = Vec::new();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let rid: String = row.get(1)?;
                let cat_str: String = row.get(2)?;
                let alts_json: String = row.get(4)?;

                let run_id: RunId = rid
                    .parse()
                    .map_err(|e| AdapterError::InvalidState(format!("invalid run_id: {e}")))?;
                let gate_category: GateCategory = deserialize_enum(&cat_str)?;
                let alternatives: Vec<String> =
                    serde_json::from_str(&alts_json).map_err(AdapterError::Serialization)?;

                records.push(HitlRecord {
                    id,
                    run_id,
                    gate_category,
                    action_attempted: row.get(3)?,
                    alternatives,
                    context_summary: row.get(5)?,
                    decision: None,
                });
            }

            Ok(records)
        })
        .await?
    }

    async fn get_decision(
        &self,
        run_id: RunId,
        gate_id: &str,
    ) -> Result<Option<HitlDecision>, Self::Error> {
        let conn = Arc::clone(&self.conn);
        let rid = run_id.to_string();
        let gid = gate_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();

            let result = conn.query_row(
                "SELECT decision FROM hitl_gates WHERE id = ?1 AND run_id = ?2",
                params![gid, rid],
                |row| row.get::<_, Option<String>>(0),
            );

            match result {
                Ok(Some(json)) => {
                    let decision: HitlDecision = serde_json::from_str(&json)?;
                    Ok(Some(decision))
                }
                Ok(None) => Ok(None),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(AdapterError::Sqlite(e)),
            }
        })
        .await?
    }
}
