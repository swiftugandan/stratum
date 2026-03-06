use stratum_core::TrajectoryStore;
use stratum_types::ExportFormat;

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(run_id: &str, format: &str, config: &StratumConfig) -> anyhow::Result<()> {
    let run_id = super::parse_run_id(run_id)?;
    let ctx = QueryContext::build(config)?;

    let export_format = match format {
        "jsonl" => ExportFormat::Jsonl,
        "csv" => ExportFormat::Csv,
        "replay" => ExportFormat::Replay,
        _ => anyhow::bail!("unsupported export format: {format} (use jsonl, csv, or replay)"),
    };

    let data = ctx.trajectory.export(run_id, export_format).await?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    std::io::Write::write_all(&mut handle, &data)?;

    Ok(())
}
