//! `stratum daemon` command handler.

use std::sync::Arc;

use stratum_orchestrator::RfbmqDispatcher;

use crate::config::StratumConfig;
use crate::daemon::{DaemonConfig, DaemonLoop};

pub async fn execute(config: StratumConfig) -> anyhow::Result<()> {
    config.require_api_key()?;

    let orch_config = config.orchestrator_config();
    let dispatch = Arc::new(RfbmqDispatcher::init_or_open(
        &orch_config.queue_root,
        orch_config.max_pending_per_queue,
    )?);

    let daemon_config = DaemonConfig {
        pid_file: config.data_dir.join("daemon.pid"),
        log_file: config.data_dir.join("daemon.log"),
        ..Default::default()
    };

    let daemon = DaemonLoop::new(config, daemon_config, dispatch);
    daemon.run().await
}
