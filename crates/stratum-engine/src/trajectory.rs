//! SqliteTrajectoryStore: Event persistence.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{params, Connection};
use stratum_core::ports::TrajectoryStore;
use stratum_core::*;

use crate::error::EngineError;
use crate::util::{deserialize_enum, parse_utc, serialize_enum};

pub struct SqliteTrajectoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTrajectoryStore {
    pub fn new(path: &str) -> Result<Self, EngineError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, EngineError> {
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), EngineError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS trajectory_events (
                event_id          TEXT PRIMARY KEY,
                run_id            TEXT NOT NULL,
                parent_run_id     TEXT,
                timestamp         TEXT NOT NULL,
                event_type        TEXT NOT NULL,
                stratum_layer     TEXT NOT NULL,
                payload           TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_events_run_id ON trajectory_events(run_id);
            CREATE INDEX IF NOT EXISTS idx_events_event_type ON trajectory_events(event_type);
            CREATE INDEX IF NOT EXISTS idx_events_timestamp ON trajectory_events(timestamp);",
        )?;
        Ok(())
    }
}

impl std::fmt::Debug for SqliteTrajectoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteTrajectoryStore").finish()
    }
}

type RawRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
);

fn parse_row(row: RawRow) -> Result<TrajectoryEvent, EngineError> {
    let (event_id, run_id, parent_run_id, timestamp, event_type, stratum_layer, payload) = row;

    Ok(TrajectoryEvent {
        event_id: event_id
            .parse()
            .map_err(|e| EngineError::InvalidState(format!("event_id: {e}")))?,
        run_id: run_id
            .parse()
            .map_err(|e| EngineError::InvalidState(format!("run_id: {e}")))?,
        parent_run_id: parent_run_id
            .map(|s| {
                s.parse()
                    .map_err(|e| EngineError::InvalidState(format!("parent_run_id: {e}")))
            })
            .transpose()?,
        timestamp: parse_utc(&timestamp)?,
        event_type: deserialize_enum(&event_type)?,
        stratum_layer: deserialize_enum(&stratum_layer)?,
        payload: serde_json::from_str(&payload)?,
    })
}

#[async_trait]
impl TrajectoryStore for SqliteTrajectoryStore {
    type Error = EngineError;

    async fn emit_event(&self, event: TrajectoryEvent) -> Result<(), Self::Error> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let event_id = event.event_id.to_string();
            let run_id = event.run_id.to_string();
            let parent_run_id = event.parent_run_id.map(|id| id.to_string());
            let timestamp = event.timestamp.to_rfc3339();
            let event_type = serialize_enum(&event.event_type)?;
            let stratum_layer = serialize_enum(&event.stratum_layer)?;
            let payload = serde_json::to_string(&event.payload)?;

            let conn = conn.lock().unwrap();
            conn.execute(
                "INSERT INTO trajectory_events
                    (event_id, run_id, parent_run_id, timestamp, event_type,
                     stratum_layer, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    event_id,
                    run_id,
                    parent_run_id,
                    timestamp,
                    event_type,
                    stratum_layer,
                    payload,
                ],
            )?;
            Ok::<(), EngineError>(())
        })
        .await??;
        Ok(())
    }

    async fn query_events(
        &self,
        run_id: Option<RunId>,
        event_type: Option<EventType>,
        limit: Option<usize>,
    ) -> Result<Vec<TrajectoryEvent>, Self::Error> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut where_clauses = Vec::new();
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(rid) = &run_id {
                where_clauses.push("run_id = ?");
                param_values.push(Box::new(rid.to_string()));
            }
            if let Some(et) = &event_type {
                where_clauses.push("event_type = ?");
                param_values.push(Box::new(serialize_enum(et)?));
            }

            let mut sql = "SELECT event_id, run_id, parent_run_id, timestamp, event_type,
                           stratum_layer, payload
                           FROM trajectory_events"
                .to_string();

            if !where_clauses.is_empty() {
                sql.push_str(" WHERE ");
                let numbered: Vec<String> = where_clauses
                    .iter()
                    .enumerate()
                    .map(|(i, clause)| clause.replacen('?', &format!("?{}", i + 1), 1))
                    .collect();
                sql.push_str(&numbered.join(" AND "));
            }

            sql.push_str(" ORDER BY timestamp ASC");

            if let Some(n) = limit {
                let idx = param_values.len() + 1;
                sql.push_str(&format!(" LIMIT ?{idx}"));
                param_values.push(Box::new(n as i64));
            }

            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();

            let raw_rows: Vec<RawRow> = {
                let conn = conn.lock().unwrap();
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map(params_refs.as_slice(), |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };

            raw_rows.into_iter().map(parse_row).collect()
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_event(run_id: RunId, event_type: EventType) -> TrajectoryEvent {
        TrajectoryEvent::new(
            run_id,
            None,
            event_type,
            StratumLayer::Trajectory,
            serde_json::json!({"key": "value"}),
        )
    }

    #[tokio::test]
    async fn emit_and_query_by_run_id() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run1 = Uuid::new_v4();
        let run2 = Uuid::new_v4();

        store
            .emit_event(make_event(run1, EventType::RunCreated))
            .await
            .unwrap();
        store
            .emit_event(make_event(run1, EventType::RunStarted))
            .await
            .unwrap();
        store
            .emit_event(make_event(run2, EventType::RunCreated))
            .await
            .unwrap();

        let results = store.query_events(Some(run1), None, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.run_id == run1));
    }

    #[tokio::test]
    async fn query_by_event_type() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        store
            .emit_event(make_event(run, EventType::RunCreated))
            .await
            .unwrap();
        store
            .emit_event(make_event(run, EventType::ToolCalled))
            .await
            .unwrap();
        store
            .emit_event(make_event(run, EventType::ToolCalled))
            .await
            .unwrap();

        let results = store
            .query_events(None, Some(EventType::ToolCalled), None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn query_with_limit() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        for _ in 0..10 {
            store
                .emit_event(make_event(run, EventType::ToolCalled))
                .await
                .unwrap();
        }

        let results = store.query_events(None, None, Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn empty_query() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let results = store.query_events(None, None, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn concurrent_writes() {
        let store = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                for _ in 0..50 {
                    let event = make_event(Uuid::new_v4(), EventType::ToolCalled);
                    store.emit_event(event).await.unwrap();
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let all = store.query_events(None, None, None).await.unwrap();
        assert_eq!(all.len(), 500);
    }
}
