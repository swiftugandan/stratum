//! `stratum serve` command — start the Prometheus metrics HTTP server.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use stratum_adapters::{start_metrics_server, InMemoryMetrics, MetricsCollector};

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(port: u16, config: &StratumConfig) -> anyhow::Result<()> {
    let ctx = QueryContext::build(config)?;

    let metrics = Arc::new(InMemoryMetrics::new());
    let collector = Arc::new(MetricsCollector::new(
        Arc::clone(&metrics),
        Arc::clone(&ctx.trajectory),
        Arc::clone(&ctx.hitl),
    ));

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let (bound_addr, server_handle) = start_metrics_server(Arc::clone(&metrics), addr).await?;
    eprintln!("Metrics server listening on http://{bound_addr}/metrics");
    eprintln!("Health check: http://{bound_addr}/health");
    eprintln!("Press Ctrl+C to stop.");

    // Background refresh loop
    let refresh_handle = {
        let collector = Arc::clone(&collector);
        let trajectory = Arc::clone(&ctx.trajectory);
        let session = Arc::clone(&ctx.session);
        tokio::spawn(async move {
            loop {
                if let Err(e) = collector.refresh_global().await {
                    tracing::warn!(error = %e, "failed to refresh global metrics");
                }

                if let Ok(all_ids) = super::discover_run_ids(trajectory.as_ref()).await {
                    if let Ok(active_ids) =
                        super::filter_active_runs(session.as_ref(), &all_ids).await
                    {
                        for rid in active_ids {
                            if let Err(e) = collector.refresh_run(rid).await {
                                tracing::warn!(run_id = %rid, error = %e, "failed to refresh run metrics");
                            }
                        }
                    }
                }

                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        })
    };

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down...");
    refresh_handle.abort();
    server_handle.abort();

    Ok(())
}
