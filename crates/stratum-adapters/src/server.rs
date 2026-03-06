//! Optional HTTP server exposing `/metrics` in Prometheus text format.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use stratum_core::MetricsExporter;

use crate::metrics::InMemoryMetrics;

/// Shared state for the metrics server.
#[derive(Clone)]
struct MetricsState {
    metrics: Arc<InMemoryMetrics>,
}

async fn metrics_handler(State(state): State<MetricsState>) -> impl IntoResponse {
    let body = state.metrics.export_metrics();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Start the metrics HTTP server on the given address.
///
/// Returns a handle that resolves when the server shuts down.
/// Use the returned `SocketAddr` to know the actual bound port (useful when binding to port 0).
pub async fn start_metrics_server(
    metrics: Arc<InMemoryMetrics>,
    addr: SocketAddr,
) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let state = MetricsState { metrics };

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "metrics server error");
        }
    });

    Ok((local_addr, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn test_metrics_endpoint_returns_200() {
        let metrics = Arc::new(InMemoryMetrics::new());
        metrics.gauge("test_gauge", 42.0, &[("env", "test")]);
        metrics.counter("test_counter", &[("method", "GET")]);

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let (bound_addr, handle) = start_metrics_server(metrics, addr).await.unwrap();

        let url = format!("http://{bound_addr}/metrics");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);

        let body = resp.text().await.unwrap();
        assert!(body.contains("test_gauge{env=\"test\"} 42"));
        assert!(body.contains("test_counter{method=\"GET\"} 1"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let metrics = Arc::new(InMemoryMetrics::new());
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let (bound_addr, handle) = start_metrics_server(metrics, addr).await.unwrap();

        let url = format!("http://{bound_addr}/health");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);

        let body = resp.text().await.unwrap();
        assert_eq!(body, "ok");

        handle.abort();
    }
}
