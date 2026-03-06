use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{params, Connection};
use stratum_core::TrajectoryStore;
use stratum_types::{EventType, ExportFormat, RunId, TimeRange, TokenCost, TrajectoryEvent};

use crate::error::AdapterError;
use crate::util::{deserialize_enum, parse_utc, serialize_enum};

pub struct SqliteTrajectoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTrajectoryStore {
    pub fn new(path: &str) -> Result<Self, AdapterError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, AdapterError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, AdapterError> {
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), AdapterError> {
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
                payload           TEXT NOT NULL DEFAULT '{}',
                cached_tokens     INTEGER NOT NULL DEFAULT 0,
                uncached_tokens   INTEGER NOT NULL DEFAULT 0,
                estimated_cost_usd REAL NOT NULL DEFAULT 0.0
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
    i64,
    i64,
    f64,
);

fn parse_row(row: RawRow) -> Result<TrajectoryEvent, AdapterError> {
    let (
        event_id,
        run_id,
        parent_run_id,
        timestamp,
        event_type,
        stratum_layer,
        payload,
        cached_tokens,
        uncached_tokens,
        estimated_cost_usd,
    ) = row;

    Ok(TrajectoryEvent {
        event_id: event_id
            .parse()
            .map_err(|e| AdapterError::InvalidState(format!("event_id: {e}")))?,
        run_id: run_id
            .parse()
            .map_err(|e| AdapterError::InvalidState(format!("run_id: {e}")))?,
        parent_run_id: parent_run_id
            .map(|s| {
                s.parse()
                    .map_err(|e| AdapterError::InvalidState(format!("parent_run_id: {e}")))
            })
            .transpose()?,
        timestamp: parse_utc(&timestamp)?,
        event_type: deserialize_enum(&event_type)?,
        stratum_layer: deserialize_enum(&stratum_layer)?,
        payload: serde_json::from_str(&payload)?,
        token_cost: TokenCost {
            cached_tokens: cached_tokens as u64,
            uncached_tokens: uncached_tokens as u64,
            estimated_cost_usd,
        },
    })
}

#[async_trait]
impl TrajectoryStore for SqliteTrajectoryStore {
    type Error = AdapterError;

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
                     stratum_layer, payload, cached_tokens, uncached_tokens, estimated_cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    event_id,
                    run_id,
                    parent_run_id,
                    timestamp,
                    event_type,
                    stratum_layer,
                    payload,
                    event.token_cost.cached_tokens as i64,
                    event.token_cost.uncached_tokens as i64,
                    event.token_cost.estimated_cost_usd,
                ],
            )?;
            Ok::<(), AdapterError>(())
        })
        .await??;
        Ok(())
    }

    async fn query_events(
        &self,
        run_id: Option<RunId>,
        event_type: Option<EventType>,
        time_range: Option<TimeRange>,
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
            if let Some(tr) = &time_range {
                where_clauses.push("timestamp >= ?");
                param_values.push(Box::new(tr.start.to_rfc3339()));
                where_clauses.push("timestamp <= ?");
                param_values.push(Box::new(tr.end.to_rfc3339()));
            }

            let mut sql = "SELECT event_id, run_id, parent_run_id, timestamp, event_type,
                           stratum_layer, payload, cached_tokens, uncached_tokens, estimated_cost_usd
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

            // Hold the lock only for the SQLite query, collect raw data, then release.
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
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            // Mutex released — deserialize outside the critical section.

            raw_rows.into_iter().map(parse_row).collect()
        })
        .await?
    }

    async fn export(&self, run_id: RunId, format: ExportFormat) -> Result<Vec<u8>, Self::Error> {
        let events = self.query_events(Some(run_id), None, None, None).await?;

        match format {
            ExportFormat::Jsonl => {
                let mut out = Vec::new();
                for event in &events {
                    let line = serde_json::to_string(event)?;
                    out.extend_from_slice(line.as_bytes());
                    out.push(b'\n');
                }
                Ok(out)
            }
            ExportFormat::Csv => {
                let mut out = String::new();
                out.push_str("event_id,run_id,parent_run_id,timestamp,event_type,stratum_layer,payload,cached_tokens,uncached_tokens,estimated_cost_usd\n");
                for event in &events {
                    let parent = event
                        .parent_run_id
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    let event_type = serialize_enum(&event.event_type)?;
                    let stratum_layer = serialize_enum(&event.stratum_layer)?;
                    let payload = serde_json::to_string(&event.payload)?;
                    // RFC 4180: double-quote fields containing commas, quotes, or newlines
                    let escaped_payload = format!("\"{}\"", payload.replace('"', "\"\""));
                    out.push_str(&format!(
                        "{},{},{},{},{},{},{},{},{},{}\n",
                        event.event_id,
                        event.run_id,
                        parent,
                        event.timestamp.to_rfc3339(),
                        event_type,
                        stratum_layer,
                        escaped_payload,
                        event.token_cost.cached_tokens,
                        event.token_cost.uncached_tokens,
                        event.token_cost.estimated_cost_usd,
                    ));
                }
                Ok(out.into_bytes())
            }
            ExportFormat::Replay => {
                let json = serde_json::to_vec(&events)?;
                Ok(json)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stratum_types::StratumLayer;
    use uuid::Uuid;

    fn make_event(run_id: RunId, event_type: EventType) -> TrajectoryEvent {
        TrajectoryEvent {
            event_id: Uuid::new_v4(),
            run_id,
            parent_run_id: None,
            timestamp: Utc::now(),
            event_type,
            stratum_layer: StratumLayer::TrajectoryStore,
            payload: serde_json::json!({"key": "value"}),
            token_cost: TokenCost::default(),
        }
    }

    #[tokio::test]
    async fn test_emit_and_query_by_run_id() {
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

        let results = store
            .query_events(Some(run1), None, None, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.run_id == run1));
    }

    #[tokio::test]
    async fn test_query_by_event_type() {
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
            .query_events(None, Some(EventType::ToolCalled), None, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_query_by_time_range() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        let before = Utc::now();
        store
            .emit_event(make_event(run, EventType::RunCreated))
            .await
            .unwrap();
        // Small delay to ensure distinct timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mid = Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        store
            .emit_event(make_event(run, EventType::RunStarted))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let after = Utc::now();

        // Full range should return both
        let all = store
            .query_events(
                None,
                None,
                Some(TimeRange {
                    start: before,
                    end: after,
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        // Range ending at mid should exclude the second event
        let first_only = store
            .query_events(
                None,
                None,
                Some(TimeRange {
                    start: before,
                    end: mid,
                }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(first_only.len(), 1);
        assert_eq!(first_only[0].event_type, EventType::RunCreated);
    }

    #[tokio::test]
    async fn test_query_with_limit() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        for _ in 0..10 {
            store
                .emit_event(make_event(run, EventType::ToolCalled))
                .await
                .unwrap();
        }

        let results = store.query_events(None, None, None, Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_query_no_filters() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run1 = Uuid::new_v4();
        let run2 = Uuid::new_v4();

        store
            .emit_event(make_event(run1, EventType::RunCreated))
            .await
            .unwrap();
        store
            .emit_event(make_event(run2, EventType::RunStarted))
            .await
            .unwrap();

        let results = store.query_events(None, None, None, None).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_export_jsonl_roundtrip() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        store
            .emit_event(make_event(run, EventType::RunCreated))
            .await
            .unwrap();
        store
            .emit_event(make_event(run, EventType::RunStarted))
            .await
            .unwrap();

        let data = store.export(run, ExportFormat::Jsonl).await.unwrap();
        let text = String::from_utf8(data).unwrap();
        let lines: Vec<&str> = text.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        for line in lines {
            let event: TrajectoryEvent = serde_json::from_str(line).unwrap();
            assert_eq!(event.run_id, run);
        }
    }

    #[tokio::test]
    async fn test_export_csv() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        store
            .emit_event(make_event(run, EventType::RunCreated))
            .await
            .unwrap();
        store
            .emit_event(make_event(run, EventType::RunStarted))
            .await
            .unwrap();

        let data = store.export(run, ExportFormat::Csv).await.unwrap();
        let text = String::from_utf8(data).unwrap();
        let lines: Vec<&str> = text.trim().split('\n').collect();
        // 1 header + 2 data rows
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("event_id,"));
        // Payload should be double-quoted
        assert!(lines[1].contains('"'));
    }

    #[tokio::test]
    async fn test_export_replay() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let run = Uuid::new_v4();

        store
            .emit_event(make_event(run, EventType::RunCreated))
            .await
            .unwrap();
        store
            .emit_event(make_event(run, EventType::RunStarted))
            .await
            .unwrap();

        let data = store.export(run, ExportFormat::Replay).await.unwrap();
        let events: Vec<TrajectoryEvent> = serde_json::from_slice(&data).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.run_id == run));
    }

    #[tokio::test]
    async fn test_concurrent_writes() {
        let store = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    let event = make_event(Uuid::new_v4(), EventType::ToolCalled);
                    store.emit_event(event).await.unwrap();
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let all = store.query_events(None, None, None, None).await.unwrap();
        assert_eq!(all.len(), 1000);
    }

    #[tokio::test]
    async fn test_empty_query() {
        let store = SqliteTrajectoryStore::in_memory().unwrap();
        let results = store.query_events(None, None, None, None).await.unwrap();
        assert!(results.is_empty());
    }
}
