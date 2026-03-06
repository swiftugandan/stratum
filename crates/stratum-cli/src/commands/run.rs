use crate::config::StratumConfig;
use crate::run_loop;
use crate::wiring::AppContext;

pub async fn execute(task: &str, config: StratumConfig) -> anyhow::Result<()> {
    config.require_api_key()?;
    let ctx = AppContext::build(config)?;
    run_loop::run_new(task, &ctx).await
}
