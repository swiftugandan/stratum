//! stratum-adapters: Concrete implementations of port traits.
//!
//! Houses external adapters: LLM clients, SQLite stores, vector DB,
//! filesystem memory, and notification backends.

pub mod error;
pub mod metrics;
pub mod trajectory;

pub use error::TrajectoryStoreError;
pub use metrics::InMemoryMetrics;
pub use trajectory::SqliteTrajectoryStore;

pub use stratum_core::LlmClient;
