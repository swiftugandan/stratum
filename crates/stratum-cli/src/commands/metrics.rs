//! `stratum metrics` command — show current metrics snapshot.

use std::sync::Arc;

use stratum_adapters::{InMemoryMetrics, MetricsCollector};
use stratum_core::MetricsExporter;

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(run_id: Option<&str>, config: &StratumConfig) -> anyhow::Result<()> {
    let ctx = QueryContext::build(config)?;

    let metrics = Arc::new(InMemoryMetrics::new());
    let collector = MetricsCollector::new(
        Arc::clone(&metrics),
        Arc::clone(&ctx.trajectory),
        Arc::clone(&ctx.hitl),
    );

    // Refresh global metrics
    collector.refresh_global().await?;

    // If a run_id is given, refresh that run's metrics
    // Otherwise, find active runs and refresh each
    match run_id {
        Some(id) => {
            let rid = super::parse_run_id(id)?;
            collector.refresh_run(rid).await?;
        }
        None => {
            let all_ids = super::discover_run_ids(ctx.trajectory.as_ref()).await?;
            let active_ids = super::filter_active_runs(ctx.session.as_ref(), &all_ids).await?;
            for rid in active_ids {
                collector.refresh_run(rid).await?;
            }
        }
    }

    let output = metrics.export_metrics();
    if output.is_empty() {
        println!("No metrics available.");
    } else {
        print!("{output}");
    }

    Ok(())
}
