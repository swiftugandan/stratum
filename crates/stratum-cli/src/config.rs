use std::path::PathBuf;

use serde::Deserialize;
use stratum_context::ContextEngineConfig;
use stratum_memory::MemoryStoreConfig;
use stratum_orchestrator::OrchestratorConfig;
use stratum_tools::{SubprocessExecutorConfig, ToolGatewayConfig};
use stratum_types::TrustLevel;

/// Supported LLM providers.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    Anthropic,
    OpenAi,
    OpenAiResponses,
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::Anthropic
    }
}

/// Top-level configuration for the Stratum CLI.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StratumConfig {
    /// LLM provider to use.
    pub llm_provider: LlmProvider,
    /// API key for the LLM provider (override: STRATUM_API_KEY env var).
    pub api_key: String,
    /// Model identifier (override: STRATUM_MODEL env var).
    pub model: String,
    /// Maximum tokens for LLM responses.
    pub max_tokens: u32,
    /// Trust level for this run (override: STRATUM_TRUST_LEVEL env var).
    pub trust_level: TrustLevel,
    /// Data directory for SQLite DBs and offloaded data (override: STRATUM_DATA_DIR env var).
    pub data_dir: PathBuf,
}

impl Default for StratumConfig {
    fn default() -> Self {
        Self {
            llm_provider: LlmProvider::default(),
            api_key: String::new(),
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            trust_level: TrustLevel::Supervised,
            data_dir: PathBuf::from(".stratum"),
        }
    }
}

impl StratumConfig {
    /// Load configuration with precedence: defaults → YAML file → env vars.
    pub fn load(config_path: Option<&PathBuf>) -> anyhow::Result<Self> {
        let mut config = Self::load_from_file(config_path)?;
        config.apply_env_overrides();
        Ok(config)
    }

    fn load_from_file(config_path: Option<&PathBuf>) -> anyhow::Result<Self> {
        // Try explicit path first, then default
        let path = config_path.cloned().or_else(|| {
            let default = PathBuf::from("stratum.yaml");
            if default.exists() {
                Some(default)
            } else {
                None
            }
        });

        match path {
            Some(p) if p.exists() => {
                let contents = std::fs::read_to_string(&p)?;
                let config: Self = serde_yaml::from_str(&contents)?;
                Ok(config)
            }
            Some(p) if config_path.is_some() => {
                anyhow::bail!("config file not found: {}", p.display());
            }
            _ => Ok(Self::default()),
        }
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(key) = std::env::var("STRATUM_API_KEY") {
            self.api_key = key;
        }
        if let Ok(model) = std::env::var("STRATUM_MODEL") {
            self.model = model;
        }
        if let Ok(level) = std::env::var("STRATUM_TRUST_LEVEL") {
            match level.to_lowercase().as_str() {
                "sandboxed" => self.trust_level = TrustLevel::Sandboxed,
                "supervised" => self.trust_level = TrustLevel::Supervised,
                "autonomous" => self.trust_level = TrustLevel::Autonomous,
                _ => tracing::warn!("unknown trust level: {level}, keeping default"),
            }
        }
        if let Ok(dir) = std::env::var("STRATUM_DATA_DIR") {
            self.data_dir = PathBuf::from(dir);
        }
    }

    /// Validate that the API key is present (required for run/resume commands).
    pub fn require_api_key(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "API key required. Set STRATUM_API_KEY env var or api_key in stratum.yaml"
            );
        }
        Ok(())
    }

    /// Build a `ContextEngineConfig` from this config.
    pub fn context_engine_config(&self) -> ContextEngineConfig {
        ContextEngineConfig {
            offload_dir: self.data_dir.join("offload"),
            ..ContextEngineConfig::default()
        }
    }

    /// Build a `MemoryStoreConfig` from this config.
    pub fn memory_store_config(&self) -> MemoryStoreConfig {
        MemoryStoreConfig {
            project_memory_dir: self.data_dir.join("memory"),
            episodic_db_path: self.data_dir.join("episodic.db"),
            global_db_path: self.data_dir.join("global.db"),
            skills_dir: self.data_dir.join("skills"),
            ..MemoryStoreConfig::default()
        }
    }

    /// Build a `ToolGatewayConfig` from this config.
    pub fn tool_gateway_config(&self) -> ToolGatewayConfig {
        ToolGatewayConfig::default()
    }

    /// Build an `OrchestratorConfig` from this config.
    pub fn orchestrator_config(&self) -> OrchestratorConfig {
        OrchestratorConfig {
            queue_root: self.data_dir.join("queues"),
            ..OrchestratorConfig::default()
        }
    }

    /// Build a `SubprocessExecutorConfig` from this config.
    pub fn subprocess_executor_config(&self) -> SubprocessExecutorConfig {
        SubprocessExecutorConfig {
            sandbox_root: self.data_dir.join("sandbox"),
            ..SubprocessExecutorConfig::default()
        }
    }

    /// Return the path to the main SQLite database.
    pub fn db_path(&self) -> String {
        self.data_dir
            .join("stratum.db")
            .to_string_lossy()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let config = StratumConfig::default();
        assert_eq!(config.llm_provider, LlmProvider::Anthropic);
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.max_tokens, 4096);
        assert_eq!(config.trust_level, TrustLevel::Supervised);
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn require_api_key_fails_when_empty() {
        let config = StratumConfig::default();
        assert!(config.require_api_key().is_err());
    }

    #[test]
    fn require_api_key_succeeds_when_set() {
        let config = StratumConfig {
            api_key: "sk-test".to_string(),
            ..StratumConfig::default()
        };
        assert!(config.require_api_key().is_ok());
    }

    #[test]
    fn db_path_uses_data_dir() {
        let config = StratumConfig::default();
        assert!(config.db_path().contains(".stratum"));
        assert!(config.db_path().contains("stratum.db"));
    }
}
