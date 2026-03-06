use stratum_core::HitlController;

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(config: &StratumConfig) -> anyhow::Result<()> {
    let ctx = QueryContext::build(config)?;
    let gates = ctx.hitl.pending_gates().await?;

    if gates.is_empty() {
        println!("No pending gates.");
        return Ok(());
    }

    println!("{:<38} {:<15} {:<30}", "RUN ID", "CATEGORY", "ACTION");
    for gate in &gates {
        println!(
            "{:<38} {:<15} {:<30}",
            gate.run_id, gate.gate_category, gate.action_attempted,
        );
    }
    println!("\nTotal: {} pending gates", gates.len());

    Ok(())
}
