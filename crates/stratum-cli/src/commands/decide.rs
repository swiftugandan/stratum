use stratum_core::HitlController;
use stratum_types::HitlDecision;

use crate::config::StratumConfig;
use crate::wiring::QueryContext;

pub async fn execute(
    run_id: &str,
    decision_str: &str,
    config: &StratumConfig,
) -> anyhow::Result<()> {
    let run_id = super::parse_run_id(run_id)?;
    let ctx = QueryContext::build(config)?;

    let decision = parse_decision(decision_str)?;
    ctx.hitl.record_decision(run_id, decision).await?;
    println!("Decision recorded for run {run_id}.");

    Ok(())
}

fn parse_decision(s: &str) -> anyhow::Result<HitlDecision> {
    if s == "approve" {
        return Ok(HitlDecision::Approve);
    }
    if s == "abort" {
        return Ok(HitlDecision::Abort);
    }
    if let Some(context) = s.strip_prefix("modify:") {
        return Ok(HitlDecision::Modify {
            context: context.to_string(),
        });
    }
    if let Some(goal) = s.strip_prefix("redirect:") {
        return Ok(HitlDecision::Redirect {
            new_goal: goal.to_string(),
        });
    }
    anyhow::bail!(
        "invalid decision: {s}\nExpected: approve, abort, modify:<context>, or redirect:<goal>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_approve() {
        assert!(matches!(
            parse_decision("approve").unwrap(),
            HitlDecision::Approve
        ));
    }

    #[test]
    fn parse_abort() {
        assert!(matches!(
            parse_decision("abort").unwrap(),
            HitlDecision::Abort
        ));
    }

    #[test]
    fn parse_modify() {
        match parse_decision("modify:use different approach").unwrap() {
            HitlDecision::Modify { context } => {
                assert_eq!(context, "use different approach");
            }
            _ => panic!("expected Modify"),
        }
    }

    #[test]
    fn parse_redirect() {
        match parse_decision("redirect:new goal here").unwrap() {
            HitlDecision::Redirect { new_goal } => {
                assert_eq!(new_goal, "new goal here");
            }
            _ => panic!("expected Redirect"),
        }
    }

    #[test]
    fn parse_invalid() {
        assert!(parse_decision("unknown").is_err());
    }
}
