//! Configuration for the memory store.

use std::path::PathBuf;

use stratum_types::{MemoryConfig, SearchStrategy};

/// Configuration for `DefaultMemoryStore`.
#[derive(Debug, Clone)]
pub struct MemoryStoreConfig {
    /// Directory for project-tier Markdown files.
    pub project_memory_dir: PathBuf,
    /// SQLite database path for episodic memory.
    pub episodic_db_path: PathBuf,
    /// SQLite database path for global memory.
    pub global_db_path: PathBuf,
    /// Search strategy (only Bm25 is currently supported).
    pub search_strategy: SearchStrategy,
    /// Directory containing skill Markdown files.
    pub skills_dir: PathBuf,
    /// Half-life in hours for temporal decay scoring (default: 168 = 1 week).
    pub temporal_decay_half_life_hours: f64,
    /// If true, global promotions skip the approval queue and write directly.
    /// Useful for autonomous daemon mode where no human is approving promotions.
    pub auto_approve_global: bool,
}

impl Default for MemoryStoreConfig {
    fn default() -> Self {
        Self {
            project_memory_dir: PathBuf::from(".stratum/memory"),
            episodic_db_path: PathBuf::from(".stratum/episodic.db"),
            global_db_path: PathBuf::from(".stratum/global.db"),
            search_strategy: SearchStrategy::Bm25,
            skills_dir: PathBuf::from(".stratum/skills"),
            temporal_decay_half_life_hours: 168.0,
            auto_approve_global: false,
        }
    }
}

impl MemoryStoreConfig {
    /// Create a config from domain `MemoryConfig`, filling in defaults for paths.
    pub fn from_memory_config(config: &MemoryConfig) -> Self {
        let project_root = config.project_root.as_deref().unwrap_or(".stratum/memory");
        Self {
            project_memory_dir: PathBuf::from(project_root),
            search_strategy: config.search_strategy.clone(),
            ..Default::default()
        }
    }
}
