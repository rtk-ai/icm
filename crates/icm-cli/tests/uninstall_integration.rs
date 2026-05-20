//! End-to-end integration tests for `icm uninstall` (issue #229).
//!
//! Each test spawns the compiled `icm` binary inside a private
//! `HOME=<tempdir>` so the user's real configuration is never touched.
//! These tests complement the in-binary unit tests in
//! `crates/icm-cli/src/uninstall/*.rs` by validating the full
//! discovery → mutation → re-scan loop through the public CLI surface.
//!
//! Gated to Linux: the assertions hard-code XDG-style data paths and
//! the seeded JSON command literals use a sample binary path
//! (`/x/icm`) that doesn't fit Windows. The in-binary unit tests
//! exercise the cross-OS logic directly through `directories`.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

/// Path to the `icm` binary built for the current test run. Cargo
/// injects `CARGO_BIN_EXE_<bin-name>` into the test process environment.
const ICM: &str = env!("CARGO_BIN_EXE_icm");

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn icm_in(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(ICM)
        .env("HOME", home)
        // Wipe any inherited tool-config overrides so the test sees a
        // clean view of the fake home.
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("GEMINI_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("COPILOT_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("spawn icm")
}

fn seed_minimal_residue(home: &Path) {
    // One JSON mcpServers entry.
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers":{"icm":{"command":"/x/icm","args":["serve"]},"other":{"command":"/x/o"}}}"#,
    );
    // One hooks block (Claude shape).
    write(
        &home.join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/x/icm hook pre"}]}]}}"#,
    );
    // One TOML mcp.
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.icm]\ncommand=\"/x/icm\"\nargs=[\"serve\"]\n",
    );
    // One owned skill file.
    write(
        &home.join(".claude/commands/recall.md"),
        "Search ICM memory for: $ARGUMENTS\n",
    );
}

#[test]
fn check_exits_one_when_residue_present_then_zero_after_uninstall() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = home.join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    seed_minimal_residue(home);

    let pre = icm_in(home, &cwd, &["uninstall", "--check"]);
    assert_eq!(
        pre.status.code(),
        Some(1),
        "expected --check exit 1, got {:?}\nstdout: {}\nstderr: {}",
        pre.status.code(),
        String::from_utf8_lossy(&pre.stdout),
        String::from_utf8_lossy(&pre.stderr),
    );

    let run = icm_in(home, &cwd, &["uninstall", "-y"]);
    assert!(
        run.status.success() || run.status.code() == Some(0),
        "uninstall should exit 0; got {:?}\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    let post = icm_in(home, &cwd, &["uninstall", "--check"]);
    assert_eq!(
        post.status.code(),
        Some(0),
        "expected --check exit 0 after uninstall; got {:?}\nstdout: {}",
        post.status.code(),
        String::from_utf8_lossy(&post.stdout),
    );
}

#[test]
fn dry_run_does_not_modify_files() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = home.join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    seed_minimal_residue(home);

    let before = std::fs::read_to_string(home.join(".claude.json")).unwrap();
    let before_settings = std::fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    let before_toml = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    let before_skill = std::fs::read_to_string(home.join(".claude/commands/recall.md")).unwrap();

    let out = icm_in(home, &cwd, &["uninstall", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0));

    let after = std::fs::read_to_string(home.join(".claude.json")).unwrap();
    let after_settings = std::fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    let after_toml = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    let after_skill = std::fs::read_to_string(home.join(".claude/commands/recall.md")).unwrap();

    assert_eq!(before, after, "--dry-run modified .claude.json");
    assert_eq!(
        before_settings, after_settings,
        "--dry-run modified settings"
    );
    assert_eq!(before_toml, after_toml, "--dry-run modified TOML");
    assert_eq!(before_skill, after_skill, "--dry-run modified skill file");
}

#[test]
fn uninstall_is_idempotent_when_already_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = home.join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    // No seeding — fake home is already pristine.

    let out = icm_in(home, &cwd, &["uninstall", "-y"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "uninstall on a clean home should exit 0; got {:?}",
        out.status.code(),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Nothing to uninstall") || stdout.contains("already clean"),
        "expected an 'already clean' message; got: {stdout}"
    );
}

// `directories::ProjectDirs` returns OS-specific data/cache paths that
// don't match the Linux XDG layout this test seeds. Gate it to Linux
// where seed and code agree — the macOS/Windows path resolution is
// covered by the unit tests inside the binary which use the real
// `directories` crate at the right side of the boundary.
#[cfg(target_os = "linux")]
#[test]
fn purge_data_does_not_recreate_db_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = home.join("proj");
    std::fs::create_dir_all(&cwd).unwrap();

    // Seed both a config residue and the ProjectDirs data directory so
    // --purge-data has something to delete.
    seed_minimal_residue(home);
    let data_dir = home.join(".local/share/icm");
    let cache_dir = home.join(".cache/icm/models");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    write(&data_dir.join("memories.db"), "stub");
    write(&cache_dir.join("model.onnx"), "stub-model");

    assert!(data_dir.exists(), "precondition: data dir must exist");
    assert!(cache_dir.exists(), "precondition: cache dir must exist");

    let out = icm_in(home, &cwd, &["uninstall", "-y", "--purge-data"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "uninstall --purge-data should exit 0; got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !data_dir.exists(),
        "--purge-data should have deleted {}",
        data_dir.display()
    );
    assert!(
        !cache_dir.exists(),
        "--purge-data should have deleted {}",
        cache_dir.display()
    );

    // Regression for the C5 fix: the subsequent --check invocation
    // must NOT re-create the data directory by accidentally opening
    // the SQLite store.
    let post = icm_in(home, &cwd, &["uninstall", "--check"]);
    assert_eq!(
        post.status.code(),
        Some(0),
        "post-purge --check must be clean"
    );
    assert!(
        !data_dir.exists(),
        "subsequent `uninstall --check` re-created {}; the open_store bypass regressed",
        data_dir.display()
    );
}

#[test]
fn scan_dir_strips_block_from_nested_project_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let cwd = home.join("primary-proj");
    std::fs::create_dir_all(&cwd).unwrap();

    let other = home.join("other-proj/sub");
    std::fs::create_dir_all(&other).unwrap();
    let target = other.join("CLAUDE.md");
    write(
        &target,
        "preamble\n\n<!-- icm:start -->\nblock\n<!-- icm:end -->\n\ntrailing\n",
    );
    // Decoy: same block inside a skip-dir must be ignored.
    let decoy_dir = home.join("other-proj/node_modules/foo");
    std::fs::create_dir_all(&decoy_dir).unwrap();
    let decoy = decoy_dir.join("README.md");
    write(&decoy, "<!-- icm:start -->\ndecoy\n<!-- icm:end -->\n");

    let out = icm_in(
        home,
        &cwd,
        &[
            "uninstall",
            "-y",
            "--scan-dir",
            other.parent().unwrap().to_str().unwrap(),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let after = std::fs::read_to_string(&target).unwrap();
    assert!(
        !after.contains("<!-- icm:start -->"),
        "scan-dir target was not stripped: {after}"
    );
    assert!(after.contains("preamble"));
    assert!(after.contains("trailing"));

    // Decoy must be untouched.
    let decoy_after = std::fs::read_to_string(&decoy).unwrap();
    assert!(
        decoy_after.contains("<!-- icm:start -->"),
        "decoy under node_modules was modified; skip-dir filter failed"
    );
}
