mod cli;
mod commands;
mod config;
mod llm_adapter;
mod run_loop;
mod turn_executor;
mod wiring;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Commands};
use config::StratumConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let config = StratumConfig::load(cli.config.as_ref())?;

    match &cli.command {
        Commands::Run { task } => {
            commands::run::execute(task, config).await?;
        }
        Commands::Resume { run_id } => {
            commands::resume::execute(run_id, config).await?;
        }
        Commands::Status { run_id } => {
            commands::status::execute(run_id.as_deref(), &config).await?;
        }
        Commands::Trajectory { run_id } => {
            commands::trajectory::execute(run_id, &config).await?;
        }
        Commands::Export { run_id, format } => {
            commands::export::execute(run_id, format, &config).await?;
        }
        Commands::Gates => {
            commands::gates::execute(&config).await?;
        }
        Commands::Decide { run_id, decision } => {
            commands::decide::execute(run_id, decision, &config).await?;
        }
        Commands::Queue { action } => {
            commands::queue::execute(action, &config).await?;
        }
    }

    Ok(())
}
