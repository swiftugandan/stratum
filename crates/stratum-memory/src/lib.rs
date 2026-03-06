//! stratum-memory: Memory Hierarchy implementation (Stratum 3).
//!
//! Provides a 4-tier memory system (Working, Episodic, Project, Global) with
//! BM25 full-text search via SQLite FTS5, tier promotion with validation,
//! and skill loading from Markdown files with YAML frontmatter.

pub mod config;
pub mod error;
pub mod search;
pub mod skill;
pub mod store;
mod tiers;

pub use config::MemoryStoreConfig;
pub use error::MemoryError;
pub use skill::FilesystemSkillLoader;
pub use store::DefaultMemoryStore;

// Re-export promotion queue types
pub use crate::tiers::global::{PromotionRequest, PromotionStatus};

// Re-export port traits for convenience
pub use stratum_core::{MemoryStore, SkillLoader};
