//! Apply phase: walks the discovered hits, stages each file in the backup
//! session, and dispatches to the right `formats::rewrite_*` helper.
//!
//! Side effects are intentionally isolated here so the strippers in
//! `formats.rs` stay pure (string in, string out).

use anyhow::Result;

use super::backup::BackupSession;
use super::discover::{HitDetail, LocationHit, RemovalPlan};
use super::formats::{
    rewrite_json_hooks, rewrite_json_mcp, rewrite_markdown, rewrite_toml, rewrite_yaml_continue,
    StripResult,
};
use super::locations::{HookCommandField, LocationKind, LocationSpec};

/// Per-file outcome surfaced to the report.
#[derive(Debug)]
pub(crate) struct ApplyOutcome {
    pub path: std::path::PathBuf,
    pub label: &'static str,
    pub result: Result<StripResult>,
}

/// Apply every non-data hit in `plan`. Each touched path is staged in
/// the backup session once (idempotent) before its mutator runs.
pub(crate) fn apply(
    plan: &RemovalPlan,
    specs: &[LocationSpec],
    backup: &mut Option<BackupSession>,
) -> Vec<ApplyOutcome> {
    // Group hits by path so a single multi-event hooks file is only
    // staged + mutated once.
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for h in plan.hits.iter().chain(plan.scan_dir_hits.iter()) {
        if matches!(h.detail, HitDetail::DataDir { .. }) {
            continue;
        }
        if !paths.contains(&h.path) {
            paths.push(h.path.clone());
        }
    }

    let mut outcomes = Vec::with_capacity(paths.len());
    for path in paths {
        let Some(hit) = plan
            .hits
            .iter()
            .chain(plan.scan_dir_hits.iter())
            .find(|h| h.path == path)
        else {
            continue;
        };
        let kind = specs
            .iter()
            .find(|s| s.path == hit.path)
            .map(|s| s.kind.clone())
            .unwrap_or(LocationKind::MarkdownBlock);

        let result: Result<StripResult> = (|| {
            // Refuse to mutate paths that are symlinks: the backup
            // session deliberately doesn't dereference them, and writing
            // through a symlink would silently mutate a shared target
            // (e.g. a dotfiles repo) without any safety net.
            if let Ok(m) = std::fs::symlink_metadata(&hit.path) {
                if m.file_type().is_symlink() {
                    return Ok(StripResult::Ambiguous {
                        reason: format!(
                            "{} is a symlink — refusing to mutate the target without an explicit backup; resolve manually.",
                            hit.path.display()
                        ),
                    });
                }
            }
            if let Some(b) = backup.as_mut() {
                b.stage(&hit.path)?;
            }
            match kind {
                LocationKind::JsonConfig {
                    servers_key,
                    has_hooks,
                    hooks_field,
                } => apply_json(&hit.path, servers_key, has_hooks, hooks_field, hit),
                LocationKind::TomlMcp { table, entry } => rewrite_toml(&hit.path, table, entry),
                LocationKind::YamlContinue => rewrite_yaml_continue(&hit.path),
                LocationKind::MarkdownBlock => rewrite_markdown(&hit.path),
                LocationKind::OwnedFile => {
                    std::fs::remove_file(&hit.path)?;
                    Ok(StripResult::DeleteFile)
                }
                LocationKind::DataDir => Ok(StripResult::NoOp),
            }
        })();
        outcomes.push(ApplyOutcome {
            path: hit.path.clone(),
            label: hit.spec_label,
            result,
        });
    }
    outcomes
}

/// JSON specs may carry both an mcp entry and hooks. Each pass is
/// independent; report the union.
fn apply_json(
    path: &std::path::Path,
    servers_key: Option<&str>,
    has_hooks: bool,
    hooks_field: HookCommandField,
    hit: &LocationHit,
) -> Result<StripResult> {
    let mut total_removed = 0usize;
    let mut deleted_file = false;

    let try_mcp = matches!(hit.detail, HitDetail::JsonServer { .. }) || servers_key.is_some();
    let try_hooks = matches!(hit.detail, HitDetail::JsonHook { .. }) || has_hooks;

    if try_mcp {
        if let Some(key) = servers_key {
            match rewrite_json_mcp(path, key)? {
                StripResult::Removed { removed } => total_removed += removed,
                StripResult::DeleteFile => deleted_file = true,
                _ => {}
            }
        }
    }
    if try_hooks && !deleted_file {
        match rewrite_json_hooks(path, hooks_field)? {
            StripResult::Removed { removed } => total_removed += removed,
            StripResult::DeleteFile => deleted_file = true,
            _ => {}
        }
    }

    if deleted_file {
        Ok(StripResult::DeleteFile)
    } else if total_removed > 0 {
        Ok(StripResult::Removed {
            removed: total_removed,
        })
    } else {
        Ok(StripResult::NoOp)
    }
}

/// Delete the data directories listed in `plan.hits`. Caller has
/// confirmed `--purge-data` and (separately) that no `icm serve` process
/// holds the DB open.
pub(crate) fn purge_data(
    plan: &RemovalPlan,
    backup: &mut Option<BackupSession>,
) -> Vec<ApplyOutcome> {
    let mut outcomes = Vec::new();
    for hit in &plan.hits {
        if !matches!(hit.detail, HitDetail::DataDir { .. }) {
            continue;
        }
        let res: Result<StripResult> = (|| {
            if let Some(b) = backup.as_mut() {
                b.stage_dir(&hit.path)?;
            }
            if hit.path.exists() {
                std::fs::remove_dir_all(&hit.path)?;
            }
            Ok(StripResult::DeleteFile)
        })();
        outcomes.push(ApplyOutcome {
            path: hit.path.clone(),
            label: hit.spec_label,
            result: res,
        });
    }
    outcomes
}

/// Tally for the final summary.
#[derive(Default, Debug)]
pub(crate) struct ApplySummary {
    pub files_changed: usize,
    pub files_deleted: usize,
    pub entries_removed: usize,
    pub errors: Vec<(std::path::PathBuf, String)>,
    pub ambiguous: Vec<(std::path::PathBuf, String)>,
}

impl ApplySummary {
    pub fn record(&mut self, outcome: &ApplyOutcome) {
        match &outcome.result {
            Ok(StripResult::NoOp) => {}
            Ok(StripResult::Removed { removed }) => {
                self.files_changed += 1;
                self.entries_removed += removed;
            }
            Ok(StripResult::DeleteFile) => {
                self.files_deleted += 1;
            }
            Ok(StripResult::Ambiguous { reason }) => {
                self.ambiguous.push((outcome.path.clone(), reason.clone()));
            }
            Err(e) => {
                self.errors.push((outcome.path.clone(), format!("{e:#}")));
            }
        }
    }
}

/// Interactive `[y/N]` confirmation. Returns `true` for a y/Y answer.
pub(crate) fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).is_err() {
        return false;
    }
    matches!(buf.trim().chars().next(), Some('y' | 'Y'))
}
