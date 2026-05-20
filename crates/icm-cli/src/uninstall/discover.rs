//! Read-only scanner that walks the [`super::locations`] catalog and
//! reports every place ICM residue is detected. Mutation lives in
//! `super::formats` (next commit).
//!
//! Discovery is JSON/TOML/YAML/Markdown-aware: it does not just
//! grep the file. We parse where parsing is cheap so a hit comes with
//! enough detail (which key? which line? how many bytes?) for the audit
//! / dry-run output to be actionable.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::locations::{HookCommandField, LocationKind, LocationSpec};

/// Per-hit detail. Drives the report formatter and, later, the mutator.
#[derive(Clone, Debug)]
pub(crate) enum HitDetail {
    /// JSON: `<servers_key>.icm` was found.
    JsonServer { pointer: String },
    /// JSON: at least one hook command matches `icm hook`.
    JsonHook { event: String, command: String },
    /// TOML: `<table>.<entry>` table was found.
    TomlTable { table: String },
    /// YAML: a candidate `- name: icm` block was found.
    YamlBlock { start_line: usize, lines: usize },
    /// Markdown: `<!-- icm:start -->` block was found.
    MarkdownBlock {
        start_line: usize,
        end_line: usize,
        file_will_be_empty: bool,
    },
    /// Owned file present on disk.
    OwnedFile { bytes: u64 },
    /// Data directory present on disk; reports approximate size.
    DataDir { bytes_total: u64, files: usize },
}

/// One hit. A single `LocationSpec` can yield multiple hits (e.g. a hooks
/// settings file has one entry per event).
#[derive(Clone, Debug)]
pub(crate) struct LocationHit {
    pub spec_label: &'static str,
    pub path: PathBuf,
    pub detail: HitDetail,
}

/// Aggregate result of a discovery pass.
#[derive(Default, Debug)]
pub(crate) struct RemovalPlan {
    pub hits: Vec<LocationHit>,
    /// Reserved for the scan-dir walker (a later commit on this PR).
    pub scan_dir_hits: Vec<LocationHit>,
    /// Reserved for the `icm serve` process detector (a later commit).
    pub processes: Vec<RunningProcess>,
}

impl RemovalPlan {
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty() && self.scan_dir_hits.is_empty()
    }

    /// Number of hits across both the catalog and the project tree scan.
    pub fn total_hits(&self) -> usize {
        self.hits.len() + self.scan_dir_hits.len()
    }
}

/// Placeholder until the process detector lands. Carries enough info to
/// print a warning in the report.
#[derive(Clone, Debug)]
pub(crate) struct RunningProcess {
    pub pid: u32,
    pub cmdline: String,
}

/// Walk every spec and return the hits. `include_data` controls whether
/// `DataDir` entries are inspected (only `--purge-data` opts in).
pub(crate) fn scan(specs: &[LocationSpec], include_data: bool) -> Result<RemovalPlan> {
    let mut plan = RemovalPlan::default();
    for spec in specs {
        if spec.purge_data_only && !include_data {
            continue;
        }
        match scan_spec(spec) {
            Ok(mut hits) => plan.hits.append(&mut hits),
            Err(_e) => {
                // Read or parse errors are non-fatal for discovery; we
                // simply skip the spec. A later commit will surface
                // these as warnings.
            }
        }
    }
    Ok(plan)
}

/// Dispatch on the spec's kind. Returns zero or more hits.
fn scan_spec(spec: &LocationSpec) -> Result<Vec<LocationHit>> {
    if !spec.path.exists() {
        return Ok(Vec::new());
    }
    match &spec.kind {
        LocationKind::JsonConfig {
            servers_key,
            has_hooks,
            hooks_field,
        } => scan_json(spec, servers_key.as_deref(), *has_hooks, *hooks_field),
        LocationKind::TomlMcp { table, entry } => scan_toml(spec, table, entry),
        LocationKind::YamlContinue => scan_yaml_continue(spec),
        LocationKind::MarkdownBlock => scan_markdown(spec),
        LocationKind::OwnedFile => Ok(vec![LocationHit {
            spec_label: spec.label,
            path: spec.path.clone(),
            detail: HitDetail::OwnedFile {
                bytes: std::fs::metadata(&spec.path).map(|m| m.len()).unwrap_or(0),
            },
        }]),
        LocationKind::DataDir => scan_data_dir(spec),
    }
}

fn scan_json(
    spec: &LocationSpec,
    servers_key: Option<&str>,
    has_hooks: bool,
    hooks_field: HookCommandField,
) -> Result<Vec<LocationHit>> {
    let value = crate::parse_json_config(&spec.path)?;
    let mut hits = Vec::new();

    if let Some(key) = servers_key {
        if let Some(servers) = lookup_dotted(&value, key) {
            if servers.get("icm").is_some() {
                hits.push(LocationHit {
                    spec_label: spec.label,
                    path: spec.path.clone(),
                    detail: HitDetail::JsonServer {
                        pointer: format!("/{}/icm", key.replace('.', "/")),
                    },
                });
            }
        }
    }

    if has_hooks {
        if let Some(hooks_obj) = value.get("hooks").and_then(|h| h.as_object()) {
            for (event, arr) in hooks_obj {
                let Some(arr) = arr.as_array() else { continue };
                for entry in arr {
                    match hooks_field {
                        HookCommandField::Command => {
                            let Some(inner) = entry.get("hooks").and_then(|h| h.as_array()) else {
                                continue;
                            };
                            for h in inner {
                                if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                                    if crate::cmd_matches_icm_pattern(cmd, "icm hook") {
                                        hits.push(LocationHit {
                                            spec_label: spec.label,
                                            path: spec.path.clone(),
                                            detail: HitDetail::JsonHook {
                                                event: event.clone(),
                                                command: cmd.to_string(),
                                            },
                                        });
                                    }
                                }
                            }
                        }
                        HookCommandField::BashTopLevel => {
                            if let Some(cmd) = entry.get("bash").and_then(|b| b.as_str()) {
                                if crate::cmd_matches_icm_pattern(cmd, "icm hook") {
                                    hits.push(LocationHit {
                                        spec_label: spec.label,
                                        path: spec.path.clone(),
                                        detail: HitDetail::JsonHook {
                                            event: event.clone(),
                                            command: cmd.to_string(),
                                        },
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(hits)
}

/// Walk a dotted key like `"amp.mcpServers"` through nested JSON objects.
fn lookup_dotted<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut cur = value;
    for segment in dotted.split('.') {
        cur = cur.get(segment)?;
    }
    Some(cur)
}

fn scan_toml(spec: &LocationSpec, table: &str, entry: &str) -> Result<Vec<LocationHit>> {
    let content = std::fs::read_to_string(&spec.path)?;
    let parsed: toml::Value = content.parse()?;
    let mut hits = Vec::new();
    if let Some(t) = parsed.get(table).and_then(|v| v.as_table()) {
        if t.contains_key(entry) {
            hits.push(LocationHit {
                spec_label: spec.label,
                path: spec.path.clone(),
                detail: HitDetail::TomlTable {
                    table: format!("[{table}.{entry}]"),
                },
            });
        }
    }
    Ok(hits)
}

/// Continue.dev YAML detection: look for a top-level `- name: icm` entry.
///
/// We intentionally do not parse the YAML. The mutator (next commit)
/// applies a regex strip with strict structural validation; we report a
/// hit here as soon as the canonical opening line is present so the user
/// sees it in the audit.
fn scan_yaml_continue(spec: &LocationSpec) -> Result<Vec<LocationHit>> {
    let content = std::fs::read_to_string(&spec.path)?;
    let mut hits = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- name: icm") || trimmed.starts_with("- name: \"icm\"") {
            let block_lines = count_yaml_block_lines(&content, idx);
            hits.push(LocationHit {
                spec_label: spec.label,
                path: spec.path.clone(),
                detail: HitDetail::YamlBlock {
                    start_line: idx + 1, // 1-based for human reports
                    lines: block_lines,
                },
            });
            // Only the first hit; multiple `- name: icm` in one file is
            // pathological and the mutator handles it line-by-line.
            break;
        }
    }
    Ok(hits)
}

/// Estimate the block length: the starting `- name: icm` line plus each
/// continuation line indented further than the `- ` marker. Approximation
/// only; the mutator does the precise strip.
fn count_yaml_block_lines(content: &str, start: usize) -> usize {
    let lines: Vec<&str> = content.lines().collect();
    let mut n = 1;
    let start_indent = lines[start].len() - lines[start].trim_start().len();
    for line in &lines[start + 1..] {
        let indent = line.len() - line.trim_start().len();
        if line.trim().is_empty() || indent > start_indent {
            n += 1;
        } else {
            break;
        }
    }
    n
}

fn scan_markdown(spec: &LocationSpec) -> Result<Vec<LocationHit>> {
    let content = std::fs::read_to_string(&spec.path)?;
    let Some(start_off) = content.find("<!-- icm:start -->") else {
        return Ok(Vec::new());
    };
    let end_off = content
        .find("<!-- icm:end -->")
        .map(|o| o + "<!-- icm:end -->".len())
        .unwrap_or(content.len());

    let before = &content[..start_off];
    let after = if end_off <= content.len() {
        &content[end_off..]
    } else {
        ""
    };
    let file_will_be_empty = before.trim().is_empty() && after.trim().is_empty();

    let start_line = byte_offset_to_line(&content, start_off);
    let end_line = byte_offset_to_line(&content, end_off.min(content.len()));

    Ok(vec![LocationHit {
        spec_label: spec.label,
        path: spec.path.clone(),
        detail: HitDetail::MarkdownBlock {
            start_line,
            end_line,
            file_will_be_empty,
        },
    }])
}

fn byte_offset_to_line(s: &str, offset: usize) -> usize {
    let clamped = offset.min(s.len());
    s[..clamped].bytes().filter(|&b| b == b'\n').count() + 1
}

fn scan_data_dir(spec: &LocationSpec) -> Result<Vec<LocationHit>> {
    let mut bytes_total = 0u64;
    let mut files = 0usize;
    walk_size(&spec.path, &mut bytes_total, &mut files)?;
    if files == 0 && bytes_total == 0 {
        // Directory exists but is empty — still report it so the user
        // sees the path that would be cleaned up.
    }
    Ok(vec![LocationHit {
        spec_label: spec.label,
        path: spec.path.clone(),
        detail: HitDetail::DataDir { bytes_total, files },
    }])
}

fn walk_size(path: &Path, bytes: &mut u64, files: &mut usize) -> Result<()> {
    let meta = std::fs::metadata(path)?;
    if meta.is_file() {
        *bytes += meta.len();
        *files += 1;
        return Ok(());
    }
    if !meta.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        // Resist symlink traversal — `read_dir` itself doesn't follow,
        // but the child metadata call might if we use `metadata`. Use
        // `symlink_metadata` to be explicit.
        let m = std::fs::symlink_metadata(&p)?;
        if m.file_type().is_symlink() {
            continue;
        }
        if m.is_file() {
            *bytes += m.len();
            *files += 1;
        } else if m.is_dir() {
            walk_size(&p, bytes, files)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uninstall::locations::{build_locations, dir_context_under};
    use std::fs;
    use std::io::Write;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn scan_empty_fake_home_yields_zero_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        // ProjectDirs may resolve outside the tempdir; only assert that
        // every hit lives under tempdir or is a data dir.
        for h in &plan.hits {
            let p = h.path.to_string_lossy();
            let ok = p.starts_with(&*tmp.path().to_string_lossy())
                || matches!(h.detail, HitDetail::DataDir { .. });
            assert!(ok, "stray hit outside tempdir: {}", h.path.display());
        }
    }

    #[test]
    fn scan_detects_mcp_server_in_synthetic_claude_json() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let claude = dirs.claude_legacy_json();
        write(
            &claude,
            r#"{
  "mcpServers": {
    "icm": { "command": "/x/icm", "args": ["serve"] },
    "other": { "command": "/x/other" }
  }
}"#,
        );
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        let mcp_hits: Vec<_> = plan
            .hits
            .iter()
            .filter(|h| matches!(h.detail, HitDetail::JsonServer { .. }))
            .collect();
        assert_eq!(mcp_hits.len(), 1, "expected one MCP hit: {plan:#?}");
        assert_eq!(mcp_hits[0].spec_label, "Claude Code MCP");
    }

    #[test]
    fn scan_detects_hook_in_synthetic_claude_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let settings = dirs.claude_dir.join("settings.json");
        write(
            &settings,
            r#"{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{"type":"command","command":"/x/icm hook pre"}] }
    ],
    "Unrelated": [
      { "hooks": [{"type":"command","command":"/x/other --do"}] }
    ]
  }
}"#,
        );
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        let hook_hits: Vec<_> = plan
            .hits
            .iter()
            .filter(|h| matches!(h.detail, HitDetail::JsonHook { .. }))
            .collect();
        assert_eq!(hook_hits.len(), 1, "{plan:#?}");
        match &hook_hits[0].detail {
            HitDetail::JsonHook { event, command } => {
                assert_eq!(event, "PreToolUse");
                assert!(command.contains("icm hook"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn scan_detects_copilot_bash_top_level_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let settings = dirs.copilot_dir.join("settings.json");
        write(
            &settings,
            r#"{
  "hooks": {
    "sessionStart": [
      { "type": "command", "bash": "/x/icm hook start", "timeoutSec": 10 }
    ]
  }
}"#,
        );
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        let hits: Vec<_> = plan
            .hits
            .iter()
            .filter(|h| matches!(h.detail, HitDetail::JsonHook { .. }))
            .collect();
        assert_eq!(hits.len(), 1, "{plan:#?}");
        assert_eq!(hits[0].spec_label, "Copilot CLI hooks");
    }

    #[test]
    fn scan_detects_codex_toml_table() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let toml_path = dirs.codex_dir.join("config.toml");
        write(
            &toml_path,
            r#"
[some.other.section]
hello = "world"

[mcp_servers.icm]
command = "/x/icm"
args = ["serve"]
"#,
        );
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        let hits: Vec<_> = plan
            .hits
            .iter()
            .filter(|h| matches!(h.detail, HitDetail::TomlTable { .. }))
            .collect();
        assert_eq!(hits.len(), 1, "{plan:#?}");
        match &hits[0].detail {
            HitDetail::TomlTable { table } => assert_eq!(table, "[mcp_servers.icm]"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn scan_detects_markdown_block_with_correct_line_range() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let claude_md = dirs.cwd.join("CLAUDE.md");
        let body = "intro\n\n<!-- icm:start -->\nblock\nlines\n<!-- icm:end -->\n\noutro\n";
        write(&claude_md, body);
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        let hits: Vec<_> = plan
            .hits
            .iter()
            .filter(|h| matches!(h.detail, HitDetail::MarkdownBlock { .. }))
            .collect();
        assert_eq!(hits.len(), 1, "{plan:#?}");
        match &hits[0].detail {
            HitDetail::MarkdownBlock {
                start_line,
                end_line,
                file_will_be_empty,
            } => {
                assert_eq!(*start_line, 3);
                assert!(*end_line >= 6);
                assert!(!file_will_be_empty);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn scan_owned_file_reports_size() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let recall = dirs.claude_dir.join("commands/recall.md");
        write(&recall, "Search ICM memory for: $ARGUMENTS");
        let specs = build_locations(&dirs);
        let plan = scan(&specs, false).unwrap();
        let hits: Vec<_> = plan
            .hits
            .iter()
            .filter(|h| matches!(h.detail, HitDetail::OwnedFile { .. }))
            .collect();
        assert!(
            hits.iter().any(|h| h.spec_label == "Claude Code /recall"),
            "missing /recall hit: {plan:#?}"
        );
    }

    #[test]
    fn scan_skips_data_dirs_unless_purge_data_set() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dir_context_under(tmp.path());
        let specs = build_locations(&dirs);
        let plan_default = scan(&specs, false).unwrap();
        assert!(
            !plan_default
                .hits
                .iter()
                .any(|h| matches!(h.detail, HitDetail::DataDir { .. })),
            "data dir reported without --purge-data"
        );
    }
}
