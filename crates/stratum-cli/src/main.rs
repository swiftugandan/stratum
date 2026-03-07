//! Stratum CLI binary: daemon-only autonomous agent harness.

mod cli;
mod config;
mod daemon;
mod run_loop;
mod turn_executor;
mod wiring;

use std::sync::Arc;

use clap::Parser;
use stratum_core::ports::TaskDispatch;
use stratum_core::*;
use stratum_engine::dispatch::RfbmqDispatcher;

use cli::{Cli, Commands};
use config::StratumConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config = StratumConfig::load(cli.config.as_ref())?;

    match cli.command {
        Commands::Start => cmd_start(config).await,
        Commands::Submit {
            goal,
            priority,
            tag,
        } => cmd_submit(config, goal, priority, tag),
        Commands::Stop => cmd_stop(config),
        Commands::Status => cmd_status(config),
    }
}

async fn cmd_start(config: StratumConfig) -> anyhow::Result<()> {
    config.require_api_key()?;
    let ctx = Arc::new(wiring::AppContext::build(config)?);
    let daemon = daemon::DaemonLoop::new(ctx);
    daemon.run().await
}

fn cmd_submit(
    config: StratumConfig,
    goal: String,
    priority: String,
    tags: Vec<String>,
) -> anyhow::Result<()> {
    let queue_root = config.queue_root();
    std::fs::create_dir_all(&queue_root)?;
    let dispatch = RfbmqDispatcher::init_or_open(&queue_root, 10000)?;

    let task_priority = match priority.to_lowercase().as_str() {
        "critical" => TaskPriority::Critical,
        "high" => TaskPriority::High,
        "normal" => TaskPriority::Normal,
        "low" => TaskPriority::Low,
        _ => {
            eprintln!("Unknown priority '{priority}', using normal");
            TaskPriority::Normal
        }
    };

    let body = serde_json::json!({"goal": goal}).to_string();
    let opts = DispatchOptions {
        priority: task_priority,
        tags,
        ..Default::default()
    };

    let id = dispatch.enqueue(&body, opts)?;
    eprintln!("Task submitted: {id}");
    Ok(())
}

fn cmd_stop(config: StratumConfig) -> anyhow::Result<()> {
    let pid_file = config.pid_file();
    match daemon::read_pid(&pid_file) {
        Some(pid) => {
            eprintln!("Sending SIGTERM to daemon (pid: {pid})");
            // Use nix or raw syscall; for simplicity, shell out to kill
            let status = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
            match status {
                Ok(s) if s.success() => eprintln!("Signal sent."),
                Ok(s) => eprintln!("kill exited with: {s}"),
                Err(e) => eprintln!("Failed to send signal: {e}"),
            }
            Ok(())
        }
        None => {
            eprintln!(
                "No running daemon found (no PID file at {})",
                pid_file.display()
            );
            Ok(())
        }
    }
}

fn cmd_status(config: StratumConfig) -> anyhow::Result<()> {
    // Check daemon status
    let pid_file = config.pid_file();
    match daemon::read_pid(&pid_file) {
        Some(pid) => eprintln!("Daemon running (pid: {pid})"),
        None => eprintln!("Daemon not running"),
    }

    // Check queue depth
    let queue_root = config.queue_root();
    if queue_root.exists() {
        match RfbmqDispatcher::init_or_open(&queue_root, 10000) {
            Ok(dispatch) => {
                let depth = dispatch.depth().unwrap_or(0);
                let ready = dispatch.list_ready().unwrap_or_default();
                eprintln!("Queue depth: {depth} total, {} ready", ready.len());
            }
            Err(e) => eprintln!("Could not read queue: {e}"),
        }
    } else {
        eprintln!("Queue not initialized");
    }

    Ok(())
}
