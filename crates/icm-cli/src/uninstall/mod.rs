//! Reverse `icm init`: remove every configuration mutation across detected
//! AI tools, with timestamped backups, dry-run preview, audit, and check.
//!
//! Issue #229: <https://github.com/rtk-ai/icm/issues/229>.
//!
//! See the crate-level docs at `crates/icm-cli/src/uninstall/locations.rs`
//! for the catalog of paths mirrored from `cmd_init`. The high-level flow
//! is `build_locations -> discover::scan -> report or mutate -> verify`.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

pub(crate) mod backup;
pub(crate) mod discover;
pub(crate) mod formats;
pub(crate) mod locations;
pub(crate) mod mutate;
pub(crate) mod process;
pub(crate) mod report;
pub(crate) mod scan_dir;

/// CLI surface for `icm uninstall`. Kept here so the rest of the crate only
/// imports `UninstallOpts` from this module.
#[derive(Args, Debug, Clone)]
pub struct UninstallOpts {
    /// Preview removals without modifying anything. Always exits 0.
    #[arg(long)]
    pub dry_run: bool,

    /// Group output by file with full discovery detail. Read-only, exits 0.
    #[arg(long)]
    pub audit: bool,

    /// Exit 0 iff no ICM residue is found. No mutation, no backup.
    #[arg(long)]
    pub check: bool,

    /// Also delete the SQLite memory database and the fastembed model cache.
    /// Off by default — your personal memories are preserved.
    #[arg(long)]
    pub purge_data: bool,

    /// Additionally scan this project tree for free-form ICM references in
    /// instruction files (CLAUDE.md, AGENTS.md, .windsurfrules, etc.).
    #[arg(long, value_name = "PATH")]
    pub scan_dir: Option<PathBuf>,

    /// Skip the interactive confirmation prompt.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Override the backup root. Defaults to `~/.icm-uninstall-backups/<ts>`.
    #[arg(long, value_name = "PATH")]
    pub backup_dir: Option<PathBuf>,

    /// Disable backups entirely. Not recommended.
    #[arg(long)]
    pub no_backup: bool,
}

/// Exit codes published in `--help`.
///
/// | code | meaning |
/// |------|---------|
/// | 0    | clean / dry-run / audit succeeded |
/// | 1    | `--check` found residue |
/// | 2    | user declined the confirmation prompt |
/// | 3    | partial success — residue remains after mutation (e.g. ambiguous YAML) |
/// | 4    | I/O or parse error during mutation |
pub mod exit_codes {
    pub const CLEAN: i32 = 0;
    pub const CHECK_RESIDUE: i32 = 1;
    pub const USER_DECLINED: i32 = 2;
    pub const PARTIAL: i32 = 3;
    pub const MUTATION_ERROR: i32 = 4;
}

/// Entry point. Returns the process exit code; the caller is responsible
/// for invoking `std::process::exit`.
pub fn run(opts: UninstallOpts) -> Result<i32> {
    let dirs = locations::DirContext::from_env()?;
    let specs = locations::build_locations(&dirs);
    let mut plan = discover::scan(&specs, opts.purge_data)?;
    if let Some(dir) = opts.scan_dir.as_deref() {
        plan.scan_dir_hits = scan_dir::scan_dir(dir)?;
    }
    plan.processes = process::detect_icm_serve();

    // --- Read-only modes ---
    if opts.check {
        return Ok(report::print_check(&plan));
    }
    if opts.audit {
        report::print_audit(&plan, "ICM uninstall audit", opts.purge_data);
        return Ok(exit_codes::CLEAN);
    }
    if opts.dry_run {
        report::print_audit(&plan, "ICM uninstall (dry run)", opts.purge_data);
        return Ok(exit_codes::CLEAN);
    }

    // --- Mutating run ---
    if plan.is_empty() {
        println!("Nothing to uninstall — already clean.");
        return Ok(exit_codes::CLEAN);
    }
    report::print_audit(&plan, "ICM uninstall plan", opts.purge_data);

    if !opts.yes && !mutate::confirm("Proceed with removal?") {
        println!("Aborted (no changes made).");
        return Ok(exit_codes::USER_DECLINED);
    }

    let mut backup_session: Option<backup::BackupSession> = if opts.no_backup {
        None
    } else {
        Some(backup::BackupSession::new(
            opts.backup_dir.as_deref(),
            &dirs.home,
        )?)
    };

    let mut summary = mutate::ApplySummary::default();
    let outcomes = mutate::apply(&plan, &specs, &mut backup_session);
    for o in &outcomes {
        summary.record(o);
    }

    if opts.purge_data {
        // Refuse to purge while `icm serve` is running unless the user
        // explicitly opted in via `-y`. Serve keeps the SQLite DB open
        // via WAL; deleting underneath it can corrupt cross-session
        // neighbour processes.
        if !plan.processes.is_empty() && !opts.yes {
            println!();
            println!(
                "Refusing to --purge-data: {} `icm serve` process(es) detected. \
                Stop them with `pkill -f 'icm serve'` (or pass -y to override at your own risk).",
                plan.processes.len()
            );
            for p in &plan.processes {
                println!("  pid={:<6} {}", p.pid, p.cmdline);
            }
        } else {
            if !plan.processes.is_empty() {
                println!();
                println!(
                    "WARNING: {} `icm serve` process(es) still running — \
                    purging the DB anyway because -y was passed.",
                    plan.processes.len()
                );
            }
            let purge_outcomes = mutate::purge_data(&plan, &mut backup_session);
            for o in &purge_outcomes {
                summary.record(o);
            }
        }
    }

    if let Some(b) = &backup_session {
        b.commit_manifest()?;
    }

    // Verify pass: rescan to detect any residue (ambiguous YAML, parse
    // errors that skipped a file, etc.).
    let after = discover::scan(&specs, opts.purge_data)?;
    let exit = report::print_apply_summary(
        &outcomes,
        &summary,
        backup_session.as_ref().map(|b| b.root()),
        after.total_hits(),
    );
    Ok(exit)
}
