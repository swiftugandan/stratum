//! stratum-test-utils: Mock/stub implementations of all port traits for testing.

use std::sync::Once;

pub mod mocks;

static INIT: Once = Once::new();

pub fn init_test_logging() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("debug")
            .init();
    });
}
