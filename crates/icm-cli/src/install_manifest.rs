//! Install manifest written by `icm init`.
//!
//! Every time `icm init` configures an AI tool, it records the touched
//! path here. The manifest persists across invocations: subsequent
//! `icm init` runs update entries in place, and `icm uninstall` consumes
//! its ownership metadata to avoid removing matching user configuration.
//!
//! Path: `<icm-data-dir>/install-manifest.json`
//! - Linux/WSL: `~/.local/share/icm/install-manifest.json`
//! - macOS:     `~/Library/Application Support/icm/install-manifest.json`
//! - Windows:   `%APPDATA%\icm\icm\data\install-manifest.json`
//!
//! Schema is versioned (`schema_version` field) so future migrations
//! stay backwards-compatible.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::trusted_mcp::TrustChange;

const CURRENT_SCHEMA: u32 = 2;

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
    /// Exact trust values inserted by ICM. Missing on legacy manifests,
    /// where uninstall retains its historical exact-match fallback. An
    /// explicit empty list means init saw matching user values and owns none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_changes: Option<Vec<TrustChange>>,
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
        let mut m: InstallManifest = serde_json::from_str(&raw)
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
        if m.schema_version == 1 {
            m.schema_version = CURRENT_SCHEMA;
        }
        Ok(m)
    }

    /// Write the manifest, creating the parent directory if needed.
    /// Bumps `updated_at` and `icm_version` on every save.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.schema_version = CURRENT_SCHEMA;
        self.updated_at = iso_timestamp();
        self.icm_version = env!("CARGO_PKG_VERSION").to_string();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        crate::uninstall::atomic_write(path, json.as_bytes())
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
                trust_changes: Some(Vec::new()),
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
            trust_changes: Some(Vec::new()),
        })
    }

    /// Add newly inserted trust values without discarding ownership from a
    /// previous idempotent init run. A legacy entry stays legacy when this
    /// invocation added nothing, preserving its documented fallback.
    pub fn record_trust_changes(&mut self, path: &Path, changes: Vec<TrustChange>) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.path == path) else {
            return;
        };
        if changes.is_empty() {
            return;
        }
        let owned = entry.trust_changes.get_or_insert_with(Vec::new);
        for change in changes {
            if !owned.contains(&change) {
                owned.push(change);
            }
        }
    }

    /// Establish an authoritative ownership record before a trust config is
    /// mutated. Persisting this empty marker first makes an interrupted init
    /// conservative: uninstall preserves values whose ownership was never
    /// durably recorded instead of applying the legacy exact-match fallback.
    pub fn ensure_trust_tracking(&mut self, path: &Path) -> Result<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.path == path)
            .with_context(|| format!("no install manifest entry for {}", path.display()))?;
        entry.trust_changes.get_or_insert_with(Vec::new);
        Ok(())
    }

    /// `None` distinguishes a manifest written before trust provenance was
    /// available from an explicit record that owns zero values.
    pub fn trust_changes_for(&self, path: &Path) -> Option<&[TrustChange]> {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.trust_changes.as_deref())
    }

    /// Consume trust ownership after a successful uninstall mutation. Keeping
    /// an explicit empty list prevents a later identical user value from being
    /// mistaken for residue by a stale manifest entry.
    pub fn clear_trust_changes(&mut self, path: &Path) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.path == path) else {
            return false;
        };
        if entry
            .trust_changes
            .as_ref()
            .is_some_and(|changes| changes.is_empty())
        {
            return false;
        }
        entry.trust_changes = Some(Vec::new());
        true
    }

    /// Reconcile a provenance-aware entry with values still present on disk.
    /// Legacy entries remain untouched because they intentionally use fallback
    /// discovery rather than an authoritative ownership list.
    pub fn replace_recorded_trust_changes(
        &mut self,
        path: &Path,
        present: Vec<TrustChange>,
    ) -> bool {
        let Some(changes) = self
            .entries
            .iter_mut()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.trust_changes.as_mut())
        else {
            return false;
        };
        if *changes == present {
            return false;
        }
        *changes = present;
        true
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
            trust_changes: Some(Vec::new()),
        });
        m.save(&path).unwrap();

        let m2 = InstallManifest::load(&path).unwrap();
        assert_eq!(m2.entries.len(), 1);
        assert_eq!(m2.entries[0].tool, "Claude Code");
        assert_eq!(m2.entries[0].kind, EntryKind::JsonMcpServer);
        assert_eq!(m2.entries[0].trust_changes, Some(Vec::new()));
    }

    #[test]
    fn trust_provenance_uses_a_schema_old_writers_must_reject() {
        assert_eq!(CURRENT_SCHEMA, 2);

        let mut manifest = InstallManifest::empty();
        manifest.record(ManifestEntry {
            path: PathBuf::from("/x/settings.json"),
            tool: "Claude Code".into(),
            kind: EntryKind::JsonHooks,
            sha256_before: None,
            bytes_before: 0,
            trust_changes: Some(vec![TrustChange::JsonArrayMember {
                path: vec!["permissions".into(), "allow".into()],
                value: "mcp__icm__icm_memory_recall".into(),
            }]),
        });

        let serialized = serde_json::to_value(&manifest).unwrap();
        assert_eq!(serialized["schema_version"], 2);
        assert!(serialized["entries"][0].get("trust_changes").is_some());
        assert!(serialized["schema_version"].as_u64().unwrap() > 1);
    }

    #[test]
    fn load_migrates_schema_one_and_save_forces_current_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("install-manifest.json");
        std::fs::write(
            &path,
            r#"{
                "schema_version": 1,
                "icm_version": "0.10.61",
                "updated_at": "2026-01-01T00:00:00Z",
                "entries": []
            }"#,
        )
        .unwrap();

        let mut manifest = InstallManifest::load(&path).unwrap();
        assert_eq!(manifest.schema_version, 2);

        manifest.schema_version = 1;
        manifest.save(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], 2);
    }

    #[test]
    fn legacy_entry_without_trust_changes_keeps_fallback_marker() {
        let raw = r#"{
            "schema_version": 1,
            "icm_version": "0.10.61",
            "updated_at": "2026-01-01T00:00:00Z",
            "entries": [{
                "path": "/x/settings.json",
                "tool": "Claude Code",
                "kind": "JsonHooks",
                "sha256_before": null,
                "bytes_before": 0
            }]
        }"#;
        let mut manifest: InstallManifest = serde_json::from_str(raw).unwrap();
        assert!(manifest.entries[0].trust_changes.is_none());
        assert!(manifest.clear_trust_changes(Path::new("/x/settings.json")));
        assert_eq!(manifest.entries[0].trust_changes, Some(Vec::new()));
    }

    #[test]
    fn trust_ownership_accumulates_without_claiming_idempotent_values() {
        let path = PathBuf::from("/x/settings.json");
        let mut manifest = InstallManifest::empty();
        manifest.record(ManifestEntry {
            path: path.clone(),
            tool: "Claude Code".into(),
            kind: EntryKind::JsonHooks,
            sha256_before: None,
            bytes_before: 0,
            trust_changes: Some(Vec::new()),
        });
        let change = TrustChange::JsonArrayMember {
            path: vec!["permissions".into(), "allow".into()],
            value: "mcp__icm__icm_memory_recall".into(),
        };
        manifest.record_trust_changes(&path, vec![change.clone(), change.clone()]);
        manifest.record_trust_changes(&path, Vec::new());
        assert_eq!(manifest.trust_changes_for(&path), Some([change].as_slice()));
        assert!(manifest.clear_trust_changes(&path));
        assert_eq!(
            manifest.trust_changes_for(&path),
            Some(&[] as &[TrustChange])
        );
        assert!(!manifest.clear_trust_changes(&path));
        assert!(!manifest.replace_recorded_trust_changes(&path, Vec::new()));
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
            trust_changes: Some(Vec::new()),
        };
        let entry2 = ManifestEntry {
            path: PathBuf::from("/x"),
            tool: "B".into(),
            kind: EntryKind::TomlMcpServer,
            sha256_before: Some("bb".into()),
            bytes_before: 2,
            trust_changes: Some(Vec::new()),
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
