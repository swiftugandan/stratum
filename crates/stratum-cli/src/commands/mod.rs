pub mod decide;
pub mod export;
pub mod gates;
pub mod queue;
pub mod resume;
pub mod run;
pub mod status;
pub mod trajectory;

use stratum_types::RunId;

/// Parse a run ID string into a UUID, with a user-friendly error.
pub fn parse_run_id(s: &str) -> anyhow::Result<RunId> {
    s.parse()
        .map_err(|_| anyhow::anyhow!("invalid run ID: {s}"))
}
