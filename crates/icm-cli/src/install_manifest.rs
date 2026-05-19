//! Install manifest written by `icm init`.
//!
//! Every time `icm init` configures an AI tool, it records the touched
//! path here. The manifest persists across invocations: subsequent
//! `icm init` runs update entries in place, and `icm uninstall` (a
//! future PR) consumes it to know exactly what to clean up — without
//! having to derive the surface from a hard-coded list.
//!
//! Path: `<icm-data-dir>/install-manifest.json`
//! - Linux/WSL: `~/.local/share/icm/install-manifest.json`
//! - macOS:     `~/Library/Application Support/icm/install-manifest.json`
//! - Windows:   `%APPDATA%\icm\icm\data\install-manifest.json`
//!
//! Schema is versioned (`schema_version` field) so future migrations
//! stay backwards-compatible.

#![allow(dead_code)] // consumed by cmd_init in the next commit

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const CURRENT_SCHEMA: u32 = 1;

/// Top-level install manifest persisted at `<data_dir>/install-manifest.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct InstallManifest {
    /// Bumped on incompatible field changes. Always read on load; reject
    /// unknown versions with a clear error so older binaries don't
    /// silently truncate a newer manifest.
    pub schema_version: u32,
    /// Version of the `icm` binary that wrote / last updated this file.
    pub icm_version: String,
    /// ISO-8601 timestamp of the last write.
    pub updated_at: String,
    /// One entry per configuration target.
    pub entries: Vec<ManifestEntry>,
}

/// One configuration mutation recorded by init.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ManifestEntry {
    /// Absolute path of the configuration file ICM wrote to.
    pub path: PathBuf,
    /// Human-readable label of the AI tool ("Claude Code", "Codex CLI",
    /// "OpenCode plugin", "Cursor rule", ...).
    pub tool: String,
    /// What kind of mutation init performed at this path.
    pub kind: EntryKind,
    /// SHA-256 of the file contents before init touched it. `None` when
    /// the file did not exist (a pure-create write).
    pub sha256_before: Option<String>,
    /// File size in bytes before init touched it. 0 for pure creates.
    pub bytes_before: u64,
}

/// What `cmd_init` did at this path.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum EntryKind {
    /// JSON file with an `mcpServers.icm` (or sibling) entry inserted.
    JsonMcpServer,
    /// JSON file with hook entries inserted (Claude/Gemini/Codex shape).
    JsonHooks,
    /// JSON file with Copilot's `bash` field hooks.
    JsonCopilotHooks,
    /// TOML file (Codex `config.toml`) with `[mcp_servers.icm]`.
    TomlMcpServer,
    /// YAML file (Continue.dev) with a `- name: icm` block appended.
    YamlContinue,
    /// Markdown file with an `<!-- icm:start --> ... <!-- icm:end -->`
    /// block injected.
    MarkdownBlock,
    /// Whole-file artifact owned solely by init (skill / plugin).
    OwnedFile,
}

impl InstallManifest {
    /// Empty manifest scaffold.
    pub fn empty() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            icm_version: env!("CARGO_PKG_VERSION").to_string(),
            updated_at: iso_timestamp(),
            entries: Vec::new(),
        }
    }

    /// Read the manifest at `path`, or return an empty one if the file
    /// does not exist yet. Rejects unknown `schema_version`s loudly.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read manifest at {}", path.display()))?;
        let m: InstallManifest = serde_json::from_str(&raw)
            .with_context(|| format!("invalid JSON in manifest {}", path.display()))?;
        if m.schema_version > CURRENT_SCHEMA {
            anyhow::bail!(
                "install manifest {} was written by a newer icm \
                (schema {} > {}). Upgrade icm or back up the manifest \
                before re-running init.",
                path.display(),
                m.schema_version,
                CURRENT_SCHEMA,
            );
        }
        Ok(m)
    }

    /// Write the manifest, creating the parent directory if needed.
    /// Bumps `updated_at` and `icm_version` on every save.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.updated_at = iso_timestamp();
        self.icm_version = env!("CARGO_PKG_VERSION").to_string();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("cannot write manifest {}", path.display()))?;
        Ok(())
    }

    /// Record (or update) an entry for `path`. If an entry with the
    /// same path already exists, its metadata is left intact —
    /// `sha256_before` reflects the state **before init ever touched
    /// the path**, not the state before this particular run.
    pub fn record(&mut self, entry: ManifestEntry) {
        if self.entries.iter().any(|e| e.path == entry.path) {
            return;
        }
        self.entries.push(entry);
    }

    /// Build a `ManifestEntry` by inspecting `path` on disk. Caller
    /// should invoke this **before** the mutation so the hash captures
    /// the pre-mutation state.
    pub fn entry_from_disk(path: &Path, tool: &str, kind: EntryKind) -> Result<ManifestEntry> {
        if !path.exists() {
            return Ok(ManifestEntry {
                path: path.to_path_buf(),
                tool: tool.to_string(),
                kind,
                sha256_before: None,
                bytes_before: 0,
            });
        }
        let meta =
            std::fs::metadata(path).with_context(|| format!("cannot stat {}", path.display()))?;
        let bytes_before = meta.len();
        let sha256_before = Some(sha256_of(path)?);
        Ok(ManifestEntry {
            path: path.to_path_buf(),
            tool: tool.to_string(),
            kind,
            sha256_before,
            bytes_before,
        })
    }

    /// Number of recorded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Resolve the manifest path from `ProjectDirs`. Falls back to
/// `<cwd>/install-manifest.json` only when ProjectDirs is unavailable
/// (stripped sandboxes).
pub(crate) fn default_manifest_path() -> PathBuf {
    directories::ProjectDirs::from("dev", "icm", "icm")
        .map(|d| d.data_dir().join("install-manifest.json"))
        .unwrap_or_else(|| PathBuf::from("install-manifest.json"))
}

fn sha256_of(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// `YYYY-MM-DDTHH:MM:SSZ` UTC. Manifest is JSON so colons are fine
/// here, unlike the backup directory name.
fn iso_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymdhms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn epoch_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let sec_of_day = secs % 86_400;
    let h = (sec_of_day / 3600) as u32;
    let mi = ((sec_of_day % 3600) / 60) as u32;
    let s = (sec_of_day % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (y as i32, mo, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_has_current_schema_and_no_entries() {
        let m = InstallManifest::empty();
        assert_eq!(m.schema_version, CURRENT_SCHEMA);
        assert!(m.entries.is_empty());
        assert!(!m.icm_version.is_empty());
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist.json");
        let m = InstallManifest::load(&missing).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/install-manifest.json");
        let mut m = InstallManifest::empty();
        m.record(ManifestEntry {
            path: PathBuf::from("/x/.claude.json"),
            tool: "Claude Code".into(),
            kind: EntryKind::JsonMcpServer,
            sha256_before: Some("abc".into()),
            bytes_before: 42,
        });
        m.save(&path).unwrap();

        let m2 = InstallManifest::load(&path).unwrap();
        assert_eq!(m2.entries.len(), 1);
        assert_eq!(m2.entries[0].tool, "Claude Code");
        assert_eq!(m2.entries[0].kind, EntryKind::JsonMcpServer);
    }

    #[test]
    fn record_is_idempotent_per_path() {
        let mut m = InstallManifest::empty();
        let entry1 = ManifestEntry {
            path: PathBuf::from("/x"),
            tool: "A".into(),
            kind: EntryKind::JsonMcpServer,
            sha256_before: Some("aa".into()),
            bytes_before: 1,
        };
        let entry2 = ManifestEntry {
            path: PathBuf::from("/x"),
            tool: "B".into(),
            kind: EntryKind::TomlMcpServer,
            sha256_before: Some("bb".into()),
            bytes_before: 2,
        };
        m.record(entry1);
        m.record(entry2);
        assert_eq!(m.entries.len(), 1);
        // First write wins — preserves the pre-mutation state.
        assert_eq!(m.entries[0].tool, "A");
        assert_eq!(m.entries[0].sha256_before.as_deref(), Some("aa"));
    }

    #[test]
    fn entry_from_disk_captures_pre_mutation_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.json");
        std::fs::write(&path, "hello").unwrap();
        let entry = InstallManifest::entry_from_disk(&path, "Test", EntryKind::OwnedFile).unwrap();
        assert_eq!(entry.bytes_before, 5);
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            entry.sha256_before.as_deref(),
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }

    #[test]
    fn entry_from_disk_handles_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("no.json");
        let entry =
            InstallManifest::entry_from_disk(&missing, "Test", EntryKind::OwnedFile).unwrap();
        assert_eq!(entry.bytes_before, 0);
        assert!(entry.sha256_before.is_none());
    }

    #[test]
    fn load_rejects_unknown_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":{},"icm_version":"99","updated_at":"x","entries":[]}}"#,
                CURRENT_SCHEMA + 1
            ),
        )
        .unwrap();
        let err = InstallManifest::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("newer icm"));
    }

    #[test]
    fn iso_timestamp_known_reference_point() {
        let (y, mo, d, h, mi, s) = epoch_to_ymdhms(1_700_000_000);
        assert_eq!((y, mo, d, h, mi, s), (2023, 11, 14, 22, 13, 20));
    }
}
