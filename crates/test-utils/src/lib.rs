use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize shared test infrastructure (logging, etc.).
/// Safe to call multiple times — only runs once.
pub fn init_test_logging() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("debug")
            .init();
    });
}
