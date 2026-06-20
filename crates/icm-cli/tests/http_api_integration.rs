//! End-to-end integration tests for `icm serve --http` (issue #290).
//!
//! Spawns the compiled `icm` binary with a fresh tempdir DB, waits
//! for the HTTP server to come up, then exercises:
//! - POST /store with TOON body assertion
//! - POST /recall returning the stored memory in TOON
//! - `?format=json` returning a JSON array
//! - GET /stats, GET /topics, GET /health
//!
//! Gated on `feature = "http-api"` so non-default builds don't fail
//! to find the `--http` flag.
//!
//! Gated to Linux for two reasons:
//!  1. macOS / Windows runners don't load sqlite-vec from the build
//!     output reliably from a child process spawned in a tempdir; the
//!     existing `init_secure_integration.rs` uses the same gating
//!     rationale.
//!  2. We use `--no-embeddings` here on purpose — the goal is to
//!     verify the HTTP transport, NOT the embedder warm-up speed
//!     (which the issue's manual smoke covers).
#![cfg(all(target_os = "linux", feature = "http-api"))]

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ICM: &str = env!("CARGO_BIN_EXE_icm");

struct ServerGuard {
    child: Child,
    addr: String,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reserve a free localhost port by binding then closing.
fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn spawn_server(db_path: &std::path::Path, extra: &[&str]) -> ServerGuard {
    let port = pick_port();
    let addr = format!("127.0.0.1:{port}");

    let mut cmd = Command::new(ICM);
    cmd.arg("--no-embeddings")
        .arg("--db")
        .arg(db_path)
        .arg("serve")
        .arg("--http")
        .arg(&addr)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn icm serve --http");

    // Wait until the server's `[icm http] listening on …` line shows
    // up on stderr, OR until we successfully poke / health, whichever
    // comes first. 10 s is generous — release builds take <1 s.
    let stderr = child.stderr.take().expect("stderr piped");
    let stderr_addr = addr.clone();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let r = BufReader::new(stderr);
        for line in r.lines().map_while(Result::ok) {
            // Mirror to test stderr so failures show the server's view.
            eprintln!("[server] {line}");
            if line.contains(&stderr_addr) {
                let _ = tx.send(());
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if rx.try_recv().is_ok() {
            break;
        }
        if probe_health(&addr) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !probe_health(&addr) {
        let _ = child.kill();
        panic!("server did not become reachable on {addr} within 10 s");
    }
    ServerGuard { child, addr }
}

fn probe_health(addr: &str) -> bool {
    ureq::get(&format!("http://{addr}/health"))
        .timeout(Duration::from_millis(500))
        .call()
        .ok()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

fn post_json(addr: &str, path: &str, body: &str) -> ureq::Response {
    ureq::post(&format!("http://{addr}{path}"))
        .timeout(Duration::from_secs(5))
        .set("content-type", "application/json")
        .send_string(body)
        .expect("POST")
}

fn get(addr: &str, path: &str) -> ureq::Response {
    ureq::get(&format!("http://{addr}{path}"))
        .timeout(Duration::from_secs(5))
        .call()
        .expect("GET")
}

fn temp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("icm.sqlite");
    (dir, path)
}

#[test]
fn store_then_recall_returns_toon_row() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &[]);

    // 1. Store
    let store_resp = post_json(
        &server.addr,
        "/store",
        r#"{"topic":"t","content":"hello world","keywords":"x"}"#,
    );
    assert_eq!(store_resp.status(), 200);
    let ct = store_resp.header("content-type").unwrap_or("").to_string();
    assert!(ct.starts_with("text/plain"), "toon CT, got: {ct}");
    let body = store_resp.into_string().unwrap();
    assert!(body.starts_with("memories[1]{"), "toon header: {body}");
    assert!(body.contains("hello world"), "stored summary: {body}");

    // 2. Recall — must find the stored memory.
    let recall_resp = post_json(
        &server.addr,
        "/recall",
        r#"{"query":"hello","topic":"t","limit":5}"#,
    );
    assert_eq!(recall_resp.status(), 200);
    let body = recall_resp.into_string().unwrap();
    assert!(
        body.starts_with("memories[") && body.contains("hello world"),
        "recall toon body: {body}"
    );
}

#[test]
fn recall_format_json_query_returns_application_json() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &[]);

    post_json(
        &server.addr,
        "/store",
        r#"{"topic":"t","content":"json variant hit"}"#,
    );

    let resp = post_json(
        &server.addr,
        "/recall?format=json",
        r#"{"query":"json","topic":"t"}"#,
    );
    assert_eq!(resp.status(), 200);
    let ct = resp.header("content-type").unwrap_or("").to_string();
    assert!(ct.starts_with("application/json"), "json CT, got: {ct}");

    let body = resp.into_string().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("response is valid json");
    let arr = parsed.as_array().expect("recall json is an array");
    assert!(!arr.is_empty(), "expected at least 1 hit, got: {body}");
    assert!(arr.iter().any(|m| m["summary"] == "json variant hit"));
}

#[test]
fn stats_and_topics_respect_format_negotiation() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &[]);

    post_json(
        &server.addr,
        "/store",
        r#"{"topic":"alpha","content":"first"}"#,
    );
    post_json(
        &server.addr,
        "/store",
        r#"{"topic":"beta","content":"second"}"#,
    );

    let s = get(&server.addr, "/stats");
    let body = s.into_string().unwrap();
    assert!(body.starts_with("stats["), "stats toon: {body}");

    let s = get(&server.addr, "/stats?format=json");
    assert_eq!(s.header("content-type").unwrap_or(""), "application/json");
    let parsed: serde_json::Value = serde_json::from_str(&s.into_string().unwrap()).unwrap();
    assert!(parsed["total_memories"].as_i64().unwrap() >= 2);

    let t = get(&server.addr, "/topics");
    let body = t.into_string().unwrap();
    assert!(body.starts_with("topics["), "topics toon: {body}");
    assert!(body.contains("alpha"));
    assert!(body.contains("beta"));
}

#[test]
fn bearer_token_required_when_configured() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &["--token", "s3cr3t"]);

    // Health works without auth (it's the liveness probe).
    assert!(probe_health(&server.addr));

    // Without a token, /recall is 401.
    let resp = ureq::post(&format!("http://{}/recall", server.addr))
        .timeout(Duration::from_secs(5))
        .set("content-type", "application/json")
        .send_string(r#"{"query":"foo"}"#);
    match resp {
        Err(ureq::Error::Status(code, _)) => assert_eq!(code, 401),
        other => panic!("expected 401, got {other:?}"),
    }

    // With the wrong token, still 401.
    let resp = ureq::post(&format!("http://{}/recall", server.addr))
        .timeout(Duration::from_secs(5))
        .set("authorization", "Bearer wrong")
        .set("content-type", "application/json")
        .send_string(r#"{"query":"foo"}"#);
    match resp {
        Err(ureq::Error::Status(code, _)) => assert_eq!(code, 401),
        other => panic!("expected 401, got {other:?}"),
    }

    // With the right token, 200.
    let resp = ureq::post(&format!("http://{}/recall", server.addr))
        .timeout(Duration::from_secs(5))
        .set("authorization", "Bearer s3cr3t")
        .set("content-type", "application/json")
        .send_string(r#"{"query":"foo","topic":"none"}"#)
        .expect("authenticated /recall");
    assert_eq!(resp.status(), 200);
}

#[test]
fn missing_required_fields_return_400() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &[]);

    // /store without topic
    let resp = ureq::post(&format!("http://{}/store", server.addr))
        .timeout(Duration::from_secs(5))
        .set("content-type", "application/json")
        .send_string(r#"{"content":"x"}"#);
    match resp {
        Err(ureq::Error::Status(code, _)) => {
            assert!(code == 400 || code == 422, "expected 4xx, got {code}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}
