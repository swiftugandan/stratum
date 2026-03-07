//! CLI definition: 4 commands (start, submit, stop, status).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "stratum", about = "Stratum autonomous agent harness")]
pub struct Cli {
    /// Path to config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start the daemon (foreground, watches rfbmq queue)
    Start,

    /// Submit a task to the queue
    Submit {
        /// Task goal
        goal: String,

        /// Task priority
        #[arg(long, default_value = "normal")]
        priority: String,

        /// Tags for the task
        #[arg(long)]
        tag: Vec<String>,
    },

    /// Stop a running daemon
    Stop,

    /// Show running/queued task status
    Status,
}
