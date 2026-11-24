//! End-to-end tests for `icm topic merge` / `icm topic rename`.
//!
//! Spawns the compiled `icm` binary with a private `HOME` and `--db`, so
//! the user's real configuration and store are never touched.

use std::path::Path;
use std::process::{Command, Output};

/// Path to the `icm` binary built for the current test run.
const ICM: &str = env!("CARGO_BIN_EXE_icm");

fn run_icm(home: &Path, db: &Path, args: &[&str]) -> Output {
    Command::new(ICM)
        .env("HOME", home)
        .env("ICM_NO_EMBEDDINGS", "1")
        .env_remove("ICM_DB")
        .env_remove("ICM_CONFIG")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .args(["--db"])
        .arg(db)
        .args(args)
        .output()
        .expect("spawn icm")
}

fn store(db: &Path, topic: &str, content: &str) {
    let home = db.parent().expect("db path has a parent");
    let out = run_icm(home, db, &["store", "--topic", topic, "--content", content]);
    assert!(
        out.status.success(),
        "store failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn list_topic(db: &Path, topic: &str) -> Vec<serde_json::Value> {
    let home = db.parent().expect("db path has a parent");
    let out = run_icm(home, db, &["list", "--topic", topic, "--format", "json"]);
    assert!(
        out.status.success(),
        "list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("list --format json was not JSON: {e}"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn merge_previews_by_default_then_applies() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("memories.db");
    store(&db, "old", "alpha fact");
    store(&db, "old", "beta fact");
    store(&db, "new", "gamma fact");

    // Default is a preview: the stats are reported, nothing moves.
    let out = run_icm(tmp.path(), &db, &["topic", "merge", "old", "new"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("dry run"), "{text}");
    assert_eq!(list_topic(&db, "old").len(), 2);
    assert_eq!(list_topic(&db, "new").len(), 1);

    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "merge", "old", "new", "--apply"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(list_topic(&db, "old").len(), 0);
    let moved = list_topic(&db, "new");
    assert_eq!(moved.len(), 3);
    assert!(moved.iter().all(|m| m["topic"] == "new"));
}

#[test]
fn merge_collision_aborts_without_write() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("memories.db");
    store(&db, "old", "duplicate summary");
    store(&db, "new", "duplicate summary");

    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "merge", "old", "new", "--apply"],
    );
    assert!(!out.status.success(), "collision must be rejected");
    assert!(stderr(&out).contains("duplicate"), "{}", stderr(&out));
    assert_eq!(list_topic(&db, "old").len(), 1);
    assert_eq!(list_topic(&db, "new").len(), 1);
}

#[test]
fn merge_dedupe_drops_the_source_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("memories.db");
    store(&db, "old", "duplicate summary");
    store(&db, "new", "duplicate summary");

    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "merge", "old", "new", "--apply", "--dedupe"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(list_topic(&db, "old").len(), 0);
    assert_eq!(list_topic(&db, "new").len(), 1);
}

#[test]
fn rename_moves_the_topic_and_rejects_an_existing_target() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("memories.db");
    store(&db, "old", "rename me");

    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "rename", "old", "fresh", "--apply"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(list_topic(&db, "old").len(), 0);
    assert_eq!(list_topic(&db, "fresh").len(), 1);

    store(&db, "other", "occupied");
    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "rename", "fresh", "other", "--apply"],
    );
    assert!(
        !out.status.success(),
        "rename onto an existing topic must fail"
    );
    assert!(stderr(&out).contains("already exists"), "{}", stderr(&out));
    assert_eq!(list_topic(&db, "fresh").len(), 1);
    assert_eq!(list_topic(&db, "other").len(), 1);
}

#[test]
fn apply_and_dry_run_are_mutually_exclusive() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("memories.db");
    store(&db, "old", "a fact");

    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "merge", "old", "new", "--apply", "--dry-run"],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("mutually exclusive"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn merge_missing_source_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("memories.db");
    store(&db, "new", "a fact");

    let out = run_icm(
        tmp.path(),
        &db,
        &["topic", "merge", "ghost", "new", "--apply"],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not found"), "{}", stderr(&out));
}
