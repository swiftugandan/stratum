//! stratum-adapters: Concrete implementations of port traits.
//!
//! Houses external adapters: LLM clients, SQLite stores, vector DB,
//! filesystem memory, and notification backends.

pub mod error;
pub mod metrics;
pub mod prompt;
pub mod session;
pub mod trajectory;
pub mod util;

pub use error::AdapterError;
pub use metrics::InMemoryMetrics;
pub use prompt::{initialiser_prompt, worker_prompt};
pub use session::SqliteSessionManager;
pub use trajectory::SqliteTrajectoryStore;

pub use stratum_core::LlmClient;
