use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "stratum", about = "Model-agnostic agent harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Path to configuration file (default: stratum.yaml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start a new agent run with the given task
    Run {
        /// The task goal for the agent
        task: String,
    },
    /// Resume a paused or checkpointed run
    Resume {
        /// The run ID to resume
        run_id: String,
    },
    /// Show the status of a run (or all runs)
    Status {
        /// Optional run ID; shows all runs if omitted
        run_id: Option<String>,
    },
    /// Show trajectory events for a run
    Trajectory {
        /// The run ID to query
        run_id: String,
    },
    /// Export trajectory events for a run
    Export {
        /// The run ID to export
        run_id: String,
        /// Export format: jsonl, csv, or replay
        #[arg(long, default_value = "jsonl")]
        format: String,
    },
    /// List pending HITL gates
    Gates,
    /// Record a decision on a HITL gate
    Decide {
        /// The run ID with the pending gate
        run_id: String,
        /// Decision: approve, abort, modify:<context>, redirect:<goal>
        decision: String,
    },
    /// Queue management commands
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
    /// Show current metrics (Prometheus format)
    Metrics {
        /// Optional run ID to show metrics for a specific run
        run_id: Option<String>,
    },
    /// Live terminal dashboard showing metrics
    Dashboard,
    /// Start Prometheus metrics HTTP server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "9090")]
        port: u16,
    },
}

#[derive(Subcommand)]
pub enum QueueAction {
    /// Show current queue depth
    Depth,
}
