//! DaemonLoop: watches rfbmq queue, dequeues tasks, runs agent loops concurrently.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
#[allow(unused_imports)]
use stratum_core::ports::TaskDispatch;
use stratum_core::*;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::wiring::AppContext;

pub struct DaemonLoop {
    ctx: Arc<AppContext>,
    active_runs: Arc<Mutex<HashMap<RunId, JoinHandle<()>>>>,
    shutdown: Arc<AtomicBool>,
}

impl DaemonLoop {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self {
            ctx,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        self.write_pid_file()?;
        tracing::info!("daemon started (pid: {})", std::process::id());
        eprintln!("Stratum daemon started (pid: {})", std::process::id());

        // Signal handler
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("received shutdown signal");
            eprintln!("\nShutting down...");
            shutdown.store(true, Ordering::SeqCst);
        });

        // Filesystem watcher on pending/
        let (tx, mut rx) = mpsc::channel::<()>(100);
        let watch_path = self.ctx.config.queue_root().join("pending");
        std::fs::create_dir_all(&watch_path)?;
        let _watcher = self.setup_watcher(&watch_path, tx)?;

        // Drain existing pending tasks
        self.drain_pending().await;

        // Event loop
        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }

            tokio::select! {
                Some(()) = rx.recv() => {
                    self.drain_pending().await;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    self.cleanup_completed().await;
                    self.drain_pending().await;
                }
            }
        }

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

    async fn drain_pending(&self) {
        loop {
            let active_count = self.active_runs.lock().await.len();
            if active_count >= self.ctx.config.max_concurrent_runs {
                tracing::debug!(active_count, "at concurrency limit");
                break;
            }

            let task = match self.ctx.dispatch.dequeue() {
                Ok(Some(task)) => task,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(error = %e, "failed to dequeue task");
                    break;
                }
            };

            let run_id = uuid::Uuid::new_v4();
            tracing::info!(run_id = %run_id, task_id = %task.id, "dequeued task");
            eprintln!("Task dequeued: {} -> run {run_id}", task.id);

            let ctx = Arc::clone(&self.ctx);
            let dispatch = Arc::clone(&self.ctx.dispatch);
            let active = self.active_runs.clone();

            let handle = tokio::spawn(async move {
                let goal = parse_task_goal(&task.body);
                match crate::run_loop::run_from_queue(&goal, &ctx).await {
                    Ok(()) => {
                        tracing::info!(task_id = %task.id, "task completed");
                        eprintln!("Task completed: {}", task.id);
                        if let Err(e) = dispatch.complete(&task) {
                            tracing::error!(error = %e, "failed to mark task complete");
                        }
                    }
                    Err(e) => {
                        tracing::error!(task_id = %task.id, error = %e, "task failed");
                        eprintln!("Task failed: {} -- {e}", task.id);
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

    async fn cleanup_completed(&self) {
        let mut active = self.active_runs.lock().await;
        let done: Vec<RunId> = active
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(&id, _)| id)
            .collect();
        for id in done {
            active.remove(&id);
        }
    }

    async fn graceful_shutdown(&self) {
        let timeout = std::time::Duration::from_secs(60);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let count = self.active_runs.lock().await.len();
            if count == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(count, "shutdown timeout, runs still active");
                eprintln!("Warning: {count} runs still active after timeout");
                break;
            }
            eprintln!("Waiting for {count} active run(s)...");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    fn write_pid_file(&self) -> anyhow::Result<()> {
        let pid_file = self.ctx.config.pid_file();
        if let Some(parent) = pid_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&pid_file, std::process::id().to_string())?;
        Ok(())
    }

    fn remove_pid_file(&self) {
        let _ = std::fs::remove_file(self.ctx.config.pid_file());
    }
}

fn parse_task_goal(body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(goal) = json.get("goal").and_then(|v| v.as_str()) {
            return goal.to_string();
        }
    }
    body.to_string()
}

/// Check if a daemon is currently running by reading the PID file.
pub fn read_pid(pid_file: &PathBuf) -> Option<u32> {
    std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
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
}
