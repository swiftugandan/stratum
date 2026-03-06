//! Metrics collector that processes trajectory events into Prometheus-compatible metrics.
//!
//! The collector queries the trajectory store and updates gauges/counters
//! on [`InMemoryMetrics`] for each known event type.

use std::sync::Arc;

use stratum_core::{HitlController, MetricsExporter, TrajectoryStore};
use stratum_types::*;

use crate::metrics::InMemoryMetrics;

// Metric name constants to avoid stringly-typed duplication.
pub const METRIC_RUN_STATE: &str = "stratum_run_state";
pub const METRIC_CONTEXT_TOKENS_USED: &str = "stratum_context_tokens_used";
pub const METRIC_COMPACTION_COUNT: &str = "stratum_compaction_count";
pub const METRIC_KV_CACHE_HIT_RATE: &str = "stratum_kv_cache_hit_rate";
pub const METRIC_TOTAL_INPUT_TOKENS: &str = "stratum_total_input_tokens";
pub const METRIC_TOTAL_CACHED_TOKENS: &str = "stratum_total_cached_tokens";
pub const METRIC_TOOL_CALLS_TOTAL: &str = "stratum_tool_calls_total";
pub const METRIC_TOOL_ERRORS_TOTAL: &str = "stratum_tool_errors_total";
pub const METRIC_TOOL_RETRIES_TOTAL: &str = "stratum_tool_retries_total";
pub const METRIC_TOOL_ERROR_RATE: &str = "stratum_tool_error_rate";
pub const METRIC_HYGIENE_DEGRADATIONS: &str = "stratum_hygiene_degradations";
pub const METRIC_SUBAGENTS_SPAWNED: &str = "stratum_subagents_spawned";
pub const METRIC_SUBAGENTS_ACTIVE: &str = "stratum_subagents_active";
pub const METRIC_LLM_INPUT_TOKENS_TOTAL: &str = "stratum_llm_input_tokens_total";
pub const METRIC_LLM_OUTPUT_TOKENS_TOTAL: &str = "stratum_llm_output_tokens_total";
pub const METRIC_LLM_CALLS_TOTAL: &str = "stratum_llm_calls_total";
pub const METRIC_HITL_QUEUE_DEPTH: &str = "stratum_hitl_queue_depth";

/// Accumulated counters from a single pass over trajectory events.
#[derive(Default)]
struct EventAggregates {
    last_state: f64,
    last_llm_input_tokens: u64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cached_tokens: u64,
    llm_calls: usize,
    compactions: usize,
    tool_calls: usize,
    tool_errors: usize,
    tool_retries: usize,
    hygiene_degradations: usize,
    subagents_spawned: usize,
    subagents_completed: usize,
    subagents_failed: usize,
}

/// Processes trajectory events and updates metrics accordingly.
pub struct MetricsCollector<T: TrajectoryStore, H: HitlController> {
    metrics: Arc<InMemoryMetrics>,
    trajectory: Arc<T>,
    hitl: Arc<H>,
}

impl<T: TrajectoryStore, H: HitlController> MetricsCollector<T, H> {
    pub fn new(metrics: Arc<InMemoryMetrics>, trajectory: Arc<T>, hitl: Arc<H>) -> Self {
        Self {
            metrics,
            trajectory,
            hitl,
        }
    }

    /// Return a reference to the underlying metrics store.
    pub fn metrics(&self) -> &Arc<InMemoryMetrics> {
        &self.metrics
    }

    /// Refresh all metrics by scanning trajectory events for a specific run.
    pub async fn refresh_run(&self, run_id: RunId) -> anyhow::Result<()> {
        let run_id_str = run_id.to_string();
        let labels = &[("run_id", run_id_str.as_str())];

        let events = self
            .trajectory
            .query_events(Some(run_id), None, None, None)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Single-pass aggregation over all events.
        let agg = aggregate_events(&events);

        // Emit all metrics from aggregates.
        self.metrics.gauge(METRIC_RUN_STATE, agg.last_state, labels);
        self.metrics.gauge(
            METRIC_CONTEXT_TOKENS_USED,
            agg.last_llm_input_tokens as f64,
            labels,
        );
        self.metrics
            .gauge(METRIC_COMPACTION_COUNT, agg.compactions as f64, labels);

        if agg.total_input_tokens > 0 {
            let hit_rate = agg.total_cached_tokens as f64 / agg.total_input_tokens as f64;
            self.metrics
                .gauge(METRIC_KV_CACHE_HIT_RATE, hit_rate, labels);
        }
        self.metrics.gauge(
            METRIC_TOTAL_INPUT_TOKENS,
            agg.total_input_tokens as f64,
            labels,
        );
        self.metrics.gauge(
            METRIC_TOTAL_CACHED_TOKENS,
            agg.total_cached_tokens as f64,
            labels,
        );

        self.metrics
            .gauge(METRIC_TOOL_CALLS_TOTAL, agg.tool_calls as f64, labels);
        self.metrics
            .gauge(METRIC_TOOL_ERRORS_TOTAL, agg.tool_errors as f64, labels);
        self.metrics
            .gauge(METRIC_TOOL_RETRIES_TOTAL, agg.tool_retries as f64, labels);
        if agg.tool_calls > 0 {
            let error_rate = agg.tool_errors as f64 / agg.tool_calls as f64;
            self.metrics
                .gauge(METRIC_TOOL_ERROR_RATE, error_rate, labels);
        }

        self.metrics.gauge(
            METRIC_HYGIENE_DEGRADATIONS,
            agg.hygiene_degradations as f64,
            labels,
        );

        self.metrics.gauge(
            METRIC_SUBAGENTS_SPAWNED,
            agg.subagents_spawned as f64,
            labels,
        );
        let active = agg
            .subagents_spawned
            .saturating_sub(agg.subagents_completed)
            .saturating_sub(agg.subagents_failed);
        self.metrics
            .gauge(METRIC_SUBAGENTS_ACTIVE, active as f64, labels);

        self.metrics.gauge(
            METRIC_LLM_INPUT_TOKENS_TOTAL,
            agg.total_input_tokens as f64,
            labels,
        );
        self.metrics.gauge(
            METRIC_LLM_OUTPUT_TOKENS_TOTAL,
            agg.total_output_tokens as f64,
            labels,
        );
        self.metrics
            .gauge(METRIC_LLM_CALLS_TOTAL, agg.llm_calls as f64, labels);

        Ok(())
    }

    /// Refresh global (non-run-specific) metrics.
    pub async fn refresh_global(&self) -> anyhow::Result<()> {
        let pending = self
            .hitl
            .pending_gates()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.metrics
            .gauge(METRIC_HITL_QUEUE_DEPTH, pending.len() as f64, &[]);

        Ok(())
    }
}

/// Single-pass aggregation over trajectory events.
fn aggregate_events(events: &[TrajectoryEvent]) -> EventAggregates {
    let mut agg = EventAggregates {
        last_state: -1.0,
        ..Default::default()
    };

    for event in events {
        // Track run state (keep latest)
        let state_val = match event.event_type {
            EventType::RunCreated => Some(0.0),
            EventType::RunStarted => Some(1.0),
            EventType::RunResumed => Some(2.0),
            EventType::RunCompleted => Some(3.0),
            EventType::RunFailed => Some(4.0),
            EventType::RunAborted => Some(5.0),
            EventType::RunPaused => Some(6.0),
            _ => None,
        };
        if let Some(v) = state_val {
            agg.last_state = v;
        }

        match event.event_type {
            EventType::LlmCompleted => {
                agg.llm_calls += 1;
                if let Some(input) = event.payload.get("input_tokens").and_then(|v| v.as_u64()) {
                    agg.total_input_tokens += input;
                    agg.last_llm_input_tokens = input;
                }
                if let Some(output) = event.payload.get("output_tokens").and_then(|v| v.as_u64()) {
                    agg.total_output_tokens += output;
                }
                if let Some(cached) = event.payload.get("cached_tokens").and_then(|v| v.as_u64()) {
                    agg.total_cached_tokens += cached;
                }
            }
            EventType::CompactionStage1Triggered
            | EventType::CompactionStage2Triggered
            | EventType::CompactionStage3Triggered => {
                agg.compactions += 1;
            }
            EventType::ToolCalled => agg.tool_calls += 1,
            EventType::ToolFailed => agg.tool_errors += 1,
            EventType::ToolRetried => agg.tool_retries += 1,
            EventType::HygieneScoreDegraded => agg.hygiene_degradations += 1,
            EventType::SubagentSpawned => agg.subagents_spawned += 1,
            EventType::SubagentCompleted => agg.subagents_completed += 1,
            EventType::SubagentFailed => agg.subagents_failed += 1,
            _ => {}
        }
    }

    agg
}

#[cfg(test)]
mod tests {
    use super::*;
    use stratum_core::MetricsExporter;
    use stratum_test_utils::mocks::{MockHitlController, MockTrajectoryStore};

    fn make_event(
        run_id: RunId,
        event_type: EventType,
        payload: serde_json::Value,
    ) -> TrajectoryEvent {
        TrajectoryEvent::new(
            run_id,
            None,
            event_type,
            StratumLayer::TrajectoryStore,
            payload,
        )
    }

    #[tokio::test]
    async fn test_run_state_gauge() {
        let run_id = uuid::Uuid::new_v4();
        let traj = Arc::new(MockTrajectoryStore::default());
        traj.emit_event(make_event(
            run_id,
            EventType::RunCreated,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
        traj.emit_event(make_event(
            run_id,
            EventType::RunStarted,
            serde_json::json!({}),
        ))
        .await
        .unwrap();

        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_run(run_id).await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_RUN_STATE));
        assert!(output.contains("1")); // RunStarted = 1.0
    }

    #[tokio::test]
    async fn test_tool_metrics() {
        let run_id = uuid::Uuid::new_v4();
        let traj = Arc::new(MockTrajectoryStore::default());
        for _ in 0..5 {
            traj.emit_event(make_event(
                run_id,
                EventType::ToolCalled,
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        }
        traj.emit_event(make_event(
            run_id,
            EventType::ToolFailed,
            serde_json::json!({}),
        ))
        .await
        .unwrap();

        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_run(run_id).await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_TOOL_CALLS_TOTAL));
        assert!(output.contains(METRIC_TOOL_ERRORS_TOTAL));
        assert!(output.contains(METRIC_TOOL_ERROR_RATE));
    }

    #[tokio::test]
    async fn test_cache_metrics() {
        let run_id = uuid::Uuid::new_v4();
        let traj = Arc::new(MockTrajectoryStore::default());
        traj.emit_event(make_event(
            run_id,
            EventType::LlmCompleted,
            serde_json::json!({
                "input_tokens": 1000,
                "output_tokens": 200,
                "cached_tokens": 500,
            }),
        ))
        .await
        .unwrap();

        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_run(run_id).await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_KV_CACHE_HIT_RATE));
        assert!(output.contains("0.5")); // 500/1000
    }

    #[tokio::test]
    async fn test_cost_metrics() {
        let run_id = uuid::Uuid::new_v4();
        let traj = Arc::new(MockTrajectoryStore::default());
        traj.emit_event(make_event(
            run_id,
            EventType::LlmCompleted,
            serde_json::json!({
                "input_tokens": 1000,
                "output_tokens": 200,
                "cached_tokens": 0,
            }),
        ))
        .await
        .unwrap();
        traj.emit_event(make_event(
            run_id,
            EventType::LlmCompleted,
            serde_json::json!({
                "input_tokens": 2000,
                "output_tokens": 300,
                "cached_tokens": 500,
            }),
        ))
        .await
        .unwrap();

        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_run(run_id).await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_LLM_INPUT_TOKENS_TOTAL));
        assert!(output.contains("3000")); // 1000 + 2000
        assert!(output.contains(METRIC_LLM_OUTPUT_TOKENS_TOTAL));
        assert!(output.contains("500")); // 200 + 300
        assert!(output.contains(METRIC_LLM_CALLS_TOTAL));
    }

    #[tokio::test]
    async fn test_global_hitl_depth() {
        let traj = Arc::new(MockTrajectoryStore::default());
        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_global().await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_HITL_QUEUE_DEPTH));
    }

    #[tokio::test]
    async fn test_subagent_metrics() {
        let run_id = uuid::Uuid::new_v4();
        let traj = Arc::new(MockTrajectoryStore::default());
        for _ in 0..3 {
            traj.emit_event(make_event(
                run_id,
                EventType::SubagentSpawned,
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        }
        traj.emit_event(make_event(
            run_id,
            EventType::SubagentCompleted,
            serde_json::json!({}),
        ))
        .await
        .unwrap();

        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_run(run_id).await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_SUBAGENTS_SPAWNED));
        assert!(output.contains(METRIC_SUBAGENTS_ACTIVE));
    }

    #[tokio::test]
    async fn test_compaction_metrics() {
        let run_id = uuid::Uuid::new_v4();
        let traj = Arc::new(MockTrajectoryStore::default());
        traj.emit_event(make_event(
            run_id,
            EventType::CompactionStage1Triggered,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
        traj.emit_event(make_event(
            run_id,
            EventType::CompactionStage2Triggered,
            serde_json::json!({}),
        ))
        .await
        .unwrap();

        let metrics = Arc::new(InMemoryMetrics::new());
        let hitl = Arc::new(MockHitlController::default());
        let collector = MetricsCollector::new(metrics.clone(), traj, hitl);

        collector.refresh_run(run_id).await.unwrap();

        let output = metrics.export_metrics();
        assert!(output.contains(METRIC_COMPACTION_COUNT));
    }
}
