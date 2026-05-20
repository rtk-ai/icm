//! End-to-end integration tests for the fix/init-secure changes:
//! `icm init` now (a) gates every mode by `detect_tool`, (b) persists
//! an install manifest, (c) writes instruction files to global per-tool
//! paths by default — and only mirrors them to the cwd when
//! `--per-project` is passed.
//!
//! Each test spawns the compiled `icm` binary inside a private
//! `HOME=<tempdir>` with a stripped `PATH` so no real AI binary is
//! detected.
//!
//! Gated to Linux: these assertions hard-code XDG-style paths
//! (`.local/share/icm`, `.claude/CLAUDE.md`, etc.). On macOS the
//! `directories` crate routes data to `Library/Application Support/icm/`
//! and on Windows to `%APPDATA%\icm\icm\data\`, so the same fake-HOME
//! scaffolding doesn't line up. The cross-OS logic itself is exercised
//! by the in-binary unit tests in `crates/icm-cli/src/install_manifest.rs`
//! which use the real `directories` crate on both sides.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

const ICM: &str = env!("CARGO_BIN_EXE_icm");

fn icm_in(home: &Path, cwd: &Path, path_env: &str, args: &[&str]) -> std::process::Output {
    Command::new(ICM)
        .env_clear()
        .env("HOME", home)
        .env("PATH", path_env)
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("spawn icm")
}

fn make_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd_path = cwd.clone();
    (tmp, cwd_path)
}

#[test]
fn init_with_no_detected_tools_writes_zero_files() {
    let (tmp, cwd) = make_home();
    let out = icm_in(tmp.path(), &cwd, "/dev/null", &["init", "--mode", "all"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "init should succeed; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // Walk the fake HOME and assert no instruction / config file was
    // written. The DB directory (~/.local/share/icm/) is opened by
    // every subcommand and is allowed to exist.
    let touched = list_user_writes(tmp.path());
    assert!(
        touched.is_empty(),
        "init with no detected tools wrote files: {touched:?}\n\
        stdout was:\n{}",
        String::from_utf8_lossy(&out.stdout),
    );
}

#[test]
fn init_with_no_tools_does_not_persist_an_empty_manifest() {
    let (tmp, cwd) = make_home();
    let _ = icm_in(tmp.path(), &cwd, "/dev/null", &["init", "--mode", "all"]);

    let manifest = tmp.path().join(".local/share/icm/install-manifest.json");
    assert!(
        !manifest.exists(),
        "manifest written even though zero entries were recorded"
    );
}

#[test]
fn init_force_writes_global_paths_and_records_manifest() {
    let (tmp, cwd) = make_home();
    let out = icm_in(
        tmp.path(),
        &cwd,
        "/dev/null",
        &["init", "--mode", "cli", "--force"],
    );
    assert_eq!(out.status.code(), Some(0));

    // With --force, global per-tool paths get written even without
    // detection.
    let global_claude = tmp.path().join(".claude/CLAUDE.md");
    let global_codex = tmp.path().join(".codex/AGENTS.md");
    let global_gemini = tmp.path().join(".gemini/GEMINI.md");
    assert!(
        global_claude.exists(),
        "expected {}",
        global_claude.display()
    );
    assert!(global_codex.exists(), "expected {}", global_codex.display());
    assert!(
        global_gemini.exists(),
        "expected {}",
        global_gemini.display()
    );

    // Cwd files must NOT exist (no --per-project flag).
    let cwd_claude = cwd.join("CLAUDE.md");
    let cwd_agents = cwd.join("AGENTS.md");
    let cwd_windsurf = cwd.join(".windsurfrules");
    assert!(
        !cwd_claude.exists(),
        "cwd CLAUDE.md was created without --per-project"
    );
    assert!(
        !cwd_agents.exists(),
        "cwd AGENTS.md was created without --per-project"
    );
    assert!(
        !cwd_windsurf.exists(),
        "cwd .windsurfrules was created without --per-project"
    );

    // Manifest must list each global path.
    let manifest = tmp.path().join(".local/share/icm/install-manifest.json");
    assert!(
        manifest.exists(),
        "manifest must exist after a successful init"
    );
    let raw = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        raw.contains("Claude Code"),
        "manifest missing Claude Code entry: {raw}"
    );
    assert!(
        raw.contains("schema_version"),
        "manifest missing schema_version"
    );
}

#[test]
fn init_per_project_force_writes_both_global_and_cwd() {
    let (tmp, cwd) = make_home();
    let out = icm_in(
        tmp.path(),
        &cwd,
        "/dev/null",
        &["init", "--mode", "cli", "--per-project", "--force"],
    );
    assert_eq!(out.status.code(), Some(0));

    let pairs = [
        // (global, cwd)
        (tmp.path().join(".claude/CLAUDE.md"), cwd.join("CLAUDE.md")),
        (tmp.path().join(".codex/AGENTS.md"), cwd.join("AGENTS.md")),
    ];
    for (g, c) in &pairs {
        assert!(g.exists(), "global path missing: {}", g.display());
        assert!(c.exists(), "cwd path missing: {}", c.display());
    }

    // Project-only tools (Copilot/Windsurf/Aider) get their cwd files
    // when --per-project is on AND detection passes (here via --force).
    assert!(cwd.join(".github/copilot-instructions.md").exists());
    assert!(cwd.join(".windsurfrules").exists());
    assert!(cwd.join(".aider.conventions.md").exists());
}

#[test]
fn init_is_idempotent_first_sha_wins_in_manifest() {
    let (tmp, cwd) = make_home();
    let manifest = tmp.path().join(".local/share/icm/install-manifest.json");

    // First run: writes from scratch.
    icm_in(
        tmp.path(),
        &cwd,
        "/dev/null",
        &["init", "--mode", "cli", "--force"],
    );
    let raw1 = std::fs::read_to_string(&manifest).unwrap();

    // Second run: paths already exist with the block. Manifest must
    // still load + re-save, but the recorded sha256_before should be
    // the pre-FIRST-run state — which is None (file didn't exist
    // before the first run).
    icm_in(
        tmp.path(),
        &cwd,
        "/dev/null",
        &["init", "--mode", "cli", "--force"],
    );
    let raw2 = std::fs::read_to_string(&manifest).unwrap();

    // Both runs should yield a manifest with at least Claude Code.
    assert!(raw1.contains("Claude Code"));
    assert!(raw2.contains("Claude Code"));

    // The `sha256_before` for Claude Code must be null in both runs:
    // run 1 captured pre-init = None, run 2 short-circuited via the
    // idempotent `record()` so the field was never overwritten.
    let v1: serde_json::Value = serde_json::from_str(&raw1).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&raw2).unwrap();
    let sha1 = v1["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["tool"] == "Claude Code")
        .unwrap()["sha256_before"]
        .clone();
    let sha2 = v2["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["tool"] == "Claude Code")
        .unwrap()["sha256_before"]
        .clone();
    assert_eq!(sha1, sha2, "sha256_before must be stable across init runs");
    assert!(
        sha1.is_null(),
        "first run never had a pre-state — sha must be null"
    );
}

#[test]
fn hook_only_mode_no_longer_crashes_on_fresh_home() {
    // Regression test for the missing create_dir_all bug.
    let (tmp, cwd) = make_home();
    let out = icm_in(
        tmp.path(),
        &cwd,
        "/dev/null",
        &["init", "--mode", "hook", "--force"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        tmp.path().join(".claude/settings.json").exists(),
        ".claude/settings.json must be created"
    );
}

/// Walk `home` and return every regular file path EXCLUDING data dirs
/// `.local/share/icm/` and `.cache/icm/` (those exist by side effect of
/// the SQLite store being opened at startup).
fn list_user_writes(home: &Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &Path, acc: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let s = p.to_string_lossy();
            if s.contains("/.local/share/icm") || s.contains("/.cache/icm") {
                continue;
            }
            if let Ok(meta) = std::fs::symlink_metadata(&p) {
                if meta.is_file() {
                    acc.push(p);
                } else if meta.is_dir() {
                    walk(&p, acc);
                }
            }
        }
    }
    let mut acc = Vec::new();
    walk(home, &mut acc);
    acc
}
