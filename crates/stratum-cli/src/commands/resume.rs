use crate::config::StratumConfig;
use crate::run_loop;
use crate::wiring::AppContext;

pub async fn execute(run_id: &str, config: StratumConfig) -> anyhow::Result<()> {
    config.require_api_key()?;
    let run_id = super::parse_run_id(run_id)?;
    let ctx = AppContext::build(config)?;
    run_loop::resume_run(run_id, &ctx).await
}
