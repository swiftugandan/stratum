//! Core daemon implementation.
//!
//! Watches the rfbmq `pending/` directory via the `notify` crate (FSEvents on macOS,
//! inotify on Linux). On file creation, dequeues tasks and spawns runs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use stratum_core::TaskDispatch;
use stratum_orchestrator::RfbmqDispatcher;
use stratum_types::*;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::config::StratumConfig;

/// Configuration for the daemon.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub max_concurrent_runs: usize,
    pub pid_file: PathBuf,
    #[allow(dead_code)]
    pub log_file: PathBuf,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 4,
            pid_file: PathBuf::from(".stratum/daemon.pid"),
            log_file: PathBuf::from(".stratum/daemon.log"),
        }
    }
}

/// The persistent daemon loop.
pub struct DaemonLoop {
    config: StratumConfig,
    daemon_config: DaemonConfig,
    dispatch: Arc<RfbmqDispatcher>,
    active_runs: Arc<Mutex<HashMap<RunId, JoinHandle<()>>>>,
    shutdown: Arc<AtomicBool>,
}

impl DaemonLoop {
    pub fn new(
        config: StratumConfig,
        daemon_config: DaemonConfig,
        dispatch: Arc<RfbmqDispatcher>,
    ) -> Self {
        Self {
            config,
            daemon_config,
            dispatch,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Run the daemon. Blocks until shutdown signal received.
    pub async fn run(&self) -> anyhow::Result<()> {
        // 1. Write PID file
        self.write_pid_file()?;
        tracing::info!(
            "daemon started, pid file: {}",
            self.daemon_config.pid_file.display()
        );
        eprintln!("Stratum daemon started (pid: {})", std::process::id());

        // 2. Set up signal handlers
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("received shutdown signal");
            eprintln!("\nShutting down...");
            shutdown.store(true, Ordering::SeqCst);
        });

        // 3. Set up filesystem watcher
        let (tx, mut rx) = mpsc::channel::<()>(100);
        let watch_path = self.config.orchestrator_config().queue_root.join("pending");
        std::fs::create_dir_all(&watch_path)?;

        let _watcher = self.setup_watcher(&watch_path, tx.clone())?;

        // 4. Drain any existing pending tasks
        self.drain_pending().await;

        // 5. Event loop
        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }

            tokio::select! {
                Some(()) = rx.recv() => {
                    self.drain_pending().await;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    // Periodic check: clean up completed runs and check for tasks
                    self.cleanup_completed().await;
                    self.drain_pending().await;
                }
            }
        }

        // 6. Graceful shutdown
        self.graceful_shutdown().await;
        self.remove_pid_file();

        eprintln!("Daemon stopped.");
        Ok(())
    }

    fn setup_watcher(
        &self,
        watch_path: &std::path::Path,
        tx: mpsc::Sender<()>,
    ) -> anyhow::Result<RecommendedWatcher> {
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Create(_)) {
                    let _ = tx.blocking_send(());
                }
            }
        })?;

        watcher.watch(watch_path, RecursiveMode::NonRecursive)?;
        tracing::info!("watching {}", watch_path.display());
        Ok(watcher)
    }

    /// Dequeue and dispatch all pending tasks up to the concurrency limit.
    async fn drain_pending(&self) {
        loop {
            let active_count = self.active_runs.lock().await.len();
            if active_count >= self.daemon_config.max_concurrent_runs {
                tracing::debug!(active_count, "at concurrency limit, waiting");
                break;
            }

            let task = match self.dispatch.dequeue() {
                Ok(Some(task)) => task,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(error = %e, "failed to dequeue task");
                    break;
                }
            };

            let run_id = uuid::Uuid::new_v4();
            tracing::info!(run_id = %run_id, task_id = %task.id, "dequeued task");
            eprintln!("Task dequeued: {} → run {run_id}", task.id);

            let config = self.config.clone();
            let dispatch = self.dispatch.clone();
            let active = self.active_runs.clone();

            let handle = tokio::spawn(async move {
                match run_task(&config, &task).await {
                    Ok(()) => {
                        tracing::info!(task_id = %task.id, "task completed");
                        eprintln!("Task completed: {}", task.id);
                        if let Err(e) = dispatch.complete(&task) {
                            tracing::error!(error = %e, "failed to mark task complete");
                        }
                    }
                    Err(e) => {
                        tracing::error!(task_id = %task.id, error = %e, "task failed");
                        eprintln!("Task failed: {} — {e}", task.id);
                        if let Err(e) = dispatch.fail(&task) {
                            tracing::error!(error = %e, "failed to mark task failed");
                        }
                    }
                }
                active.lock().await.remove(&run_id);
            });

            self.active_runs.lock().await.insert(run_id, handle);
        }
    }

    /// Clean up completed run handles.
    async fn cleanup_completed(&self) {
        let mut active = self.active_runs.lock().await;
        let done_ids: Vec<RunId> = active
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(&id, _)| id)
            .collect();
        for id in done_ids {
            active.remove(&id);
        }
    }

    /// Wait for active runs to complete (with timeout).
    async fn graceful_shutdown(&self) {
        let timeout = std::time::Duration::from_secs(60);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let count = self.active_runs.lock().await.len();
            if count == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(count, "shutdown timeout, {} runs still active", count);
                eprintln!("Warning: {count} runs still active after timeout");
                break;
            }
            eprintln!("Waiting for {count} active run(s)...");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    fn write_pid_file(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.daemon_config.pid_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let pid = std::process::id();
        std::fs::write(&self.daemon_config.pid_file, pid.to_string())?;
        Ok(())
    }

    fn remove_pid_file(&self) {
        let _ = std::fs::remove_file(&self.daemon_config.pid_file);
    }
}

/// Parse a task body JSON into a goal string.
fn parse_task_goal(body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(goal) = json.get("goal").and_then(|v| v.as_str()) {
            return goal.to_string();
        }
    }
    // Fall back to using the body as the goal
    body.to_string()
}

/// Run a single task from the queue.
async fn run_task(config: &StratumConfig, task: &ClaimedTask) -> anyhow::Result<()> {
    let goal = parse_task_goal(&task.body);
    tracing::info!(task_id = %task.id, goal = %goal, "running task");

    // Build AppContext for this run, with auto_approve_global enabled for daemon mode
    let ctx = crate::wiring::AppContext::build_daemon(config.clone())?;

    // Run the task using the queue-specific entry point (daemon system prompt, no initialiser)
    crate::run_loop::run_from_queue(&goal, &ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_task_goal_json() {
        let body = r#"{"goal": "build a hello world", "priority": "normal"}"#;
        assert_eq!(parse_task_goal(body), "build a hello world");
    }

    #[test]
    fn parse_task_goal_plain_text() {
        let body = "just a plain task";
        assert_eq!(parse_task_goal(body), "just a plain task");
    }

    #[test]
    fn daemon_config_defaults() {
        let cfg = DaemonConfig::default();
        assert_eq!(cfg.max_concurrent_runs, 4);
        assert!(cfg.pid_file.ends_with("daemon.pid"));
    }

    #[test]
    fn pid_file_write_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("test.pid");
        let daemon = DaemonLoop {
            config: StratumConfig::default(),
            daemon_config: DaemonConfig {
                pid_file: pid_file.clone(),
                ..Default::default()
            },
            dispatch: Arc::new(RfbmqDispatcher::init(dir.path().join("q").as_path(), 100).unwrap()),
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        daemon.write_pid_file().unwrap();
        assert!(pid_file.exists());
        let content = std::fs::read_to_string(&pid_file).unwrap();
        assert_eq!(content, std::process::id().to_string());
        daemon.remove_pid_file();
        assert!(!pid_file.exists());
    }
}
