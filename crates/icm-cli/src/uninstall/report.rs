//! Stdout formatters for the read-only modes (`--check`, `--dry-run`,
//! `--audit`) and the post-mutation summary.

use super::discover::{HitDetail, RemovalPlan};

/// Print the audit / dry-run preview. Groups hits by file so users see
/// each path once with all the things uninstall would touch under it.
/// `purge_data` toggles the wording for [`HitDetail::DataDir`] so the
/// preview matches what the run is actually about to do.
pub(crate) fn print_audit(plan: &RemovalPlan, header: &str, purge_data: bool) {
    println!("{header}");
    println!("{}", "=".repeat(header.len()));

    if plan.hits.is_empty() && plan.scan_dir_hits.is_empty() {
        println!("No known ICM residue found.");
        return;
    }

    print_section("Configured locations", &plan.hits, purge_data);
    if !plan.scan_dir_hits.is_empty() {
        print_section(
            "Project tree references (--scan-dir)",
            &plan.scan_dir_hits,
            purge_data,
        );
    }

    if !plan.processes.is_empty() {
        println!();
        println!("Running `icm serve` processes:");
        for p in &plan.processes {
            println!("  pid={:<6} {}", p.pid, p.cmdline);
        }
    }

    println!();
    println!("Total: {} item(s).", plan.total_hits());
}

fn print_section(title: &str, hits: &[super::discover::LocationHit], purge_data: bool) {
    if hits.is_empty() {
        return;
    }
    println!();
    println!("{title}");
    println!("{}", "-".repeat(title.len()));

    // Group by path so multiple hits in the same file collapse together.
    let mut current: Option<&std::path::Path> = None;
    for hit in hits {
        if current != Some(hit.path.as_path()) {
            println!();
            println!(
                "{:<28} {}",
                format!("[{}]", hit.spec_label),
                hit.path.display()
            );
            current = Some(hit.path.as_path());
        }
        match &hit.detail {
            HitDetail::JsonServer { pointer } => {
                println!("  MCP server entry at {pointer}");
            }
            HitDetail::JsonHook { event, command } => {
                println!("  hook {event}: {command}");
            }
            HitDetail::TomlTable { table } => {
                println!("  TOML table {table}");
            }
            HitDetail::YamlBlock { start_line, lines } => {
                println!(
                    "  YAML block at line {start_line} (~{lines} line(s)) — manual review may be needed"
                );
            }
            HitDetail::MarkdownBlock {
                start_line,
                end_line,
                file_will_be_empty,
            } => {
                let tag = if *file_will_be_empty {
                    " (file will be deleted, no other content)"
                } else {
                    ""
                };
                println!("  Markdown block lines {start_line}-{end_line}{tag}");
            }
            HitDetail::OwnedFile { bytes } => {
                println!("  Owned file ({} byte(s)) — will be deleted", bytes);
            }
            HitDetail::DataDir { bytes_total, files } => {
                let tag = if purge_data {
                    "will be deleted (--purge-data)"
                } else {
                    "kept unless --purge-data"
                };
                println!("  Data directory: {files} file(s), {bytes_total} byte(s) — {tag}",);
            }
        }
    }
}

/// Brief output for `--check`. Returns the exit code.
pub(crate) fn print_check(plan: &RemovalPlan) -> i32 {
    if plan.is_empty() {
        println!("OK: no known ICM residue found");
        super::exit_codes::CLEAN
    } else {
        println!("FOUND: {} known ICM residue item(s)", plan.total_hits());
        super::exit_codes::CHECK_RESIDUE
    }
}

/// Per-outcome and aggregate summary after a mutation run.
pub(crate) fn print_apply_summary(
    outcomes: &[super::mutate::ApplyOutcome],
    summary: &super::mutate::ApplySummary,
    backup_root: Option<&std::path::Path>,
    residue_after: usize,
) -> i32 {
    println!();
    println!("Per-file results");
    println!("----------------");
    for o in outcomes {
        let tag = match &o.result {
            Ok(super::formats::StripResult::NoOp) => "no-op".to_string(),
            Ok(super::formats::StripResult::Removed { removed }) => {
                format!("removed {removed}")
            }
            Ok(super::formats::StripResult::DeleteFile) => "deleted".to_string(),
            Ok(super::formats::StripResult::Ambiguous { reason: _ }) => "ambiguous".to_string(),
            Err(e) => format!("ERROR: {e:#}"),
        };
        println!("  [{:<22}] {:<10} {}", o.label, tag, o.path.display());
    }

    println!();
    println!("Summary");
    println!("-------");
    println!("  Files modified : {}", summary.files_changed);
    println!("  Files deleted  : {}", summary.files_deleted);
    println!("  Entries removed: {}", summary.entries_removed);

    if !summary.ambiguous.is_empty() {
        println!();
        println!("Ambiguous (manual review needed):");
        for (p, why) in &summary.ambiguous {
            println!("  {}: {}", p.display(), why);
        }
    }
    if !summary.errors.is_empty() {
        println!();
        println!("Errors:");
        for (p, why) in &summary.errors {
            println!("  {}: {}", p.display(), why);
        }
    }
    if let Some(root) = backup_root {
        println!();
        println!(
            "Backups written under {} (restore with `cp -a <ts>/files/. /`)",
            root.display()
        );
    }

    if residue_after > 0 {
        println!();
        println!(
            "WARNING: {residue_after} item(s) remain after the run (see above). \
            Re-run with --dry-run to inspect."
        );
    }

    if !summary.errors.is_empty() {
        super::exit_codes::MUTATION_ERROR
    } else if !summary.ambiguous.is_empty() || residue_after > 0 {
        super::exit_codes::PARTIAL
    } else {
        super::exit_codes::CLEAN
    }
}
