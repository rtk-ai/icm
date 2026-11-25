//! End-to-end tests for `icm hook prompt` (UserPromptSubmit): the
//! `[recall]` settings decide whether memories are injected and how many.
//!
//! Spawns the compiled `icm` binary with a private `HOME=<tempdir>`, an
//! explicit `--db` and `ICM_CONFIG`, so the user's real configuration and
//! store are never touched. Embeddings are off: the hook recalls without an
//! embedder.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Path to the `icm` binary built for the current test run.
const ICM: &str = env!("CARGO_BIN_EXE_icm");

fn icm(
    home: &Path,
    config: Option<&Path>,
    args: &[&str],
    stdin: Option<&str>,
) -> std::process::Output {
    let mut cmd = Command::new(ICM);
    cmd.env("HOME", home)
        // the hook keeps per-session state in the temp dir
        .env("TMPDIR", home)
        .env("ICM_NO_EMBEDDINGS", "1")
        .env_remove("ICM_DB")
        .env_remove("ICM_CONFIG")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CONFIG_HOME");
    if let Some(config) = config {
        cmd.env("ICM_CONFIG", config);
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn icm");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait icm")
}

/// A store with eight memories of project `myproj` that all match the
/// prompt, worded apart so the render-side paraphrase filter keeps them.
fn seeded(home: &Path) -> String {
    let db = home.join("memories.db").to_str().unwrap().to_string();
    let facts = [
        "kubernetes ingress terminates TLS with cert-manager issuer letsencrypt",
        "kubernetes namespace billing runs postgres sixteen replica pair",
        "kubernetes deployment dashboard listens port 8080 behind traefik",
        "kubernetes cronjob nightly backup writes bucket archives europe",
        "kubernetes secret oauth client rotated quarterly by platform team",
        "kubernetes node pool spot instances drained before upgrades",
        "kubernetes helm chart pinned version 4.2 for redis cache",
        "kubernetes horizontal autoscaler targets seventy percent cpu",
    ];
    for fact in facts {
        let out = icm(
            home,
            None,
            &["--db", &db, "store", "-t", "decisions-myproj", "-c", fact],
            None,
        );
        assert!(
            out.status.success(),
            "store failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    db
}

/// Memories injected by the hook for a prompt matching all of them.
fn injected(home: &Path, db: &str, config: Option<&Path>) -> usize {
    let input = r#"{"user_message":"what do we know about kubernetes here?","cwd":"/work/myproj"}"#;
    let out = icm(home, config, &["--db", db, "hook", "prompt"], Some(input));
    assert!(
        out.status.success(),
        "hook prompt failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with("- kubernetes"))
        .count()
}

#[test]
fn hook_prompt_injects_recall_limit_memories() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[recall]\nlimit = 3\n").unwrap();
    assert_eq!(injected(tmp.path(), &db, Some(&config)), 3);
}

#[test]
fn hook_prompt_injects_nothing_when_recall_is_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[recall]\nenabled = false\n").unwrap();
    assert_eq!(injected(tmp.path(), &db, Some(&config)), 0);
}

#[test]
fn hook_prompt_injects_five_memories_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    assert_eq!(injected(tmp.path(), &db, None), 5);
}

/// Memories injected for `message` in session `session`.
fn injected_in(
    home: &Path,
    db: &str,
    config: Option<&Path>,
    session: &str,
    message: &str,
) -> usize {
    let input =
        serde_json::json!({"user_message": message, "cwd": "/work/myproj", "session_id": session})
            .to_string();
    let out = icm(home, config, &["--db", db, "hook", "prompt"], Some(&input));
    assert!(
        out.status.success(),
        "hook prompt failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with("- kubernetes"))
        .count()
}

#[test]
fn hook_prompt_skips_short_prompts() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    assert_eq!(injected_in(tmp.path(), &db, None, "s1", "kubernetes ok"), 0);
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[recall]\nmin_prompt_words = 0\n").unwrap();
    assert_eq!(
        injected_in(tmp.path(), &db, Some(&config), "s1", "kubernetes ok"),
        5
    );
}

#[test]
fn hook_prompt_does_not_inject_a_memory_twice_in_a_session() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    let ask = "what do we know about kubernetes here?";
    assert_eq!(injected_in(tmp.path(), &db, None, "s1", ask), 5);
    assert_eq!(injected_in(tmp.path(), &db, None, "s1", ask), 0);
    // another session gets them, and so does s1 once it starts again (compact, clear, resume)
    assert_eq!(injected_in(tmp.path(), &db, None, "s2", ask), 5);
    let out = icm(
        tmp.path(),
        None,
        &["--db", &db, "hook", "start"],
        Some(r#"{"session_id":"s1","cwd":"/work/myproj"}"#),
    );
    assert!(out.status.success());
    assert_eq!(injected_in(tmp.path(), &db, None, "s1", ask), 5);
}

#[test]
fn hook_prompt_repeats_memories_when_skip_seen_is_off() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[recall]\nskip_seen = false\n").unwrap();
    let ask = "what do we know about kubernetes here?";
    assert_eq!(injected_in(tmp.path(), &db, Some(&config), "s1", ask), 5);
    assert_eq!(injected_in(tmp.path(), &db, Some(&config), "s1", ask), 5);
}

/// A running `icm serve --http` on a free loopback port, over `db`, killed
/// on drop.
struct Server {
    child: std::process::Child,
    url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(home: &Path, db: &str) -> Server {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let child = Command::new(ICM)
        .env("HOME", home)
        .env("ICM_NO_EMBEDDINGS", "1")
        .env_remove("ICM_DB")
        .env_remove("ICM_CONFIG")
        .args(["--db", db, "serve", "--http", &addr])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn icm serve");
    let url = format!("http://{addr}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while ureq::get(&format!("{url}/health")).call().is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "icm serve did not come up"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Server { child, url }
}

/// With `server_url` set, the memories come from the running server, up to
/// `server_limit`, not from the hook's own recall (`limit`).
#[test]
fn hook_prompt_recalls_through_a_running_server() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    let server = serve(tmp.path(), &db);
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[recall]\nlimit = 3\nserver_url = \"{}\"\nserver_limit = 7\n",
            server.url
        ),
    )
    .unwrap();
    assert_eq!(injected(tmp.path(), &db, Some(&config)), 7);
}

/// A server that does not answer costs at most the timeout: the hook falls
/// back to its own recall.
#[test]
fn hook_prompt_falls_back_when_the_server_does_not_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let db = seeded(tmp.path());
    // A bound listener that never accepts: the connection hangs, not refused.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", silent.local_addr().unwrap());
    let config = tmp.path().join("config.toml");
    std::fs::write(
        &config,
        format!("[recall]\nlimit = 3\nserver_url = \"{url}\"\nserver_timeout_ms = 300\n"),
    )
    .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(injected(tmp.path(), &db, Some(&config)), 3);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the hook waited {:?}",
        started.elapsed()
    );
}
