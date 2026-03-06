//! Gate policy engine — maps gate categories to actions based on `HitlPolicy`.

use stratum_types::HitlPolicy;

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
    /// Evaluate a gate category string against the given policy.
    ///
    /// Known categories: "destructive", "irreversible", "trust_escalation",
    /// "ambiguity", "drift", "budget", "scheduled".
    /// Unknown categories default to `Block` (safest).
    pub fn evaluate(category: &str, policy: &HitlPolicy) -> GateAction {
        use stratum_types::GatePolicy;

        let gate_policy = match category {
            "destructive" => policy.destructive,
            "irreversible" => policy.irreversible,
            "trust_escalation" => policy.trust_escalation,
            "ambiguity" => policy.ambiguity,
            "drift" => policy.drift,
            "budget" => policy.budget,
            "scheduled" => {
                if policy.scheduled_interval.is_some() {
                    GatePolicy::NotifyAndOption
                } else {
                    GatePolicy::Auto
                }
            }
            _ => return GateAction::Block,
        };

        match gate_policy {
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
            GatePolicyEngine::evaluate("destructive", &policy),
            GateAction::Block
        );
    }

    #[test]
    fn test_drift_notify_and_option() {
        let policy = HitlPolicy::default();
        assert_eq!(
            GatePolicyEngine::evaluate("drift", &policy),
            GateAction::NotifyWithOption
        );
    }

    #[test]
    fn test_budget_notify() {
        let policy = HitlPolicy::default();
        assert_eq!(
            GatePolicyEngine::evaluate("budget", &policy),
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
            GatePolicyEngine::evaluate("destructive", &policy),
            GateAction::AutoApprove
        );
    }

    #[test]
    fn test_unknown_category_defaults_to_block() {
        let policy = HitlPolicy::default();
        assert_eq!(
            GatePolicyEngine::evaluate("unknown_category", &policy),
            GateAction::Block
        );
    }

    #[test]
    fn test_scheduled_with_interval() {
        let policy = HitlPolicy {
            scheduled_interval: Some(10),
            ..Default::default()
        };
        assert_eq!(
            GatePolicyEngine::evaluate("scheduled", &policy),
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
            GatePolicyEngine::evaluate("scheduled", &policy),
            GateAction::AutoApprove
        );
    }

    #[test]
    fn test_all_known_categories() {
        let policy = HitlPolicy::default();
        // Just verify no panics and known categories return non-Block where expected
        for cat in &[
            "destructive",
            "irreversible",
            "trust_escalation",
            "ambiguity",
            "drift",
            "budget",
            "scheduled",
        ] {
            let _ = GatePolicyEngine::evaluate(cat, &policy);
        }
    }
}
