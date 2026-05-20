//! Pure string/value strippers — every function takes content in,
//! returns content out, never touches the filesystem. The mutator
//! (`super::mutate`) wraps these to stage backups and write the result.
//!
//! Symmetric design with the `inject_*` helpers in `main.rs`: each
//! stripper undoes exactly what its counterpart wrote, and the public
//! return type is a `StripResult` so callers can distinguish "nothing to
//! do", "removed N entries", and "ambiguous — manual review needed".

use anyhow::{Context, Result};
use serde_json::Value;

use super::locations::HookCommandField;

/// Outcome of a stripper run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StripResult {
    /// File was already clean — no change required, no rewrite needed.
    NoOp,
    /// `removed` entries were stripped. Caller should persist `new_content`.
    Removed { removed: usize },
    /// The whole file should be deleted (e.g. markdown block was the only
    /// content). Caller deletes the file rather than writing back.
    DeleteFile,
    /// Structurally ambiguous; caller logs a warning and skips. Contributes
    /// to exit code 3 in the orchestrator.
    Ambiguous { reason: String },
}

// =========================================================================
// JSON: mcpServers / servers / mcp / context_servers
// =========================================================================

/// Remove the `icm` entry from a dotted `servers_key` path (e.g.
/// `"mcpServers"`, `"amp.mcpServers"`). Empty parent objects are cleaned
/// up cascading upward.
pub(crate) fn strip_json_mcp_server(value: &mut Value, dotted_key: &str) -> StripResult {
    let segments: Vec<&str> = dotted_key.split('.').collect();
    if segments.is_empty() {
        return StripResult::NoOp;
    }
    let removed = remove_path_then_clean(value, &segments, "icm");
    if removed {
        StripResult::Removed { removed: 1 }
    } else {
        StripResult::NoOp
    }
}

/// Walks `value` through `segments`, removes `leaf_key` at the deepest
/// object, then cleans up emptied parent objects on the way back. Returns
/// whether anything was removed.
fn remove_path_then_clean(value: &mut Value, segments: &[&str], leaf_key: &str) -> bool {
    if segments.is_empty() {
        if let Some(obj) = value.as_object_mut() {
            return obj.remove(leaf_key).is_some();
        }
        return false;
    }
    let head = segments[0];
    let tail = &segments[1..];
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let Some(child) = obj.get_mut(head) else {
        return false;
    };
    let removed = remove_path_then_clean(child, tail, leaf_key);
    // After recursion, drop the parent if its object became empty.
    if removed {
        let drop_parent = obj
            .get(head)
            .and_then(|v| v.as_object())
            .map(|o| o.is_empty())
            .unwrap_or(false);
        if drop_parent {
            obj.remove(head);
        }
    }
    removed
}

// =========================================================================
// JSON: hook arrays (Claude/Gemini/Codex shape and Copilot top-level shape)
// =========================================================================

/// Strip every ICM-bearing hook entry from `value["hooks"]`, cascading the
/// clean-up: empty `hooks[]` arrays drop their wrapper entry, empty event
/// arrays drop the event key, and an empty `hooks` object drops the top-
/// level key. The root JSON object is never deleted.
pub(crate) fn strip_json_hooks(value: &mut Value, field: HookCommandField) -> StripResult {
    let Some(root) = value.as_object_mut() else {
        return StripResult::NoOp;
    };
    let Some(hooks_value) = root.get_mut("hooks") else {
        return StripResult::NoOp;
    };
    let Some(hooks_obj) = hooks_value.as_object_mut() else {
        return StripResult::NoOp;
    };

    let mut removed = 0usize;
    let event_names: Vec<String> = hooks_obj.keys().cloned().collect();

    for event in event_names {
        let Some(event_arr) = hooks_obj.get_mut(&event).and_then(|v| v.as_array_mut()) else {
            continue;
        };

        match field {
            HookCommandField::Command => {
                // Mutate each entry's `hooks[]` in place; collect indices
                // to drop after the inner pass.
                let mut entry_drop = Vec::new();
                for (i, entry) in event_arr.iter_mut().enumerate() {
                    let Some(inner) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
                        continue;
                    };
                    let before = inner.len();
                    inner.retain(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|s| !crate::cmd_matches_icm_pattern(s, "icm hook"))
                            .unwrap_or(true)
                    });
                    removed += before - inner.len();
                    if inner.is_empty() {
                        entry_drop.push(i);
                    }
                }
                // Drop entries whose `hooks[]` became empty. Reverse-iter
                // so indices stay valid.
                for i in entry_drop.into_iter().rev() {
                    event_arr.remove(i);
                }
            }
            HookCommandField::BashTopLevel => {
                let before = event_arr.len();
                event_arr.retain(|entry| {
                    entry
                        .get("bash")
                        .and_then(|b| b.as_str())
                        .map(|s| !crate::cmd_matches_icm_pattern(s, "icm hook"))
                        .unwrap_or(true)
                });
                removed += before - event_arr.len();
            }
        }
    }

    // Cascade clean: drop now-empty event arrays.
    let empty_events: Vec<String> = hooks_obj
        .iter()
        .filter_map(|(k, v)| v.as_array().filter(|a| a.is_empty()).map(|_| k.clone()))
        .collect();
    for k in empty_events {
        hooks_obj.remove(&k);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }

    if removed == 0 {
        StripResult::NoOp
    } else {
        StripResult::Removed { removed }
    }
}

// =========================================================================
// TOML: [<table>.<entry>]
// =========================================================================

/// Remove `[<table>.<entry>]` from a TOML document. Cleans up an empty
/// parent table. Note: this round-trips through `toml`, which loses
/// comments — symmetric with `inject_codex_mcp_server` in `main.rs`.
pub(crate) fn strip_toml_table(value: &mut toml::Value, table: &str, entry: &str) -> StripResult {
    let Some(root) = value.as_table_mut() else {
        return StripResult::NoOp;
    };
    let Some(parent) = root.get_mut(table).and_then(|v| v.as_table_mut()) else {
        return StripResult::NoOp;
    };
    if parent.remove(entry).is_some() {
        if parent.is_empty() {
            root.remove(table);
        }
        StripResult::Removed { removed: 1 }
    } else {
        StripResult::NoOp
    }
}

// =========================================================================
// YAML: Continue.dev `- name: icm` block
// =========================================================================

/// Strip a single `- name: icm` block from a YAML document, with strict
/// structural validation. Returns `Ambiguous` if the block doesn't carry
/// both a `command:` and an `args:` continuation line — in that case the
/// user has hand-edited the layout and we refuse to guess.
pub(crate) fn strip_yaml_continue(content: &str) -> StripResult {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| {
        let t = l.trim_start();
        t.starts_with("- name: icm") || t.starts_with("- name: \"icm\"")
    }) else {
        return StripResult::NoOp;
    };
    let start_indent = lines[start].len() - lines[start].trim_start().len();

    // The block is the starting `- ` line plus every continuation line
    // indented strictly more than `start_indent`. A blank line inside the
    // block counts as a continuation only if the next non-blank line is
    // still more-indented than `start_indent`.
    let mut end = start + 1;
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() {
            // Peek ahead: blanks belong to the block only if continuation
            // follows.
            let mut peek = end + 1;
            while peek < lines.len() && lines[peek].trim().is_empty() {
                peek += 1;
            }
            if peek < lines.len() {
                let indent = lines[peek].len() - lines[peek].trim_start().len();
                if indent > start_indent {
                    end = peek + 1;
                    continue;
                }
            }
            break;
        }
        let indent = line.len() - line.trim_start().len();
        if indent > start_indent {
            end += 1;
        } else {
            break;
        }
    }

    // Structural check: the block must contain both `command:` and `args:`
    // (Continue.dev's canonical shape, which is what init writes).
    let block = &lines[start..end];
    let mut has_command = false;
    let mut has_args = false;
    for line in block {
        let t = line.trim_start();
        if t.starts_with("command:") {
            has_command = true;
        } else if t.starts_with("args:") {
            has_args = true;
        }
    }
    if !(has_command && has_args) {
        return StripResult::Ambiguous {
            reason: format!(
                "YAML block at line {} for `- name: icm` is missing the canonical `command:`/`args:` lines — refusing to guess; review the file by hand.",
                start + 1
            ),
        };
    }

    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            if i >= start && i < end {
                None
            } else {
                Some(*l)
            }
        })
        .collect();
    let new_content = if content.ends_with('\n') {
        format!("{}\n", kept.join("\n"))
    } else {
        kept.join("\n")
    };
    if new_content == content {
        StripResult::NoOp
    } else {
        // Caller persists `new_content` via `apply_yaml_continue`.
        // We can't bundle the string in `StripResult::Removed` without
        // bloating the enum, so the apply function recomputes once. Keep
        // both call sites in sync via the helper below.
        StripResult::Removed { removed: 1 }
    }
}

/// Same logic as [`strip_yaml_continue`] but returns the rewritten
/// content. Used by the mutator after `strip_yaml_continue` reports a
/// removal. Returning two values from the same scan would simplify the
/// API; that refactor can wait.
pub(crate) fn apply_yaml_continue(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| {
        let t = l.trim_start();
        t.starts_with("- name: icm") || t.starts_with("- name: \"icm\"")
    }) else {
        return content.to_string();
    };
    let start_indent = lines[start].len() - lines[start].trim_start().len();
    let mut end = start + 1;
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() {
            let mut peek = end + 1;
            while peek < lines.len() && lines[peek].trim().is_empty() {
                peek += 1;
            }
            if peek < lines.len() {
                let indent = lines[peek].len() - lines[peek].trim_start().len();
                if indent > start_indent {
                    end = peek + 1;
                    continue;
                }
            }
            break;
        }
        let indent = line.len() - line.trim_start().len();
        if indent > start_indent {
            end += 1;
        } else {
            break;
        }
    }
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            if i >= start && i < end {
                None
            } else {
                Some(*l)
            }
        })
        .collect();
    if content.ends_with('\n') {
        format!("{}\n", kept.join("\n"))
    } else {
        kept.join("\n")
    }
}

// =========================================================================
// Markdown: <!-- icm:start --> ... <!-- icm:end -->
// =========================================================================

/// Result of `strip_markdown_block`: either a rewritten string, the
/// signal to delete the file (block was the only content), or a no-op.
pub(crate) enum MarkdownOutcome {
    NoOp,
    Rewrite(String),
    DeleteFile,
}

pub(crate) fn strip_markdown_block(content: &str) -> MarkdownOutcome {
    const START: &str = "<!-- icm:start -->";
    const END: &str = "<!-- icm:end -->";
    let Some(start) = content.find(START) else {
        return MarkdownOutcome::NoOp;
    };
    let end = content
        .find(END)
        .map(|o| o + END.len())
        .unwrap_or(content.len());
    let before = &content[..start];
    let after = if end <= content.len() {
        &content[end..]
    } else {
        ""
    };
    if before.trim().is_empty() && after.trim().is_empty() {
        return MarkdownOutcome::DeleteFile;
    }
    // Collapse the blank line that often separates the block from its
    // surroundings: trim trailing whitespace from `before` and leading
    // whitespace from `after`, then rejoin with one newline.
    let trimmed_before = before.trim_end();
    let trimmed_after = after.trim_start();
    let mut out = String::with_capacity(trimmed_before.len() + trimmed_after.len() + 2);
    out.push_str(trimmed_before);
    if !trimmed_before.is_empty() && !trimmed_after.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(trimmed_after);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    MarkdownOutcome::Rewrite(out)
}

// =========================================================================
// Public file-level helpers used by the mutator
// =========================================================================

/// Read JSON, mutate, write back. Returns the strip outcome. The caller
/// is responsible for backing up `path` before invoking this.
pub(crate) fn rewrite_json_mcp(path: &std::path::Path, dotted_key: &str) -> Result<StripResult> {
    let mut value = crate::parse_json_config(path)?;
    let result = strip_json_mcp_server(&mut value, dotted_key);
    if matches!(result, StripResult::Removed { .. }) {
        let out = serde_json::to_string_pretty(&value)?;
        std::fs::write(path, out).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(result)
}

pub(crate) fn rewrite_json_hooks(
    path: &std::path::Path,
    field: HookCommandField,
) -> Result<StripResult> {
    let mut value = crate::parse_json_config(path)?;
    let result = strip_json_hooks(&mut value, field);
    if matches!(result, StripResult::Removed { .. }) {
        let out = serde_json::to_string_pretty(&value)?;
        std::fs::write(path, out).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(result)
}

pub(crate) fn rewrite_toml(
    path: &std::path::Path,
    table: &str,
    entry: &str,
) -> Result<StripResult> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut value: toml::Value = content.parse()?;
    let result = strip_toml_table(&mut value, table, entry);
    if matches!(result, StripResult::Removed { .. }) {
        let out = toml::to_string(&value)?;
        std::fs::write(path, out).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(result)
}

pub(crate) fn rewrite_yaml_continue(path: &std::path::Path) -> Result<StripResult> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let result = strip_yaml_continue(&content);
    if matches!(result, StripResult::Removed { .. }) {
        let new_content = apply_yaml_continue(&content);
        std::fs::write(path, new_content)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(result)
}

pub(crate) fn rewrite_markdown(path: &std::path::Path) -> Result<StripResult> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    match strip_markdown_block(&content) {
        MarkdownOutcome::NoOp => Ok(StripResult::NoOp),
        MarkdownOutcome::Rewrite(new) => {
            std::fs::write(path, new)
                .with_context(|| format!("cannot write {}", path.display()))?;
            Ok(StripResult::Removed { removed: 1 })
        }
        MarkdownOutcome::DeleteFile => {
            std::fs::remove_file(path)
                .with_context(|| format!("cannot delete {}", path.display()))?;
            Ok(StripResult::DeleteFile)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_mcp_server_removes_icm_and_keeps_siblings() {
        let mut v = json!({
            "mcpServers": {
                "icm": {"command": "/x/icm"},
                "other": {"command": "/x/o"}
            }
        });
        let r = strip_json_mcp_server(&mut v, "mcpServers");
        assert_eq!(r, StripResult::Removed { removed: 1 });
        assert!(v["mcpServers"].get("icm").is_none());
        assert!(v["mcpServers"].get("other").is_some());
    }

    #[test]
    fn strip_mcp_server_drops_emptied_parent() {
        let mut v = json!({ "mcpServers": { "icm": {} } });
        let r = strip_json_mcp_server(&mut v, "mcpServers");
        assert_eq!(r, StripResult::Removed { removed: 1 });
        // Parent dropped because it became empty.
        assert!(v.get("mcpServers").is_none());
    }

    #[test]
    fn strip_mcp_server_dotted_path() {
        let mut v = json!({ "amp": { "mcpServers": { "icm": {}, "k": {} } } });
        let r = strip_json_mcp_server(&mut v, "amp.mcpServers");
        assert_eq!(r, StripResult::Removed { removed: 1 });
        assert!(v["amp"]["mcpServers"].get("icm").is_none());
        assert!(v["amp"]["mcpServers"].get("k").is_some());
    }

    #[test]
    fn strip_mcp_server_noop_when_absent() {
        let mut v = json!({ "mcpServers": { "other": {} } });
        let r = strip_json_mcp_server(&mut v, "mcpServers");
        assert_eq!(r, StripResult::NoOp);
    }

    #[test]
    fn strip_hooks_command_field_removes_icm_keeps_siblings_and_cascades() {
        let mut v = json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher":"Bash","hooks":[
                        {"type":"command","command":"/x/icm hook pre"},
                        {"type":"command","command":"/x/other"}
                    ]}
                ],
                "PostToolUse": [
                    {"hooks":[{"type":"command","command":"/x/icm hook post"}]}
                ]
            }
        });
        let r = strip_json_hooks(&mut v, HookCommandField::Command);
        assert_eq!(r, StripResult::Removed { removed: 2 });
        // Sibling hook preserved.
        let pre_entry = &v["hooks"]["PreToolUse"][0];
        assert_eq!(pre_entry["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(
            pre_entry["hooks"][0]["command"].as_str().unwrap(),
            "/x/other"
        );
        // PostToolUse became empty -> event key dropped.
        assert!(v["hooks"].get("PostToolUse").is_none());
    }

    #[test]
    fn strip_hooks_drops_hooks_object_when_completely_empty() {
        let mut v = json!({
            "permissions": ["read"],
            "hooks": {
                "PreToolUse": [
                    {"hooks":[{"type":"command","command":"/x/icm hook pre"}]}
                ]
            }
        });
        let r = strip_json_hooks(&mut v, HookCommandField::Command);
        assert_eq!(r, StripResult::Removed { removed: 1 });
        // hooks object dropped entirely; permissions untouched.
        assert!(v.get("hooks").is_none());
        assert_eq!(v["permissions"][0].as_str().unwrap(), "read");
    }

    #[test]
    fn strip_hooks_bash_top_level_for_copilot() {
        let mut v = json!({
            "hooks": {
                "sessionStart": [
                    {"type":"command","bash":"/x/icm hook start","timeoutSec":10},
                    {"type":"command","bash":"/x/other","timeoutSec":5}
                ]
            }
        });
        let r = strip_json_hooks(&mut v, HookCommandField::BashTopLevel);
        assert_eq!(r, StripResult::Removed { removed: 1 });
        let arr = v["hooks"]["sessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["bash"].as_str().unwrap(), "/x/other");
    }

    #[test]
    fn strip_toml_table_removes_and_cascades() {
        let src = r#"
[some.other]
hello = "world"

[mcp_servers.icm]
command = "/x/icm"
args = ["serve"]
"#;
        let mut v: toml::Value = src.parse().unwrap();
        let r = strip_toml_table(&mut v, "mcp_servers", "icm");
        assert_eq!(r, StripResult::Removed { removed: 1 });
        // mcp_servers parent had only icm -> dropped.
        assert!(v.get("mcp_servers").is_none());
        // Sibling table preserved.
        assert!(v["some"]["other"]["hello"].is_str());
    }

    #[test]
    fn strip_toml_table_keeps_parent_with_siblings() {
        let src = r#"
[mcp_servers.icm]
command = "/x/icm"

[mcp_servers.other]
command = "/x/o"
"#;
        let mut v: toml::Value = src.parse().unwrap();
        let r = strip_toml_table(&mut v, "mcp_servers", "icm");
        assert_eq!(r, StripResult::Removed { removed: 1 });
        assert!(v["mcp_servers"].get("other").is_some());
        assert!(v["mcp_servers"].get("icm").is_none());
    }

    #[test]
    fn strip_yaml_continue_removes_canonical_block() {
        let src = "\
mcpServers:
  - name: icm
    command: /x/icm
    args:
      - serve
  - name: other
    command: /x/other
    args:
      - run
";
        let r = strip_yaml_continue(src);
        assert_eq!(r, StripResult::Removed { removed: 1 });
        let new = apply_yaml_continue(src);
        assert!(!new.contains("- name: icm"));
        assert!(new.contains("- name: other"));
    }

    #[test]
    fn strip_yaml_continue_flags_ambiguous_block_missing_command_or_args() {
        let src = "\
mcpServers:
  - name: icm
    foo: bar
";
        let r = strip_yaml_continue(src);
        assert!(
            matches!(r, StripResult::Ambiguous { .. }),
            "expected Ambiguous, got {r:?}"
        );
    }

    #[test]
    fn strip_yaml_continue_noop_when_absent() {
        let src = "mcpServers:\n  - name: other\n    command: /x/o\n";
        assert_eq!(strip_yaml_continue(src), StripResult::NoOp);
    }

    #[test]
    fn strip_markdown_block_preserves_surrounding_content() {
        let src = "intro\n\n<!-- icm:start -->\nblock\n<!-- icm:end -->\n\noutro\n";
        match strip_markdown_block(src) {
            MarkdownOutcome::Rewrite(new) => {
                assert!(!new.contains("icm:start"));
                assert!(new.contains("intro"));
                assert!(new.contains("outro"));
            }
            other => panic!("expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn strip_markdown_block_deletes_file_when_block_is_only_content() {
        let src = "<!-- icm:start -->\nfoo\n<!-- icm:end -->\n";
        assert!(matches!(
            strip_markdown_block(src),
            MarkdownOutcome::DeleteFile
        ));
    }

    #[test]
    fn strip_markdown_block_noop_when_no_block() {
        let src = "# Title\n\nbody\n";
        assert!(matches!(strip_markdown_block(src), MarkdownOutcome::NoOp));
    }

    impl std::fmt::Debug for MarkdownOutcome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                MarkdownOutcome::NoOp => write!(f, "NoOp"),
                MarkdownOutcome::Rewrite(_) => write!(f, "Rewrite(..)"),
                MarkdownOutcome::DeleteFile => write!(f, "DeleteFile"),
            }
        }
    }
}
