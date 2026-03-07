//! `stratum submit` command handler.

use stratum_core::TaskDispatch;
use stratum_orchestrator::RfbmqDispatcher;
use stratum_types::{DispatchOptions, TaskPriority};

use crate::config::StratumConfig;

pub async fn execute(
    task: &str,
    priority: &str,
    tags: &[String],
    config: &StratumConfig,
) -> anyhow::Result<()> {
    let orch_config = config.orchestrator_config();
    let dispatch =
        RfbmqDispatcher::init_or_open(&orch_config.queue_root, orch_config.max_pending_per_queue)?;

    let priority = match priority.to_lowercase().as_str() {
        "critical" => TaskPriority::Critical,
        "high" => TaskPriority::High,
        "low" => TaskPriority::Low,
        _ => TaskPriority::Normal,
    };

    let body = serde_json::json!({
        "goal": task,
        "priority": format!("{priority:?}"),
        "tags": tags,
    })
    .to_string();

    let opts = DispatchOptions {
        priority,
        tags: tags.to_vec(),
        ..Default::default()
    };

    let msg_id = dispatch.enqueue(&body, opts)?;
    eprintln!("Task submitted: {msg_id}");
    eprintln!("Goal: {task}");
    if !tags.is_empty() {
        eprintln!("Tags: {}", tags.join(", "));
    }

    Ok(())
}
