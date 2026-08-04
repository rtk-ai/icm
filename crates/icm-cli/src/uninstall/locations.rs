//! Catalog of every file `icm init` may have touched.
//!
//! The list is built statically per the mirror of `cmd_init`
//! (`crates/icm-cli/src/main.rs`). When `icm init` learns to write a new
//! file, this catalog must be kept in sync. A future PR will replace this
//! with an install manifest persisted at init time so uninstall doesn't
//! have to re-derive the surface from a hard-coded list (issue #229).
//!
//! Path resolution honors the same environment overrides as `cmd_init`:
//! `CLAUDE_CONFIG_DIR`, `GEMINI_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME`.

use std::path::PathBuf;

use anyhow::Result;

use crate::install_manifest::InstallManifest;
use crate::trusted_mcp::{
    JsonTrustSpec, ProviderConfig, ProviderPathContext, TomlTrustSpec, TrustChange,
    TRUSTED_PROVIDERS,
};

/// Shape of the command field inside a hook entry. Copilot uses a
/// top-level `bash` field; every other CLI uses `hooks[].command`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum HookCommandField {
    Command,
    BashTopLevel,
}

#[derive(Clone, Debug)]
pub(crate) struct JsonTrustLocation {
    pub spec: JsonTrustSpec,
    /// `None` keeps legacy exact-match cleanup; `Some` is authoritative,
    /// including an empty list that owns nothing.
    pub owned_changes: Option<Vec<TrustChange>>,
}

#[derive(Clone, Debug)]
pub(crate) struct TomlTrustLocation {
    pub spec: TomlTrustSpec,
    pub owned_changes: Option<Vec<TrustChange>>,
}

/// One *category* of artifact init produces. Drives the matching strategy
/// in `discover::scan` and the mutation strategy in `formats`.
#[derive(Clone, Debug)]
pub(crate) enum LocationKind {
    /// JSON file (possibly JSONC). The icm entry lives at
    /// `<servers_key>.icm` when `servers_key` is `Some`; the file may also
    /// carry a `hooks.<Event>[].hooks[]` section that needs filtering.
    /// `servers_key` accepts dotted paths (e.g. `"amp.mcpServers"`).
    JsonConfig {
        servers_key: Option<&'static str>,
        has_hooks: bool,
        hooks_field: HookCommandField,
    },
    /// A JSON hook file that also carries provider-described trust values.
    JsonHooksWithTrust { trust: JsonTrustLocation },
    /// A JSON file containing only provider-described trust values.
    JsonTrust { trust: JsonTrustLocation },
    /// A JSON settings file containing both an ICM MCP server and
    /// client-specific ICM permission rules.
    JsonMcpWithTrust {
        servers_key: &'static str,
        trust: JsonTrustLocation,
    },
    /// TOML file with `[<table>.<entry>]` to remove, plus dotted child
    /// keys like `<table>.<entry>.env`.
    TomlMcp {
        table: &'static str,
        entry: &'static str,
        trust: Option<TomlTrustLocation>,
    },
    /// Continue.dev YAML — regex-stripped block under `mcpServers:`.
    YamlContinue,
    /// Markdown file with an `<!-- icm:start --> ... <!-- icm:end -->`
    /// block to remove. Whole file is deleted if the block was the only
    /// content.
    MarkdownBlock,
    /// Whole-file artifact owned solely by `icm init` (skill prompts,
    /// OpenCode plugin). Deleted outright.
    OwnedFile,
    /// Data directory. Touched only with `--purge-data`. The catalog
    /// records the directory; discover enumerates the actual files inside.
    DataDir,
}

/// One concrete file/directory ICM may have written to.
#[derive(Clone, Debug)]
pub(crate) struct LocationSpec {
    pub label: &'static str,
    pub path: PathBuf,
    pub kind: LocationKind,
    /// `true` for DataDir entries (memories.db, fastembed cache) that are
    /// preserved unless the user passes `--purge-data`.
    pub purge_data_only: bool,
}

/// Resolved per-tool directories, honoring env var overrides. Built once
/// at the start of `uninstall::run` and passed into the catalog.
#[derive(Clone, Debug)]
pub(crate) struct DirContext {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub gemini_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub copilot_dir: PathBuf,
    pub vscode_data: PathBuf,
    pub zed_settings: PathBuf,
    pub cwd: PathBuf,
}

impl DirContext {
    /// Resolve every dir from the environment exactly the way `cmd_init`
    /// does. Tests should construct `DirContext` manually with a tempdir.
    pub(crate) fn from_env() -> Result<Self> {
        let home_str = crate::home_dir_str()?;
        let home = PathBuf::from(&home_str);
        let claude_dir = crate::cli_config_dir("CLAUDE_CONFIG_DIR", ".claude", &home_str);
        let gemini_dir = crate::cli_config_dir("GEMINI_CONFIG_DIR", ".gemini", &home_str);
        let codex_dir = crate::cli_config_dir("CODEX_HOME", ".codex", &home_str);
        let copilot_dir = crate::cli_config_dir("COPILOT_HOME", ".copilot", &home_str);
        let vscode_data = if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Code/User")
        } else {
            home.join(".config/Code/User")
        };
        let zed_settings = if cfg!(target_os = "macos") {
            home.join(".zed/settings.json")
        } else {
            home.join(".config/zed/settings.json")
        };
        let cwd = std::env::current_dir()?;
        Ok(Self {
            home,
            claude_dir,
            gemini_dir,
            codex_dir,
            copilot_dir,
            vscode_data,
            zed_settings,
            cwd,
        })
    }

    /// `~/.claude.json` lives at the home root, unless `CLAUDE_CONFIG_DIR`
    /// is set — in which case `cmd_init` colocates it inside the override.
    pub(crate) fn claude_legacy_json(&self) -> PathBuf {
        if std::env::var("CLAUDE_CONFIG_DIR")
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            self.claude_dir.join(".claude.json")
        } else {
            self.home.join(".claude.json")
        }
    }

    pub(crate) fn claude_desktop_json(&self) -> PathBuf {
        self.home
            .join("Library/Application Support/Claude/claude_desktop_config.json")
    }
}

/// `ProjectDirs::from("dev","icm","icm")` data dir, mirroring
/// `default_db_path` in `main.rs`. Returned even if the directory does
/// not exist on disk so discover can decide whether it's a hit.
pub(crate) fn icm_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "icm", "icm").map(|d| d.data_dir().to_path_buf())
}

/// Same as [`icm_data_dir`] for the fastembed model cache.
pub(crate) fn icm_cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "icm", "icm").map(|d| d.cache_dir().to_path_buf())
}

/// Build the full catalog of locations to scan.
///
/// Order roughly groups locations by tool family so reports group nicely.
/// Path existence is **not** checked here — discover does that.
pub(crate) fn build_locations(d: &DirContext) -> Vec<LocationSpec> {
    use HookCommandField as F;
    use LocationKind as K;

    let mut specs = Vec::with_capacity(40);

    let provider_paths = ProviderPathContext {
        home: &d.home,
        claude_dir: &d.claude_dir,
        codex_dir: &d.codex_dir,
        zed_settings: &d.zed_settings,
    };
    for provider in TRUSTED_PROVIDERS {
        let kind = match provider.config {
            ProviderConfig::JsonTrust {
                spec,
                has_hooks: true,
            } => K::JsonHooksWithTrust {
                trust: JsonTrustLocation {
                    spec,
                    owned_changes: None,
                },
            },
            ProviderConfig::JsonTrust {
                spec,
                has_hooks: false,
            } => K::JsonTrust {
                trust: JsonTrustLocation {
                    spec,
                    owned_changes: None,
                },
            },
            ProviderConfig::JsonMcp { spec, servers_key } => K::JsonMcpWithTrust {
                servers_key,
                trust: JsonTrustLocation {
                    spec,
                    owned_changes: None,
                },
            },
            ProviderConfig::TomlMcp { spec, table, entry } => K::TomlMcp {
                table,
                entry,
                trust: Some(TomlTrustLocation {
                    spec,
                    owned_changes: None,
                }),
            },
        };
        specs.push(LocationSpec {
            label: provider.uninstall_label,
            path: provider.config_path(&provider_paths),
            kind,
            purge_data_only: false,
        });
    }

    // --- Claude Code (mcpServers + hooks + skills + cwd CLAUDE.md) ---
    specs.push(LocationSpec {
        label: "Claude Code MCP",
        path: d.claude_legacy_json(),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Claude Code /recall",
        path: d.claude_dir.join("commands/recall.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Claude Code /remember",
        path: d.claude_dir.join("commands/remember.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });

    // --- Claude Desktop (macOS) ---
    specs.push(LocationSpec {
        label: "Claude Desktop MCP",
        path: d.claude_desktop_json(),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });

    // --- Codex CLI (TOML MCP + JSON hooks) ---
    specs.push(LocationSpec {
        label: "Codex CLI hooks",
        path: d.codex_dir.join("hooks.json"),
        kind: K::JsonConfig {
            servers_key: None,
            has_hooks: true,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });

    // --- Gemini CLI (MCP + hooks live in the same settings.json) ---
    specs.push(LocationSpec {
        label: "Gemini CLI",
        path: d.gemini_dir.join("settings.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: true,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Gemini CLI GEMINI.md",
        path: d.gemini_dir.join("GEMINI.md"),
        kind: K::MarkdownBlock,
        purge_data_only: false,
    });

    // --- Copilot CLI (separate mcp + settings files, bash top-level hooks) ---
    specs.push(LocationSpec {
        label: "Copilot CLI MCP",
        path: d.copilot_dir.join("mcp-config.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Copilot CLI hooks",
        path: d.copilot_dir.join("settings.json"),
        kind: K::JsonConfig {
            servers_key: None,
            has_hooks: true,
            hooks_field: F::BashTopLevel,
        },
        purge_data_only: false,
    });

    // --- Cursor ---
    specs.push(LocationSpec {
        label: "Cursor MCP",
        path: d.home.join(".cursor/mcp.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Cursor rule",
        path: d.home.join(".cursor/rules/icm.mdc"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });

    // --- Roo Code (global rules) ---
    specs.push(LocationSpec {
        label: "Roo Code rule",
        path: d.home.join(".roo/rules/icm.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });

    // --- Windsurf ---
    specs.push(LocationSpec {
        label: "Windsurf MCP",
        path: d.home.join(".codeium/windsurf/mcp_config.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });

    // --- VS Code (user mcp.json + 3 extensions in globalStorage) ---
    specs.push(LocationSpec {
        label: "VS Code MCP",
        path: d.vscode_data.join("mcp.json"),
        kind: K::JsonConfig {
            servers_key: Some("servers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Cline (VS Code)",
        path: d
            .vscode_data
            .join("globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Roo (VS Code)",
        path: d
            .vscode_data
            .join("globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Kilo Code (VS Code)",
        path: d
            .vscode_data
            .join("globalStorage/kilocode.kilo-code/settings/mcp_settings.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });

    // --- OpenCode (different JSON shape under "mcp" + TS plugin) ---
    specs.push(LocationSpec {
        label: "OpenCode plugin",
        path: d.home.join(".config/opencode/plugins/icm.ts"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "OpenCode plugin (legacy .js)",
        path: d.home.join(".config/opencode/plugins/icm.js"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });

    // --- Amp (dotted servers key + skills) ---
    specs.push(LocationSpec {
        label: "Amp MCP",
        path: d.home.join(".config/amp/settings.json"),
        kind: K::JsonConfig {
            servers_key: Some("amp.mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Amp /icm-recall",
        path: d.home.join(".config/amp/skills/icm-recall.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Amp /icm-remember",
        path: d.home.join(".config/amp/skills/icm-remember.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });

    // --- Pi (pi.dev / earendil-works/pi) — issue #259 ---
    specs.push(LocationSpec {
        label: "Pi AGENTS.md",
        path: d.home.join(".pi/agent/AGENTS.md"),
        kind: K::MarkdownBlock,
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Pi /icm-recall",
        path: d.home.join(".pi/agent/skills/icm-recall.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });
    specs.push(LocationSpec {
        label: "Pi /icm-remember",
        path: d.home.join(".pi/agent/skills/icm-remember.md"),
        kind: K::OwnedFile,
        purge_data_only: false,
    });

    // --- Amazon Q ---
    specs.push(LocationSpec {
        label: "Amazon Q MCP",
        path: d.home.join(".aws/amazonq/mcp.json"),
        kind: K::JsonConfig {
            servers_key: Some("mcpServers"),
            has_hooks: false,
            hooks_field: F::Command,
        },
        purge_data_only: false,
    });

    // --- Continue.dev (YAML config) ---
    specs.push(LocationSpec {
        label: "Continue.dev",
        path: d.home.join(".continue/config.yaml"),
        kind: K::YamlContinue,
        purge_data_only: false,
    });

    // --- Cwd instruction files (written by `icm init` at the cwd it ran in) ---
    let cwd_md_files: &[(&str, &str)] = &[
        ("CLAUDE.md (cwd)", "CLAUDE.md"),
        ("AGENTS.md (cwd)", "AGENTS.md"),
        (
            "Copilot instructions (cwd)",
            ".github/copilot-instructions.md",
        ),
        ("Windsurf rules (cwd)", ".windsurfrules"),
        ("Aider conventions (cwd)", ".aider.conventions.md"),
    ];
    for (label, rel) in cwd_md_files {
        specs.push(LocationSpec {
            label,
            path: d.cwd.join(rel),
            kind: K::MarkdownBlock,
            purge_data_only: false,
        });
    }

    // --- Data (--purge-data) ---
    if let Some(data) = icm_data_dir() {
        specs.push(LocationSpec {
            label: "ICM database directory",
            path: data,
            kind: K::DataDir,
            purge_data_only: true,
        });
    }
    if let Some(cache) = icm_cache_dir() {
        specs.push(LocationSpec {
            label: "Fastembed model cache",
            path: cache.join("models"),
            kind: K::DataDir,
            purge_data_only: true,
        });
    }

    specs
}

/// Attach the ownership snapshot before discovery so the read-only plan and
/// the later mutation use the same provenance boundary.
pub(crate) fn apply_manifest_ownership(specs: &mut [LocationSpec], manifest: &InstallManifest) {
    for location in specs {
        let owned = manifest
            .trust_changes_for(&location.path)
            .map(<[TrustChange]>::to_vec);
        match &mut location.kind {
            LocationKind::JsonHooksWithTrust { trust }
            | LocationKind::JsonTrust { trust }
            | LocationKind::JsonMcpWithTrust { trust, .. } => {
                trust.owned_changes = owned;
            }
            LocationKind::TomlMcp {
                trust: Some(trust), ..
            } => {
                trust.owned_changes = owned;
            }
            _ => {}
        }
    }
}

/// Convenience for tests: build a `DirContext` rooted at `root`, so the
/// real `$HOME` is not touched.
#[cfg(test)]
pub(crate) fn dir_context_under(root: &std::path::Path) -> DirContext {
    let home = root.to_path_buf();
    let vscode_data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Code/User")
    } else {
        home.join(".config/Code/User")
    };
    let zed_settings = if cfg!(target_os = "macos") {
        home.join(".zed/settings.json")
    } else {
        home.join(".config/zed/settings.json")
    };
    DirContext {
        home: home.clone(),
        claude_dir: home.join(".claude"),
        gemini_dir: home.join(".gemini"),
        codex_dir: home.join(".codex"),
        copilot_dir: home.join(".copilot"),
        vscode_data,
        zed_settings,
        cwd: home.join("proj"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_provider_registry_drives_every_uninstall_location() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let paths = ProviderPathContext {
            home: &dirs.home,
            claude_dir: &dirs.claude_dir,
            codex_dir: &dirs.codex_dir,
            zed_settings: &dirs.zed_settings,
        };
        let locations = build_locations(&dirs);

        for provider in TRUSTED_PROVIDERS {
            let expected_path = provider.config_path(&paths);
            let matches = locations
                .iter()
                .filter(|location| {
                    location.label == provider.uninstall_label && location.path == expected_path
                })
                .collect::<Vec<_>>();
            assert_eq!(matches.len(), 1, "{} uninstall location", provider.client);

            let wired = match (provider.config, &matches[0].kind) {
                (
                    ProviderConfig::JsonTrust {
                        spec: expected,
                        has_hooks: true,
                    },
                    LocationKind::JsonHooksWithTrust { trust },
                )
                | (
                    ProviderConfig::JsonTrust {
                        spec: expected,
                        has_hooks: false,
                    },
                    LocationKind::JsonTrust { trust },
                )
                | (
                    ProviderConfig::JsonMcp { spec: expected, .. },
                    LocationKind::JsonMcpWithTrust { trust, .. },
                ) => trust.spec == expected,
                (
                    ProviderConfig::TomlMcp { spec: expected, .. },
                    LocationKind::TomlMcp {
                        trust: Some(trust), ..
                    },
                ) => trust.spec == expected,
                _ => false,
            };
            assert!(wired, "{} registry config was not wired", provider.client);
        }
    }

    #[test]
    fn build_locations_covers_every_tool_family_under_fake_home() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let d = dir_context_under(tmp.path());
        let specs = build_locations(&d);

        // Sanity: catalog should include all major surfaces. Tweak as
        // `cmd_init` learns new files.
        let labels: Vec<&str> = specs.iter().map(|s| s.label).collect();
        for expected in [
            "Claude Code MCP",
            "Claude Code settings",
            "Claude Code /recall",
            "Claude Code /remember",
            "Claude Desktop MCP",
            "Codex CLI MCP",
            "Codex CLI hooks",
            "Gemini CLI",
            "Gemini CLI GEMINI.md",
            "Copilot CLI MCP",
            "Copilot CLI hooks",
            "Cursor MCP",
            "Cursor rule",
            "Roo Code rule",
            "Windsurf MCP",
            "VS Code MCP",
            "Cline (VS Code)",
            "Roo (VS Code)",
            "Kilo Code (VS Code)",
            "Zed MCP",
            "OpenCode MCP",
            "OpenCode plugin",
            "Amp MCP",
            "Amp /icm-recall",
            "Amp /icm-remember",
            "Pi AGENTS.md",
            "Pi /icm-recall",
            "Pi /icm-remember",
            "Amazon Q MCP",
            "Continue.dev",
            "CLAUDE.md (cwd)",
            "AGENTS.md (cwd)",
        ] {
            assert!(
                labels.contains(&expected),
                "catalog missing {expected}: {labels:?}"
            );
        }

        // Every non-data spec must live under the fake home or the fake
        // cwd; nothing should escape to the real filesystem.
        for spec in &specs {
            if spec.purge_data_only {
                continue; // data dirs are platform-global; not under tempdir
            }
            let p = spec.path.to_string_lossy();
            let under_home = p.starts_with(&*tmp.path().to_string_lossy());
            assert!(
                under_home,
                "{} escaped tempdir: {}",
                spec.label,
                spec.path.display()
            );
        }

        // At least 25 specs, per the original audit in issue #229.
        assert!(
            specs.len() >= 25,
            "catalog has only {} entries; expected at least 25",
            specs.len()
        );
    }

    #[test]
    fn data_dirs_are_marked_purge_only() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dir_context_under(tmp.path());
        let specs = build_locations(&d);
        let purge: Vec<&str> = specs
            .iter()
            .filter(|s| s.purge_data_only)
            .map(|s| s.label)
            .collect();
        // ProjectDirs is best-effort; on minimal sandboxes it can return
        // None, in which case nothing is marked purge-only. Assert at
        // most "if present, all data entries are flagged".
        for label in &purge {
            assert!(
                label.contains("database") || label.contains("cache"),
                "unexpected purge-only label: {label}"
            );
        }
    }
}
