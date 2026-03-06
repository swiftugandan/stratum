use stratum_core::TrajectoryStore;

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(run_id: &str, config: &StratumConfig) -> anyhow::Result<()> {
    let run_id = super::parse_run_id(run_id)?;
    let ctx = QueryContext::build(config)?;

    let events = ctx
        .trajectory
        .query_events(Some(run_id), None, None, None)
        .await?;

    if events.is_empty() {
        println!("No events found for run {run_id}.");
        return Ok(());
    }

    println!("{:<20} {:<25} {:<15}", "TIMESTAMP", "EVENT TYPE", "STRATUM");
    for event in &events {
        println!(
            "{:<20} {:<25} {:<15}",
            event.timestamp.format("%H:%M:%S%.3f"),
            format!("{:?}", event.event_type),
            format!("{:?}", event.stratum_layer),
        );
    }
    println!("\nTotal: {} events", events.len());

    Ok(())
}
