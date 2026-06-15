//! Global preferences (`config.toml`). Distinct from `state.toml`, which holds
//! the declarative inventory of servers/vhosts/PHP versions.

use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Homebrew prefix, e.g. `/opt/homebrew`.
    pub brew_prefix: String,
    /// Where user sites live by default, e.g. `~/Sites`.
    pub sites_root: String,
    /// Default backend for `server add` when unspecified.
    pub default_backend: String,
    /// Default PHP version for new vhosts, e.g. `8.3`.
    pub default_php: Option<String>,
    /// TLDs used for local wildcard DNS (default `["test"]`).
    #[serde(default = "default_tlds")]
    pub local_tlds: Vec<String>,
}

fn default_tlds() -> Vec<String> {
    vec!["test".to_string()]
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir()
            .map(|h| h.join("Sites").display().to_string())
            .unwrap_or_else(|| "~/Sites".to_string());
        Self {
            brew_prefix: "/opt/homebrew".to_string(),
            sites_root: home,
            default_backend: "caddy".to_string(),
            default_php: None,
            local_tlds: default_tlds(),
        }
    }
}

impl Config {
    /// Normalize TLDs: lowercase, strip dots/whitespace, drop blanks, dedupe,
    /// and fall back to `test` if the list ends up empty.
    pub fn normalize_tlds(&mut self) {
        let mut seen = std::collections::BTreeSet::new();
        self.local_tlds = self
            .local_tlds
            .iter()
            .map(|t| t.trim().trim_matches('.').to_lowercase())
            .filter(|t| !t.is_empty())
            .filter(|t| seen.insert(t.clone()))
            .collect();
        if self.local_tlds.is_empty() {
            self.local_tlds = default_tlds();
        }
    }
}

pub fn load_config() -> Result<Config> {
    let path = paths::config_path()?;
    if !path.exists() {
        anyhow::bail!("reeve is not configured. Run `reeve init` first.");
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config from {}", path.display()))?;
    let mut cfg: Config = toml::from_str(&contents).context("Invalid config.toml")?;
    cfg.normalize_tlds();
    Ok(cfg)
}

pub fn save_config(config: &Config) -> Result<()> {
    paths::ensure_dirs()?;
    let path = paths::config_path()?;
    let contents = toml::to_string_pretty(config).context("Failed to serialize config")?;
    fs::write(&path, contents)
        .with_context(|| format!("Failed to write config to {}", path.display()))?;
    Ok(())
}
