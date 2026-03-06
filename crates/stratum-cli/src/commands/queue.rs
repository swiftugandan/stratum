use stratum_core::TaskDispatch;

use crate::cli::QueueAction;
use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(action: &QueueAction, config: &StratumConfig) -> anyhow::Result<()> {
    let ctx = QueryContext::build(config)?;

    match action {
        QueueAction::Depth => {
            let depth = ctx.dispatch.depth()?;
            println!("Queue depth: {depth}");
        }
    }

    Ok(())
}
