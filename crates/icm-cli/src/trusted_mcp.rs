//! Declarative trust policy for local ICM MCP tools.
//!
//! Provider descriptors stay deliberately finite and source-controlled.
//! They are interpreted by the same apply, discovery, and uninstall code,
//! so adding a provider does not create another lifecycle implementation.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Only the read and write primitives needed for persistent memory are
/// approved automatically. Destructive and administrative tools continue
/// to require the client's normal confirmation flow.
pub(crate) const TRUSTED_LOCAL_TOOLS: [&str; 2] = ["icm_memory_recall", "icm_memory_store"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderId {
    Claude,
    Cursor,
    Zed,
    Codex,
    OpenCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderConfigPath {
    ClaudeSettings,
    CursorPermissions,
    ZedSettings,
    CodexConfig,
    OpenCodeConfig,
}

pub(crate) struct ProviderPathContext<'a> {
    pub home: &'a Path,
    pub claude_dir: &'a Path,
    pub codex_dir: &'a Path,
    pub zed_settings: &'a Path,
}

impl ProviderConfigPath {
    pub(crate) fn resolve(self, paths: &ProviderPathContext<'_>) -> PathBuf {
        match self {
            Self::ClaudeSettings => paths.claude_dir.join("settings.json"),
            Self::CursorPermissions => paths.home.join(".cursor/permissions.json"),
            Self::ZedSettings => paths.zed_settings.to_path_buf(),
            Self::CodexConfig => paths.codex_dir.join("config.toml"),
            Self::OpenCodeConfig => paths.home.join(".config/opencode/opencode.json"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JsonTrustShape {
    StringArray,
    StringMap {
        value: &'static str,
    },
    ObjectMapField {
        field: &'static str,
        value: &'static str,
    },
}

/// A provider's JSON representation of one permission per trusted tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JsonTrustSpec {
    provider: ProviderId,
    path: &'static [&'static str],
    key_prefix: &'static str,
    shape: JsonTrustShape,
}

/// Codex keeps tool approval inside the ICM server table rather than in a
/// separate permission document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TomlTrustSpec {
    provider: ProviderId,
    path: &'static [&'static str],
    field: &'static str,
    value: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderConfig {
    JsonTrust {
        spec: JsonTrustSpec,
        has_hooks: bool,
    },
    JsonMcp {
        spec: JsonTrustSpec,
        servers_key: &'static str,
    },
    TomlMcp {
        spec: TomlTrustSpec,
        table: &'static str,
        entry: &'static str,
    },
}

impl ProviderConfig {
    pub(crate) const fn provider_id(self) -> ProviderId {
        match self {
            Self::JsonTrust { spec, .. } | Self::JsonMcp { spec, .. } => spec.provider,
            Self::TomlMcp { spec, .. } => spec.provider,
        }
    }

    pub(crate) const fn includes_mcp_server(self) -> bool {
        matches!(self, Self::JsonMcp { .. } | Self::TomlMcp { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrustedProvider {
    pub client: &'static str,
    pub uninstall_label: &'static str,
    pub path: ProviderConfigPath,
    pub config: ProviderConfig,
}

impl TrustedProvider {
    pub(crate) const fn id(self) -> ProviderId {
        self.config.provider_id()
    }

    pub(crate) fn config_path(self, paths: &ProviderPathContext<'_>) -> PathBuf {
        self.path.resolve(paths)
    }

    #[cfg(test)]
    pub(crate) const fn json_spec(self) -> Option<JsonTrustSpec> {
        match self.config {
            ProviderConfig::JsonTrust { spec, .. } | ProviderConfig::JsonMcp { spec, .. } => {
                Some(spec)
            }
            ProviderConfig::TomlMcp { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn toml_spec(self) -> Option<TomlTrustSpec> {
        match self.config {
            ProviderConfig::TomlMcp { spec, .. } => Some(spec),
            ProviderConfig::JsonTrust { .. } | ProviderConfig::JsonMcp { .. } => None,
        }
    }
}

pub(crate) const TRUSTED_PROVIDERS: [TrustedProvider; 5] = [
    TrustedProvider {
        client: "Claude Code",
        uninstall_label: "Claude Code settings",
        path: ProviderConfigPath::ClaudeSettings,
        config: ProviderConfig::JsonTrust {
            spec: JsonTrustSpec {
                provider: ProviderId::Claude,
                path: &["permissions", "allow"],
                key_prefix: "mcp__icm__",
                shape: JsonTrustShape::StringArray,
            },
            has_hooks: true,
        },
    },
    TrustedProvider {
        client: "Cursor",
        uninstall_label: "Cursor permissions",
        path: ProviderConfigPath::CursorPermissions,
        config: ProviderConfig::JsonTrust {
            spec: JsonTrustSpec {
                provider: ProviderId::Cursor,
                path: &["mcpAllowlist"],
                key_prefix: "icm:",
                shape: JsonTrustShape::StringArray,
            },
            has_hooks: false,
        },
    },
    TrustedProvider {
        client: "Zed",
        uninstall_label: "Zed MCP",
        path: ProviderConfigPath::ZedSettings,
        config: ProviderConfig::JsonMcp {
            spec: JsonTrustSpec {
                provider: ProviderId::Zed,
                path: &["agent", "tool_permissions", "tools"],
                key_prefix: "mcp:icm:",
                shape: JsonTrustShape::ObjectMapField {
                    field: "default",
                    value: "allow",
                },
            },
            servers_key: "context_servers",
        },
    },
    TrustedProvider {
        client: "Codex CLI",
        uninstall_label: "Codex CLI MCP",
        path: ProviderConfigPath::CodexConfig,
        config: ProviderConfig::TomlMcp {
            spec: TomlTrustSpec {
                provider: ProviderId::Codex,
                path: &["mcp_servers", "icm", "tools"],
                field: "approval_mode",
                value: "approve",
            },
            table: "mcp_servers",
            entry: "icm",
        },
    },
    TrustedProvider {
        client: "OpenCode",
        uninstall_label: "OpenCode MCP",
        path: ProviderConfigPath::OpenCodeConfig,
        config: ProviderConfig::JsonMcp {
            spec: JsonTrustSpec {
                provider: ProviderId::OpenCode,
                path: &["permission"],
                key_prefix: "icm_",
                shape: JsonTrustShape::StringMap { value: "allow" },
            },
            servers_key: "mcp",
        },
    },
];

pub(crate) fn trusted_provider(id: ProviderId) -> &'static TrustedProvider {
    let index = match id {
        ProviderId::Claude => 0,
        ProviderId::Cursor => 1,
        ProviderId::Zed => 2,
        ProviderId::Codex => 3,
        ProviderId::OpenCode => 4,
    };
    &TRUSTED_PROVIDERS[index]
}

impl JsonTrustSpec {
    pub(crate) fn client(self) -> &'static str {
        trusted_provider(self.provider).client
    }
}

impl TomlTrustSpec {
    pub(crate) fn client(self) -> &'static str {
        trusted_provider(self.provider).client
    }
}

/// One exact config value inserted by ICM. The install manifest persists
/// these changes so uninstall does not claim identical user-authored values.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TrustChange {
    JsonArrayMember {
        path: Vec<String>,
        value: String,
    },
    JsonMapEntry {
        path: Vec<String>,
        key: String,
        value: String,
    },
    JsonObjectField {
        path: Vec<String>,
        key: String,
        field: String,
        value: String,
    },
    TomlString {
        path: Vec<String>,
        value: String,
    },
}

impl TrustChange {
    pub(crate) fn permission_label(&self) -> Option<&str> {
        match self {
            Self::JsonArrayMember { value, .. } => Some(value),
            Self::JsonMapEntry { key, .. } | Self::JsonObjectField { key, .. } => Some(key),
            Self::TomlString { path, .. } => path.iter().rev().nth(1).map(String::as_str),
        }
    }
}

impl JsonTrustSpec {
    fn desired_changes(self) -> Vec<TrustChange> {
        let path: Vec<String> = self.path.iter().map(|part| (*part).to_string()).collect();
        TRUSTED_LOCAL_TOOLS
            .iter()
            .map(|tool| {
                let rule = format!("{}{}", self.key_prefix, tool);
                match self.shape {
                    JsonTrustShape::StringArray => TrustChange::JsonArrayMember {
                        path: path.clone(),
                        value: rule,
                    },
                    JsonTrustShape::StringMap { value } => TrustChange::JsonMapEntry {
                        path: path.clone(),
                        key: rule,
                        value: value.to_string(),
                    },
                    JsonTrustShape::ObjectMapField { field, value } => {
                        TrustChange::JsonObjectField {
                            path: path.clone(),
                            key: rule,
                            field: field.to_string(),
                            value: value.to_string(),
                        }
                    }
                }
            })
            .collect()
    }
}

impl TomlTrustSpec {
    fn desired_changes(self) -> Vec<TrustChange> {
        TRUSTED_LOCAL_TOOLS
            .iter()
            .map(|tool| {
                let mut path: Vec<String> =
                    self.path.iter().map(|part| (*part).to_string()).collect();
                path.push((*tool).to_string());
                path.push(self.field.to_string());
                TrustChange::TomlString {
                    path,
                    value: self.value.to_string(),
                }
            })
            .collect()
    }
}

/// Apply all missing JSON permissions and return only the values inserted by
/// this invocation. Existing values are left untouched and are not owned.
pub(crate) fn apply_json_trust(root: &mut Value, spec: JsonTrustSpec) -> Result<Vec<TrustChange>> {
    let mut inserted = Vec::new();
    for change in spec.desired_changes() {
        if apply_json_change(root, &change)? {
            inserted.push(change);
        }
    }
    Ok(inserted)
}

/// Find exact provider permissions. `None` enables the pre-provenance legacy
/// policy; a recorded slice limits discovery to values ICM actually inserted.
pub(crate) fn discover_json_trust(
    root: &Value,
    spec: JsonTrustSpec,
    owned: Option<&[TrustChange]>,
) -> Vec<TrustChange> {
    candidate_json_changes(spec, owned)
        .into_iter()
        .filter(|change| json_change_is_present(root, change))
        .collect()
}

/// Remove exact provider permissions and return the number of config values
/// removed. Empty containers are collapsed only after an owned value changed.
pub(crate) fn strip_json_trust(
    root: &mut Value,
    spec: JsonTrustSpec,
    owned: Option<&[TrustChange]>,
) -> usize {
    let legacy = owned.is_none();
    candidate_json_changes(spec, owned)
        .iter()
        .map(|change| remove_json_change(root, change, legacy))
        .sum()
}

/// Apply Codex's missing tool approvals and return the exact fields inserted.
pub(crate) fn apply_toml_trust(
    root: &mut toml::Value,
    spec: TomlTrustSpec,
) -> Result<Vec<TrustChange>> {
    let mut inserted = Vec::new();
    for change in spec.desired_changes() {
        if apply_toml_change(root, &change)? {
            inserted.push(change);
        }
    }
    Ok(inserted)
}

pub(crate) fn discover_toml_trust(
    root: &toml::Value,
    spec: TomlTrustSpec,
    owned: Option<&[TrustChange]>,
) -> Vec<TrustChange> {
    candidate_toml_changes(spec, owned)
        .into_iter()
        .filter(|change| toml_change_is_present(root, change))
        .collect()
}

pub(crate) fn strip_toml_trust(
    root: &mut toml::Value,
    spec: TomlTrustSpec,
    owned: Option<&[TrustChange]>,
) -> usize {
    candidate_toml_changes(spec, owned)
        .iter()
        .map(|change| remove_toml_change(root, change))
        .sum()
}

fn candidate_json_changes(spec: JsonTrustSpec, owned: Option<&[TrustChange]>) -> Vec<TrustChange> {
    let desired = spec.desired_changes();
    match owned {
        None => desired,
        Some(changes) => changes
            .iter()
            .filter(|change| desired.contains(change))
            .cloned()
            .collect(),
    }
}

fn candidate_toml_changes(spec: TomlTrustSpec, owned: Option<&[TrustChange]>) -> Vec<TrustChange> {
    let desired = spec.desired_changes();
    match owned {
        None => desired,
        Some(changes) => changes
            .iter()
            .filter(|change| desired.contains(change))
            .cloned()
            .collect(),
    }
}

fn apply_json_change(root: &mut Value, change: &TrustChange) -> Result<bool> {
    match change {
        TrustChange::JsonArrayMember { path, value } => {
            let slot = json_path_mut(root, path, JsonContainer::Array)?;
            let values = slot
                .as_array_mut()
                .with_context(|| format!("JSON trust path {} is not an array", path_label(path)))?;
            if values.iter().any(|item| item.as_str() == Some(value)) {
                Ok(false)
            } else {
                values.push(Value::String(value.clone()));
                Ok(true)
            }
        }
        TrustChange::JsonMapEntry { path, key, value } => {
            let slot = json_path_mut(root, path, JsonContainer::Object)?;
            let values = slot.as_object_mut().with_context(|| {
                format!("JSON trust path {} is not an object", path_label(path))
            })?;
            if values.contains_key(key) {
                Ok(false)
            } else {
                values.insert(key.clone(), Value::String(value.clone()));
                Ok(true)
            }
        }
        TrustChange::JsonObjectField {
            path,
            key,
            field,
            value,
        } => {
            let slot = json_path_mut(root, path, JsonContainer::Object)?;
            let values = slot.as_object_mut().with_context(|| {
                format!("JSON trust path {} is not an object", path_label(path))
            })?;
            if let Some(existing) = values.get_mut(key) {
                let Some(entry) = existing.as_object_mut() else {
                    return Ok(false);
                };
                if entry.contains_key(field) {
                    Ok(false)
                } else {
                    entry.insert(field.clone(), Value::String(value.clone()));
                    Ok(true)
                }
            } else {
                let mut entry = Map::new();
                entry.insert(field.clone(), Value::String(value.clone()));
                values.insert(key.clone(), Value::Object(entry));
                Ok(true)
            }
        }
        TrustChange::TomlString { .. } => Ok(false),
    }
}

#[derive(Clone, Copy)]
enum JsonContainer {
    Array,
    Object,
}

impl JsonContainer {
    fn empty(self) -> Value {
        match self {
            Self::Array => Value::Array(Vec::new()),
            Self::Object => Value::Object(Map::new()),
        }
    }
}

fn json_path_mut<'a>(
    value: &'a mut Value,
    path: &[String],
    leaf: JsonContainer,
) -> Result<&'a mut Value> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    let object = value.as_object_mut().with_context(|| {
        format!(
            "JSON trust parent for {} is not an object",
            path_label(path)
        )
    })?;
    let child = object.entry(head.clone()).or_insert_with(|| {
        if tail.is_empty() {
            leaf.empty()
        } else {
            Value::Object(Map::new())
        }
    });
    json_path_mut(child, tail, leaf)
}

fn json_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = value;
    for part in path {
        current = current.as_object()?.get(part)?;
    }
    Some(current)
}

fn json_change_is_present(root: &Value, change: &TrustChange) -> bool {
    match change {
        TrustChange::JsonArrayMember { path, value } => json_path(root, path)
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(value))),
        TrustChange::JsonMapEntry { path, key, value } => {
            json_path(root, path)
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(key))
                .and_then(Value::as_str)
                == Some(value)
        }
        TrustChange::JsonObjectField {
            path,
            key,
            field,
            value,
        } => {
            json_path(root, path)
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(key))
                .and_then(Value::as_object)
                .and_then(|entry| entry.get(field))
                .and_then(Value::as_str)
                == Some(value)
        }
        TrustChange::TomlString { .. } => false,
    }
}

fn remove_json_change(root: &mut Value, change: &TrustChange, legacy: bool) -> usize {
    let (removed, cleanup_path) = match change {
        TrustChange::JsonArrayMember { path, value } => {
            let Some(items) = json_path_mut_existing(root, path).and_then(Value::as_array_mut)
            else {
                return 0;
            };
            let removed = if legacy {
                let before = items.len();
                items.retain(|item| item.as_str() != Some(value));
                before - items.len()
            } else if let Some(index) = items.iter().position(|item| item.as_str() == Some(value)) {
                items.remove(index);
                1
            } else {
                0
            };
            (removed, path.as_slice())
        }
        TrustChange::JsonMapEntry { path, key, value } => {
            let Some(entries) = json_path_mut_existing(root, path).and_then(Value::as_object_mut)
            else {
                return 0;
            };
            let removed = if entries.get(key).and_then(Value::as_str) == Some(value) {
                entries.remove(key);
                1
            } else {
                0
            };
            (removed, path.as_slice())
        }
        TrustChange::JsonObjectField {
            path,
            key,
            field,
            value,
        } => {
            let Some(entries) = json_path_mut_existing(root, path).and_then(Value::as_object_mut)
            else {
                return 0;
            };
            let Some(entry) = entries.get_mut(key).and_then(Value::as_object_mut) else {
                return 0;
            };
            let removed = if entry.get(field).and_then(Value::as_str) == Some(value) {
                entry.remove(field);
                1
            } else {
                0
            };
            if removed > 0 && entry.is_empty() {
                entries.remove(key);
            }
            (removed, path.as_slice())
        }
        TrustChange::TomlString { .. } => return 0,
    };

    if removed > 0 {
        cleanup_json_path(root, cleanup_path);
    }
    removed
}

fn json_path_mut_existing<'a>(value: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let Some((head, tail)) = path.split_first() else {
        return Some(value);
    };
    let child = value.as_object_mut()?.get_mut(head)?;
    json_path_mut_existing(child, tail)
}

fn cleanup_json_path(value: &mut Value, path: &[String]) -> bool {
    let Some((head, tail)) = path.split_first() else {
        return json_container_is_empty(value);
    };
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    let remove_child = if let Some(child) = object.get_mut(head) {
        if tail.is_empty() {
            json_container_is_empty(child)
        } else {
            cleanup_json_path(child, tail)
        }
    } else {
        false
    };
    if remove_child {
        object.remove(head);
    }
    object.is_empty()
}

fn json_container_is_empty(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.is_empty())
        || value.as_array().is_some_and(|array| array.is_empty())
}

fn apply_toml_change(root: &mut toml::Value, change: &TrustChange) -> Result<bool> {
    let TrustChange::TomlString { path, value } = change else {
        return Ok(false);
    };
    let Some((field, parents)) = path.split_last() else {
        return Ok(false);
    };
    let table = toml_table_mut(root, parents)?;
    if table.contains_key(field) {
        Ok(false)
    } else {
        table.insert(field.clone(), toml::Value::String(value.clone()));
        Ok(true)
    }
}

fn toml_table_mut<'a>(
    value: &'a mut toml::Value,
    path: &[String],
) -> Result<&'a mut toml::map::Map<String, toml::Value>> {
    let table = value
        .as_table_mut()
        .with_context(|| format!("TOML trust parent for {} is not a table", path_label(path)))?;
    let Some((head, tail)) = path.split_first() else {
        return Ok(table);
    };
    let child = table
        .entry(head.clone())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    toml_table_mut(child, tail)
}

fn toml_path<'a>(value: &'a toml::Value, path: &[String]) -> Option<&'a toml::Value> {
    let mut current = value;
    for part in path {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

fn toml_change_is_present(root: &toml::Value, change: &TrustChange) -> bool {
    match change {
        TrustChange::TomlString { path, value } => {
            toml_path(root, path).and_then(toml::Value::as_str) == Some(value)
        }
        _ => false,
    }
}

fn remove_toml_change(root: &mut toml::Value, change: &TrustChange) -> usize {
    let TrustChange::TomlString { path, value } = change else {
        return 0;
    };
    let Some((field, parents)) = path.split_last() else {
        return 0;
    };
    let Some(parent) = toml_table_mut_existing(root, parents) else {
        return 0;
    };
    if parent.get(field).and_then(toml::Value::as_str) != Some(value) {
        return 0;
    }
    parent.remove(field);
    cleanup_toml_path(root, parents);
    1
}

fn toml_table_mut_existing<'a>(
    value: &'a mut toml::Value,
    path: &[String],
) -> Option<&'a mut toml::map::Map<String, toml::Value>> {
    let table = value.as_table_mut()?;
    let Some((head, tail)) = path.split_first() else {
        return Some(table);
    };
    let child = table.get_mut(head)?;
    toml_table_mut_existing(child, tail)
}

fn cleanup_toml_path(value: &mut toml::Value, path: &[String]) -> bool {
    let Some((head, tail)) = path.split_first() else {
        return value.as_table().is_some_and(|table| table.is_empty());
    };
    let Some(table) = value.as_table_mut() else {
        return false;
    };
    let remove_child = table
        .get_mut(head)
        .is_some_and(|child| cleanup_toml_path(child, tail));
    if remove_child {
        table.remove(head);
    }
    table.is_empty()
}

fn path_label(path: &[String]) -> String {
    path.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn json_spec(id: ProviderId) -> JsonTrustSpec {
        trusted_provider(id)
            .json_spec()
            .expect("provider must use JSON trust")
    }

    fn toml_spec(id: ProviderId) -> TomlTrustSpec {
        trusted_provider(id)
            .toml_spec()
            .expect("provider must use TOML trust")
    }

    #[test]
    fn provider_registry_shares_apply_discover_and_strip_lifecycle() {
        for provider in TRUSTED_PROVIDERS {
            assert_eq!(*trusted_provider(provider.id()), provider);
            match provider.config {
                ProviderConfig::JsonTrust { spec, .. } | ProviderConfig::JsonMcp { spec, .. } => {
                    let mut value = json!({});
                    let inserted =
                        apply_json_trust(&mut value, spec).expect("provider spec applies");
                    assert_eq!(
                        inserted.len(),
                        TRUSTED_LOCAL_TOOLS.len(),
                        "{}",
                        provider.client
                    );
                    assert_eq!(spec.client(), provider.client);
                    assert_eq!(
                        discover_json_trust(&value, spec, Some(&inserted)),
                        inserted,
                        "{}",
                        provider.client
                    );
                    assert_eq!(strip_json_trust(&mut value, spec, Some(&inserted)), 2);
                    assert_eq!(value, json!({}), "{}", provider.client);
                }
                ProviderConfig::TomlMcp { spec, .. } => {
                    let mut value = toml::Value::Table(toml::map::Map::new());
                    let inserted =
                        apply_toml_trust(&mut value, spec).expect("provider spec applies");
                    assert_eq!(inserted.len(), TRUSTED_LOCAL_TOOLS.len());
                    assert_eq!(spec.client(), provider.client);
                    assert_eq!(discover_toml_trust(&value, spec, Some(&inserted)), inserted);
                    assert_eq!(strip_toml_trust(&mut value, spec, Some(&inserted)), 2);
                    assert_eq!(value, toml::Value::Table(toml::map::Map::new()));
                }
            }
        }
    }

    #[test]
    fn json_apply_is_idempotent_and_does_not_claim_existing_values() {
        let mut value = json!({});
        let spec = json_spec(ProviderId::Claude);
        let first = apply_json_trust(&mut value, spec).unwrap();
        let after_first = value.clone();
        let second = apply_json_trust(&mut value, spec).unwrap();
        assert_eq!(first.len(), 2);
        assert!(second.is_empty());
        assert_eq!(value, after_first);
    }

    #[test]
    fn recorded_empty_ownership_preserves_matching_user_values() {
        let mut value = json!({
            "permission": {
                "icm_icm_memory_recall": "allow",
                "icm_icm_memory_store": "allow"
            }
        });
        let before = value.clone();
        let spec = json_spec(ProviderId::OpenCode);
        assert!(discover_json_trust(&value, spec, Some(&[])).is_empty());
        assert_eq!(strip_json_trust(&mut value, spec, Some(&[])), 0);
        assert_eq!(value, before);
    }

    #[test]
    fn recorded_subset_removes_only_owned_value() {
        let mut value = json!({});
        let spec = json_spec(ProviderId::Cursor);
        let owned = apply_json_trust(&mut value, spec).unwrap();
        assert_eq!(strip_json_trust(&mut value, spec, Some(&owned[..1])), 1);
        assert!(json_change_is_present(&value, &owned[1]));
    }

    #[test]
    fn legacy_json_cleanup_removes_exact_matches() {
        let mut value = json!({
            "permissions": {"allow": [
                "Read",
                "mcp__icm__icm_memory_recall",
                "mcp__icm__icm_memory_store"
            ]}
        });
        assert_eq!(
            strip_json_trust(&mut value, json_spec(ProviderId::Claude), None),
            2
        );
        assert_eq!(value, json!({"permissions": {"allow": ["Read"]}}));
    }

    #[test]
    fn no_op_strip_does_not_clean_empty_user_containers() {
        let mut value = json!({"agent": {"tool_permissions": {"tools": {}}}});
        let before = value.clone();
        assert_eq!(
            strip_json_trust(&mut value, json_spec(ProviderId::Zed), None),
            0
        );
        assert_eq!(value, before);
    }

    #[test]
    fn recorded_empty_codex_ownership_preserves_matching_value() {
        let mut value: toml::Value = r#"
            [mcp_servers.icm.tools.icm_memory_recall]
            approval_mode = "approve"
        "#
        .parse()
        .unwrap();
        let before = value.clone();
        let spec = toml_spec(ProviderId::Codex);
        assert!(discover_toml_trust(&value, spec, Some(&[])).is_empty());
        assert_eq!(strip_toml_trust(&mut value, spec, Some(&[])), 0);
        assert_eq!(value, before);
    }
}
