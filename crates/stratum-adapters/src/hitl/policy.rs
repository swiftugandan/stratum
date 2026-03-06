//! Gate policy engine — maps gate categories to actions based on `HitlPolicy`.

use stratum_types::{GateCategory, HitlPolicy};

/// Action the harness should take for a given gate category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    /// Block execution and wait for human decision (AlwaysAsk).
    Block,
    /// Notify and offer the human a chance to intervene (NotifyAndOption).
    NotifyWithOption,
    /// Notify only, continue execution (Notify).
    NotifyOnly,
    /// Auto-approve, no human involvement (Auto).
    AutoApprove,
}

/// Evaluates gate categories against an `HitlPolicy` to determine the action.
#[derive(Debug)]
pub struct GatePolicyEngine;

impl GatePolicyEngine {
    /// Evaluate a gate category against the given policy.
    pub fn evaluate(category: GateCategory, policy: &HitlPolicy) -> GateAction {
        use stratum_types::GatePolicy;

        match policy.policy_for(category) {
            GatePolicy::AlwaysAsk => GateAction::Block,
            GatePolicy::NotifyAndOption => GateAction::NotifyWithOption,
            GatePolicy::Notify => GateAction::NotifyOnly,
            GatePolicy::Auto => GateAction::AutoApprove,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stratum_types::{GatePolicy, HitlPolicy};

    #[test]
    fn test_destructive_always_ask() {
        let policy = HitlPolicy::default();
        assert_eq!(
            GatePolicyEngine::evaluate(GateCategory::Destructive, &policy),
            GateAction::Block
        );
    }

    #[test]
    fn test_drift_notify_and_option() {
        let policy = HitlPolicy::default();
        assert_eq!(
            GatePolicyEngine::evaluate(GateCategory::Drift, &policy),
            GateAction::NotifyWithOption
        );
    }

    #[test]
    fn test_budget_notify() {
        let policy = HitlPolicy::default();
        assert_eq!(
            GatePolicyEngine::evaluate(GateCategory::Budget, &policy),
            GateAction::NotifyOnly
        );
    }

    #[test]
    fn test_auto_policy() {
        let policy = HitlPolicy {
            destructive: GatePolicy::Auto,
            ..Default::default()
        };
        assert_eq!(
            GatePolicyEngine::evaluate(GateCategory::Destructive, &policy),
            GateAction::AutoApprove
        );
    }

    #[test]
    fn test_scheduled_with_interval() {
        let policy = HitlPolicy {
            scheduled_interval: Some(10),
            ..Default::default()
        };
        assert_eq!(
            GatePolicyEngine::evaluate(GateCategory::Scheduled, &policy),
            GateAction::NotifyWithOption
        );
    }

    #[test]
    fn test_scheduled_without_interval() {
        let policy = HitlPolicy {
            scheduled_interval: None,
            ..Default::default()
        };
        assert_eq!(
            GatePolicyEngine::evaluate(GateCategory::Scheduled, &policy),
            GateAction::AutoApprove
        );
    }

    #[test]
    fn test_all_known_categories() {
        let policy = HitlPolicy::default();
        for cat in &[
            GateCategory::Destructive,
            GateCategory::Irreversible,
            GateCategory::TrustEscalation,
            GateCategory::Ambiguity,
            GateCategory::Drift,
            GateCategory::Budget,
            GateCategory::Scheduled,
        ] {
            let _ = GatePolicyEngine::evaluate(*cat, &policy);
        }
    }
}
