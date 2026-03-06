use stratum_core::{SessionManager, TrajectoryStore};

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(run_id: Option<&str>, config: &StratumConfig) -> anyhow::Result<()> {
    let ctx = QueryContext::build(config)?;

    match run_id {
        Some(id) => {
            let run_id = super::parse_run_id(id)?;
            let run = ctx
                .session
                .get_run(run_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("run {run_id} not found"))?;

            println!("Run: {}", run.id);
            println!("State: {:?}", run.state);
            println!("Model: {}", run.model_ref);
            println!("Trust: {:?}", run.trust_level);
            println!("Created: {}", run.created_at);

            // Show event count
            let events = ctx
                .trajectory
                .query_events(Some(run_id), None, None, None)
                .await?;
            println!("Events: {}", events.len());
        }
        None => {
            // List all runs — query trajectory for distinct run IDs
            let events = ctx.trajectory.query_events(None, None, None, None).await?;
            let mut run_ids: Vec<_> = events.iter().map(|e| e.run_id).collect();
            run_ids.sort();
            run_ids.dedup();

            if run_ids.is_empty() {
                println!("No runs found.");
            } else {
                println!("{:<38} {:<15} {:<20}", "RUN ID", "STATE", "CREATED");
                for rid in run_ids {
                    if let Some(run) = ctx.session.get_run(rid).await? {
                        println!(
                            "{:<38} {:<15} {:<20}",
                            run.id,
                            format!("{:?}", run.state),
                            run.created_at.format("%Y-%m-%d %H:%M:%S")
                        );
                    }
                }
            }
        }
    }

    Ok(())
}
