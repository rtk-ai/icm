//! Configuration loading from TOML files.
//!
//! Lookup order:
//! 1. `$ICM_CONFIG` environment variable
//! 2. `~/.config/icm/config.toml`
//! 3. Built-in defaults (everything is optional)

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub store: StoreConfig,
    pub memory: MemoryConfig,
    pub extraction: ExtractionConfig,
    pub recall: RecallConfig,
    pub mcp: McpConfig,
}

/// Database storage settings.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StoreConfig {
    /// SQLite database path. Default: platform-specific data dir.
    pub path: Option<String>,
}

/// Memory decay and pruning settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub default_importance: String,
    pub decay_rate: f32,
    pub prune_threshold: f32,
}

/// Auto-extraction settings (Layer 0).
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ExtractionConfig {
    pub enabled: bool,
    /// Minimum keyword score to keep a fact.
    pub min_score: f32,
    /// Maximum facts per extraction pass.
    pub max_facts: usize,
}

/// Context recall/injection settings (Layer 2).
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RecallConfig {
    pub enabled: bool,
    /// Maximum memories to inject.
    pub limit: usize,
}

/// MCP server settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub transport: String,
    /// Custom system instructions appended to MCP server info.
    pub instructions: Option<String>,
}

// --- Defaults ---

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            default_importance: "medium".into(),
            decay_rate: 0.95,
            prune_threshold: 0.1,
        }
    }
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_score: 3.0,
            max_facts: 10,
        }
    }
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            limit: 15,
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            transport: "stdio".into(),
            instructions: None,
        }
    }
}

/// Load config from disk. Returns defaults if no config file exists.
pub fn load_config() -> Result<Config> {
    let path = config_path();

    if let Some(p) = &path {
        if p.exists() {
            let content =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            let config: Config =
                toml::from_str(&content).with_context(|| format!("parsing {}", p.display()))?;
            return Ok(config);
        }
    }

    Ok(Config::default())
}

/// Resolve the config file path.
fn config_path() -> Option<PathBuf> {
    // 1. Environment variable
    if let Ok(p) = std::env::var("ICM_CONFIG") {
        return Some(PathBuf::from(p));
    }

    // 2. ~/.config/icm/config.toml
    if let Some(home) = dirs_home() {
        let p = home.join(".config").join("icm").join("config.toml");
        return Some(p);
    }

    None
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Show the active config path (for `icm config show`).
pub fn show_config_path() -> String {
    match config_path() {
        Some(p) if p.exists() => format!("{} (loaded)", p.display()),
        Some(p) => format!("{} (not found, using defaults)", p.display()),
        None => "no config path resolved (using defaults)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.extraction.enabled);
        assert_eq!(config.memory.decay_rate, 0.95);
        assert_eq!(config.recall.limit, 15);
    }

    #[test]
    fn test_parse_minimal_toml() {
        let toml_str = r#"
[memory]
decay_rate = 0.90
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.memory.decay_rate, 0.90);
        // Other fields should be defaults
        assert!(config.extraction.enabled);
    }

    #[test]
    fn test_parse_full_toml() {
        let toml_str = r#"
[store]
path = "/tmp/test.db"

[memory]
default_importance = "high"
decay_rate = 0.90
prune_threshold = 0.2

[extraction]
enabled = false
min_score = 5.0
max_facts = 5

[recall]
enabled = true
limit = 20

[mcp]
transport = "stdio"
instructions = "Custom instructions here"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.store.path.as_deref(), Some("/tmp/test.db"));
        assert!(!config.extraction.enabled);
        assert_eq!(config.recall.limit, 20);
        assert!(config.mcp.instructions.is_some());
    }
}
