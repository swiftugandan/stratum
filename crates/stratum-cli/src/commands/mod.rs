pub mod dashboard;
pub mod decide;
pub mod export;
pub mod gates;
pub mod metrics;
pub mod queue;
pub mod resume;
pub mod run;
pub mod serve;
pub mod status;
pub mod trajectory;

use stratum_core::{SessionManager, TrajectoryStore};
use stratum_types::{RunId, RunState};

/// Parse a run ID string into a UUID, with a user-friendly error.
pub fn parse_run_id(s: &str) -> anyhow::Result<RunId> {
    s.parse()
        .map_err(|_| anyhow::anyhow!("invalid run ID: {s}"))
}

/// Discover all unique run IDs from the trajectory store.
pub async fn discover_run_ids<T: TrajectoryStore>(trajectory: &T) -> anyhow::Result<Vec<RunId>> {
    let events = trajectory
        .query_events(None, None, None, None)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut run_ids: Vec<_> = events.iter().map(|e| e.run_id).collect();
    run_ids.sort();
    run_ids.dedup();
    Ok(run_ids)
}

/// Filter run IDs to only those in an active state (Running, Initialising, Paused).
pub async fn filter_active_runs<S: SessionManager>(
    session: &S,
    run_ids: &[RunId],
) -> anyhow::Result<Vec<RunId>> {
    let mut active = Vec::new();
    for &rid in run_ids {
        if let Some(run) = session
            .get_run(rid)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            if matches!(
                run.state,
                RunState::Running | RunState::Initialising | RunState::Paused
            ) {
                active.push(rid);
            }
        }
    }
    Ok(active)
}
