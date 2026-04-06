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
    pub embeddings: EmbeddingsConfig,
    pub extraction: ExtractionConfig,
    pub recall: RecallConfig,
    pub mcp: McpConfig,
    pub cloud: CloudConfig,
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
    /// Enable automatic consolidation when a topic exceeds the threshold.
    pub auto_consolidate_enabled: bool,
    /// Number of entries in a topic before auto-consolidation triggers.
    pub auto_consolidate_threshold: usize,
}

/// Embedding model settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EmbeddingsConfig {
    /// Enable embeddings (set to false to skip model download entirely).
    pub enabled: bool,
    /// Model identifier (fastembed model_code, e.g. "intfloat/multilingual-e5-small").
    pub model: String,
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "intfloat/multilingual-e5-base".into(),
        }
    }
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
    /// Extract every N tool calls (hook counter).
    pub extract_every: usize,
    /// Store raw text as fallback when no facts are extracted.
    pub store_raw: bool,
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
    /// Compact mode: shorter MCP responses to save tokens (default: true).
    pub compact: bool,
    /// Custom system instructions appended to MCP server info.
    pub instructions: Option<String>,
}

/// RTK Cloud sync settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CloudConfig {
    /// Enable cloud sync (requires login).
    pub enabled: bool,
    /// RTK Cloud endpoint.
    pub endpoint: String,
    /// Default scope for new memories (user, project, org).
    pub default_scope: String,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://cloud.rtk-ai.app".into(),
            default_scope: "user".into(),
        }
    }
}

// --- Defaults ---

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            default_importance: "medium".into(),
            decay_rate: 0.95,
            prune_threshold: 0.1,
            auto_consolidate_enabled: false,
            auto_consolidate_threshold: 10,
        }
    }
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_score: 2.0,
            max_facts: 20,
            extract_every: 3,
            store_raw: true,
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
            compact: true,
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

/// Resolve the config file path (cross-platform via `directories`).
fn config_path() -> Option<PathBuf> {
    // 1. Environment variable
    if let Ok(p) = std::env::var("ICM_CONFIG") {
        return Some(PathBuf::from(p));
    }

    // 2. Platform-specific config dir:
    //    macOS:   ~/Library/Application Support/dev.icm.icm/config.toml
    //    Linux:   ~/.config/icm/config.toml  (XDG_CONFIG_HOME)
    //    Windows: C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml
    directories::ProjectDirs::from("dev", "icm", "icm")
        .map(|dirs| dirs.config_dir().join("config.toml"))
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
        assert!(config.mcp.compact);
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
extract_every = 20
store_raw = false

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
        assert_eq!(config.extraction.extract_every, 20);
        assert!(!config.extraction.store_raw);
        assert_eq!(config.recall.limit, 20);
        assert!(config.mcp.instructions.is_some());
    }
}
