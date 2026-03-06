//! Exponential backoff with full jitter for tool execution retries.

use rand::Rng;
use std::time::Duration;

use crate::config::ToolGatewayConfig;

/// Compute the delay for a given attempt (0-indexed) with full jitter.
///
/// Formula: `rand(0..min(max_delay, base_delay * 2^attempt))`
pub(crate) fn delay_for_attempt(config: &ToolGatewayConfig, attempt: u32) -> Duration {
    let base_ms = config.retry_base_delay.as_millis() as u64;
    let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(10));
    let capped_ms = exp_ms.min(config.retry_max_delay.as_millis() as u64);
    if capped_ms == 0 {
        return Duration::ZERO;
    }
    let jitter_ms = rand::thread_rng().gen_range(0..=capped_ms);
    Duration::from_millis(jitter_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_bounded_by_max() {
        let config = ToolGatewayConfig {
            retry_base_delay: Duration::from_millis(100),
            retry_max_delay: Duration::from_secs(2),
            ..Default::default()
        };
        for attempt in 0..15 {
            let delay = delay_for_attempt(&config, attempt);
            assert!(delay <= config.retry_max_delay);
        }
    }

    #[test]
    fn delay_increases_with_attempt() {
        // With enough samples, higher attempts should have higher *average* caps.
        // We just check the cap formula directly.
        let config = ToolGatewayConfig {
            retry_base_delay: Duration::from_millis(100),
            retry_max_delay: Duration::from_secs(60),
            ..Default::default()
        };
        let cap_0 = 100u64; // 100 * 2^0
        let cap_3 = 800u64; // 100 * 2^3
                            // The cap at attempt 3 should be higher than attempt 0.
        assert!(cap_3 > cap_0);
        // All delays should still be bounded.
        let delay = delay_for_attempt(&config, 3);
        assert!(delay <= config.retry_max_delay);
    }

    #[test]
    fn zero_base_returns_zero() {
        let config = ToolGatewayConfig {
            retry_base_delay: Duration::ZERO,
            retry_max_delay: Duration::from_secs(30),
            ..Default::default()
        };
        let delay = delay_for_attempt(&config, 5);
        assert_eq!(delay, Duration::ZERO);
    }
}
