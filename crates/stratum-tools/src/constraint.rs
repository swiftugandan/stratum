//! Default constraint enforcer implementation.

use std::sync::Arc;

use async_trait::async_trait;
use stratum_core::{ConstraintEnforcer, ToolGateway};
use stratum_types::*;

use crate::error::ToolError;

/// Default constraint enforcer that invokes constraint check tools via the gateway.
pub struct DefaultConstraintEnforcer<G: ToolGateway> {
    gateway: Arc<G>,
    constraints: Vec<ConstraintDefinition>,
}

impl<G: ToolGateway> DefaultConstraintEnforcer<G> {
    pub fn new(gateway: Arc<G>, constraints: Vec<ConstraintDefinition>) -> Self {
        Self {
            gateway,
            constraints,
        }
    }

    fn parse_constraint_result(
        &self,
        constraint_name: &str,
        tool_result: &ToolResult,
    ) -> Result<ConstraintResult, ToolError> {
        if tool_result.status != ToolResultStatus::Success {
            return Err(ToolError::Executor(format!(
                "constraint check tool failed for `{constraint_name}`: {:?}",
                tool_result.output
            )));
        }
        serde_json::from_value::<ConstraintResult>(tool_result.output.clone()).map_err(|e| {
            ToolError::Executor(format!(
                "malformed constraint output for `{constraint_name}`: {e}"
            ))
        })
    }
}

impl<G: ToolGateway> std::fmt::Debug for DefaultConstraintEnforcer<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultConstraintEnforcer")
            .field("constraints", &self.constraints)
            .finish()
    }
}

#[async_trait]
impl<G: ToolGateway> ConstraintEnforcer for DefaultConstraintEnforcer<G> {
    type Error = ToolError;

    fn constraints(&self) -> &[ConstraintDefinition] {
        &self.constraints
    }

    async fn check_all(&self, run_id: RunId) -> Result<Vec<ConstraintResult>, Self::Error> {
        let mut results = Vec::with_capacity(self.constraints.len());
        for constraint in &self.constraints {
            let invocation = ToolInvocation {
                tool_name: constraint.check_tool.clone(),
                parameters: serde_json::json!({}),
                run_id,
            };
            let tool_result = self
                .gateway
                .call_tool(invocation)
                .await
                .map_err(|e| ToolError::Executor(e.to_string()))?;
            let result = self.parse_constraint_result(&constraint.name, &tool_result)?;
            results.push(result);
        }
        Ok(results)
    }

    async fn check_one(
        &self,
        run_id: RunId,
        constraint_name: &str,
    ) -> Result<ConstraintResult, Self::Error> {
        let constraint = self
            .constraints
            .iter()
            .find(|c| c.name == constraint_name)
            .ok_or_else(|| ToolError::ConstraintNotFound(constraint_name.to_string()))?;

        let invocation = ToolInvocation {
            tool_name: constraint.check_tool.clone(),
            parameters: serde_json::json!({}),
            run_id,
        };
        let tool_result = self
            .gateway
            .call_tool(invocation)
            .await
            .map_err(|e| ToolError::Executor(e.to_string()))?;
        self.parse_constraint_result(constraint_name, &tool_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    /// A mock gateway that returns configurable results for constraint tests.
    #[derive(Debug)]
    struct ConstraintTestGateway {
        results: Mutex<Vec<ToolResult>>,
    }

    #[async_trait]
    impl ToolGateway for ConstraintTestGateway {
        type Error = ToolError;

        async fn call_tool(&self, _invocation: ToolInvocation) -> Result<ToolResult, Self::Error> {
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                return Err(ToolError::ToolNotFound("no results".to_string()));
            }
            Ok(results.remove(0))
        }
    }

    fn constraint_def(name: &str, check_tool: &str) -> ConstraintDefinition {
        ConstraintDefinition {
            name: name.to_string(),
            description: format!("{name} constraint"),
            check_tool: check_tool.to_string(),
            severity: ConstraintSeverity::Error,
        }
    }

    fn success_result(constraint_name: &str, passed: bool) -> ToolResult {
        ToolResult {
            tool_name: "check_tool".to_string(),
            status: ToolResultStatus::Success,
            output: serde_json::json!({
                "constraint_name": constraint_name,
                "passed": passed,
                "violations": []
            }),
            remediation_hint: None,
            latency_ms: 1,
            cached_tokens_used: 0,
            uncached_tokens_used: 0,
        }
    }

    #[tokio::test]
    async fn check_all_returns_all_results() {
        let gateway = Arc::new(ConstraintTestGateway {
            results: Mutex::new(vec![
                success_result("lint", true),
                success_result("test", true),
            ]),
        });
        let enforcer = DefaultConstraintEnforcer::new(
            gateway,
            vec![
                constraint_def("lint", "run_lint"),
                constraint_def("test", "run_test"),
            ],
        );
        let results = enforcer.check_all(Uuid::new_v4()).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.passed));
    }

    #[tokio::test]
    async fn check_one_by_name() {
        let gateway = Arc::new(ConstraintTestGateway {
            results: Mutex::new(vec![success_result("lint", true)]),
        });
        let enforcer =
            DefaultConstraintEnforcer::new(gateway, vec![constraint_def("lint", "run_lint")]);
        let result = enforcer.check_one(Uuid::new_v4(), "lint").await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn unknown_constraint_returns_error() {
        let gateway = Arc::new(ConstraintTestGateway {
            results: Mutex::new(vec![]),
        });
        let enforcer = DefaultConstraintEnforcer::new(gateway, vec![]);
        let err = enforcer
            .check_one(Uuid::new_v4(), "nonexistent")
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::ConstraintNotFound(_)));
    }

    #[tokio::test]
    async fn violation_captured() {
        let result_with_violation = ToolResult {
            tool_name: "check_tool".to_string(),
            status: ToolResultStatus::Success,
            output: serde_json::json!({
                "constraint_name": "lint",
                "passed": false,
                "violations": [{
                    "location": "src/main.rs:10",
                    "message": "unused variable",
                    "remediation": "remove or prefix with _",
                    "severity": "Warning"
                }]
            }),
            remediation_hint: None,
            latency_ms: 1,
            cached_tokens_used: 0,
            uncached_tokens_used: 0,
        };
        let gateway = Arc::new(ConstraintTestGateway {
            results: Mutex::new(vec![result_with_violation]),
        });
        let enforcer =
            DefaultConstraintEnforcer::new(gateway, vec![constraint_def("lint", "run_lint")]);
        let result = enforcer.check_one(Uuid::new_v4(), "lint").await.unwrap();
        assert!(!result.passed);
        assert_eq!(result.violations.len(), 1);
    }

    #[tokio::test]
    async fn malformed_output_returns_error() {
        let bad_result = ToolResult {
            tool_name: "check_tool".to_string(),
            status: ToolResultStatus::Success,
            output: serde_json::json!({"not": "a constraint result"}),
            remediation_hint: None,
            latency_ms: 1,
            cached_tokens_used: 0,
            uncached_tokens_used: 0,
        };
        let gateway = Arc::new(ConstraintTestGateway {
            results: Mutex::new(vec![bad_result]),
        });
        let enforcer =
            DefaultConstraintEnforcer::new(gateway, vec![constraint_def("lint", "run_lint")]);
        let err = enforcer
            .check_one(Uuid::new_v4(), "lint")
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Executor(_)));
    }
}
