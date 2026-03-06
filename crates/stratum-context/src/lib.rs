//! stratum-context: Context Engine implementation (Stratum 2).
//!
//! Owns context assembly, budget model, 3-stage compaction, hygiene scoring,
//! and todo recitation.

pub mod compaction;
pub mod engine;
pub mod error;
pub mod token;

pub use engine::{ContextEngineConfig, DefaultContextEngine};
pub use error::ContextError;
pub use stratum_core::ContextEngine;
