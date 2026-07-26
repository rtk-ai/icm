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

/// Write `content` to `path` atomically: write to a temp file in the same
/// directory, then rename over the target. A crash or kill mid-write
/// leaves either the old file fully intact or the new one fully written —
/// never a truncated/corrupted file (audit finding: `formats.rs`/
/// `backup.rs` both used plain `fs::write`, truncate-then-write, which can
/// leave the user's real settings.json/config.toml or our own backup
/// manifest half-written on a crash).
pub(crate) fn atomic_write(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".icm-tmp");
    let tmp_path = path.with_file_name(tmp_name);
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, path)
}

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

    /// Override the backup root. Defaults to
    /// `<icm-data-dir>/uninstall-backups/<ts>/` (resolved by
    /// `directories::ProjectDirs` so the location follows each OS:
    /// `~/.local/share/icm/` on Linux/WSL, `~/Library/Application Support/icm/`
    /// on macOS, `%APPDATA%\icm\icm\data\` on Windows).
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
    // Process detection only matters when --purge-data is about to
    // delete the SQLite DB underneath a live `icm serve` (WAL
    // corruption risk). Skip it otherwise — most users run in
    // --mode standard which never spawns `icm serve`.
    if opts.purge_data {
        let detection = process::detect_icm_serve();
        plan.processes = detection.processes;
        plan.process_detection_unsupported = detection.unsupported;
    }

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

    // Resolve the default backup root via ProjectDirs so the layout
    // follows each OS's convention (XDG on Linux/WSL, Application
    // Support on macOS, AppData/Roaming on Windows). Falls back to a
    // dotfile at $HOME when ProjectDirs is unavailable (stripped
    // sandboxes without standard env vars).
    let default_backup_base = locations::icm_data_dir()
        .map(|d| d.join("uninstall-backups"))
        .unwrap_or_else(|| dirs.home.join(".icm-uninstall-backups"));

    let mut backup_session: Option<backup::BackupSession> = if opts.no_backup {
        None
    } else {
        Some(backup::BackupSession::new(
            opts.backup_dir.as_deref(),
            &default_backup_base,
        )?)
    };

    let mut summary = mutate::ApplySummary::default();
    let outcomes = mutate::apply(&plan, &specs, &mut backup_session);
    for o in &outcomes {
        summary.record(o);
    }

    // Persist the manifest **before** any --purge-data step: when the
    // default backup root lives under the data dir we're about to delete,
    // we want the manifest to have been written at least once before
    // the recursive remove takes the whole tree.
    if let Some(b) = &backup_session {
        b.commit_manifest()?;
    }

    if opts.purge_data {
        // Refuse to purge while `icm serve` is running unless the user
        // explicitly opted in via `-y`. Serve keeps the SQLite DB open
        // via WAL; deleting underneath it can corrupt cross-session
        // neighbour processes. Audit finding: process detection isn't
        // implemented on every platform (Windows/BSD) and can fail even
        // where it is (e.g. `/proc` unreadable) — `process_detection_unsupported`
        // must gate this the same as "a process was found," or an empty
        // list silently defeats the only safeguard here.
        if should_refuse_purge(&plan, opts.yes) {
            println!();
            if plan.process_detection_unsupported {
                println!(
                    "Refusing to --purge-data: `icm serve` process detection isn't \
                    supported on this platform, so we can't confirm none is running. \
                    Stop any `icm serve` process yourself, then pass -y to override."
                );
            } else {
                println!(
                    "Refusing to --purge-data: {} `icm serve` process(es) detected. \
                    Stop them with `pkill -f 'icm serve'` (or pass -y to override at your own risk).",
                    plan.processes.len()
                );
            }
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
            } else if plan.process_detection_unsupported {
                println!();
                println!(
                    "WARNING: process detection isn't supported on this platform — \
                    purging the DB anyway because -y was passed. Make sure `icm serve` \
                    isn't running."
                );
            }
            let purge_outcomes = mutate::purge_data(&plan, &mut backup_session);
            for o in &purge_outcomes {
                summary.record(o);
            }
        }
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

/// Whether `--purge-data` must refuse: a live `icm serve` was detected, or
/// detection wasn't possible at all (audit finding — an empty process list
/// is otherwise indistinguishable from "confirmed nothing running"), and
/// the user hasn't passed `-y` to override.
fn should_refuse_purge(plan: &discover::RemovalPlan, yes: bool) -> bool {
    (!plan.processes.is_empty() || plan.process_detection_unsupported) && !yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uninstall::discover::RunningProcess;

    #[test]
    fn should_refuse_purge_when_a_process_is_detected() {
        let mut plan = discover::RemovalPlan::default();
        plan.processes.push(RunningProcess {
            pid: 1234,
            cmdline: "icm serve".into(),
        });
        assert!(should_refuse_purge(&plan, false));
        assert!(!should_refuse_purge(&plan, true), "-y must override");
    }

    /// Audit regression: detection-unsupported must refuse by default, the
    /// same as a confirmed-running process — not be treated as "confirmed
    /// nothing running" just because the list happens to be empty.
    #[test]
    fn should_refuse_purge_when_detection_is_unsupported() {
        let plan = discover::RemovalPlan {
            process_detection_unsupported: true,
            ..Default::default()
        };
        assert!(should_refuse_purge(&plan, false));
        assert!(!should_refuse_purge(&plan, true), "-y must override");
    }

    #[test]
    fn should_not_refuse_purge_when_confirmed_clean() {
        let plan = discover::RemovalPlan::default();
        assert!(!should_refuse_purge(&plan, false));
    }

    #[test]
    fn atomic_write_replaces_content_and_leaves_no_temp_file() {
        let dir =
            std::env::temp_dir().join(format!("icm-atomic-write-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target.txt");
        std::fs::write(&path, "v1").unwrap();

        atomic_write(&path, b"v2").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        assert!(!path.with_file_name("target.txt.icm-tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Audit finding: plain `fs::write` truncates the destination before
    /// writing, so a write failure mid-way corrupts the existing file. The
    /// atomic version must leave the original file fully intact if the
    /// (separate) temp file write fails, since the destination is only ever
    /// touched by the final `rename`.
    #[test]
    fn atomic_write_preserves_original_when_temp_write_fails() {
        let dir =
            std::env::temp_dir().join(format!("icm-atomic-write-fail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target.txt");
        std::fs::write(&path, "original").unwrap();
        // Force the temp-file write to fail by pre-occupying its path with
        // a directory instead of a plain file.
        let tmp_path = path.with_file_name("target.txt.icm-tmp");
        std::fs::create_dir_all(&tmp_path).unwrap();

        let result = atomic_write(&path, b"new content");

        assert!(result.is_err(), "write into a directory must fail");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "original",
            "destination must be untouched when the temp write fails"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
