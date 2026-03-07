//! Simplified config: Anthropic-only, daemon-only.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StratumConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub data_dir: PathBuf,
    pub max_concurrent_runs: usize,
}

impl Default for StratumConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            data_dir: PathBuf::from(".stratum"),
            max_concurrent_runs: 4,
        }
    }
}

impl StratumConfig {
    pub fn load(config_path: Option<&PathBuf>) -> anyhow::Result<Self> {
        let mut config = Self::load_from_file(config_path)?;
        config.apply_env_overrides();
        Ok(config)
    }

    fn load_from_file(config_path: Option<&PathBuf>) -> anyhow::Result<Self> {
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
        if let Ok(dir) = std::env::var("STRATUM_DATA_DIR") {
            self.data_dir = PathBuf::from(dir);
        }
    }

    pub fn require_api_key(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "API key required. Set STRATUM_API_KEY env var or api_key in stratum.yaml"
            );
        }
        Ok(())
    }

    pub fn db_path(&self) -> String {
        self.data_dir
            .join("stratum.db")
            .to_string_lossy()
            .to_string()
    }

    pub fn queue_root(&self) -> PathBuf {
        self.data_dir.join("queues")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.data_dir.join("daemon.pid")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.data_dir.join("skills")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = StratumConfig::default();
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.max_tokens, 4096);
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
        assert!(config.db_path().contains("stratum.db"));
    }
}
