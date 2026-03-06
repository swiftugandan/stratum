//! Configuration for the Tool Execution Gateway.

use std::time::Duration;

/// Configuration for `DefaultToolGateway`.
#[derive(Debug, Clone)]
pub struct ToolGatewayConfig {
    /// Maximum number of retries for retryable executor failures.
    pub max_retries: u32,
    /// Base delay for exponential backoff.
    pub retry_base_delay: Duration,
    /// Maximum delay cap for exponential backoff.
    pub retry_max_delay: Duration,
    /// Number of consecutive failures before HITL escalation.
    pub hitl_escalation_threshold: u32,
    /// Whether to validate parameters against tool JSON Schema.
    pub validate_schema: bool,
}

impl Default for ToolGatewayConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_base_delay: Duration::from_millis(500),
            retry_max_delay: Duration::from_secs(30),
            hitl_escalation_threshold: 3,
            validate_schema: true,
        }
    }
}
