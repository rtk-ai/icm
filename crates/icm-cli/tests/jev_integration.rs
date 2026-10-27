//! End-to-end tests for Jev reranking in `icm recall` (`[recall] reranker`).
//!
//! Runs the compiled `icm` binary against a fake `/v1/systemone` server on
//! localhost (`[recall] reranker_url`), with an isolated DB, HOME and config.
//! Covers: reorder + truncate by score on both recall engines, 3x
//! over-fetch, auth header, Laya without key or model, a failing server
//! that leaves the local order (and leaks neither key nor response body),
//! and that the per-prompt hook never calls the reranker.
//!
//! Linux-only for the same sqlite-vec child-process reason as
//! `http_api_integration.rs`.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

const ICM: &str = env!("CARGO_BIN_EXE_icm");
const KEY: &str = "test-key-do-not-leak";

/// What the fake server saw in one request.
struct Seen {
    auth: String,
    body: Value,
}

/// Serve exactly one request, replying with `status` and a body built
/// from the parsed request. Returns the endpoint URL and a receiver for
/// what the server saw.
fn fake_jev(status: u16, reply: fn(&Value) -> String) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (mut auth, mut len) = (String::new(), 0usize);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            } else if lower.starts_with("authorization:") {
                auth = line["authorization:".len()..].trim().to_string();
            }
        }
        let mut buf = vec![0; len];
        reader.read_exact(&mut buf).unwrap();
        let body: Value = serde_json::from_slice(&buf).unwrap();
        let out = reply(&body);
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
            out.len()
        )
        .unwrap();
        let _ = tx.send(Seen { auth, body });
    });
    (url, rx)
}

/// Score 0.95 for the candidate mentioning WINNER, 0.1 for the rest.
fn score_winner(req: &Value) -> String {
    let answers: serde_json::Map<String, Value> = req["questions"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(id, q)| {
            let hit = q["instructions"].as_str().unwrap().contains("WINNER");
            let s = if hit { 0.95 } else { 0.1 };
            (id.clone(), json!({ "type": "noul", "noul": s }))
        })
        .collect();
    json!({ "model": "jev-test", "answers": answers }).to_string()
}

fn secret_body(_: &Value) -> String {
    r#"{"error":"SECRET_RESPONSE_BODY"}"#.to_string()
}

/// Isolated env: temp DB/HOME/XDG config with Jev enabled.
struct Env {
    dir: tempfile::TempDir,
    reranker: String,
}

impl Env {
    fn new() -> Self {
        Self::with_reranker("jev:jev-test")
    }

    fn with_reranker(reranker: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Self {
            dir,
            reranker: reranker.to_string(),
        };
        // FTS order is unrelated to Jev's: WINNER is stored in the middle.
        for c in [
            "widget alpha note",
            "widget beta note",
            "widget WINNER private-memory-text",
            "widget gamma note",
            "widget delta note",
        ] {
            let out = env.icm(&["store", "-t", "test", "-c", c], None, None);
            assert!(out.status.success(), "store failed: {out:?}");
        }
        env
    }

    fn icm(&self, args: &[&str], endpoint: Option<&str>, stdin: Option<&str>) -> Output {
        let p = self.dir.path();
        let url = endpoint.unwrap_or("http://127.0.0.1:9/unused");
        std::fs::write(
            p.join("config.toml"),
            format!(
                "[recall]\nreranker = \"{}\"\nreranker_url = \"{url}\"\nreranker_timeout_ms = 3000\n",
                self.reranker
            ),
        )
        .unwrap();
        let mut cmd = Command::new(ICM);
        cmd.arg("--no-embeddings")
            .arg("--db")
            .arg(p.join("m.db"))
            .args(args)
            .env("HOME", p)
            .env("ICM_CONFIG", p.join("config.toml"))
            .env("XDG_DATA_HOME", p.join("data"))
            .env("TYPESAFE_API_KEY", KEY)
            .env_remove("LAYA_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn icm");
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn recall_json(env: &Env, endpoint: &str, limit: &str, engine: &str) -> Output {
    env.icm(
        &[
            "recall", "widget", "-l", limit, "--format", "json", "--engine", engine,
        ],
        Some(endpoint),
        None,
    )
}

#[test]
fn recall_reorders_and_truncates_by_score_on_both_engines() {
    for engine in ["v2", "legacy"] {
        let env = Env::new();
        let (url, seen) = fake_jev(200, score_winner);
        let out = recall_json(&env, &url, "2", engine);
        assert!(out.status.success(), "{engine}: recall failed: {out:?}");

        let results: Value = serde_json::from_slice(&out.stdout).unwrap();
        let results = results.as_array().unwrap();
        assert_eq!(
            results.len(),
            2,
            "{engine}: truncated to limit: {results:?}"
        );
        assert!(
            results[0].to_string().contains("WINNER"),
            "{engine}: {results:?}"
        );

        let seen = seen.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen.auth, format!("Bearer {KEY}"));
        assert_eq!(seen.body["model"], "jev-test");
        assert_eq!(seen.body["state"]["query_excerpt"], "widget");
        // limit 2 -> pool of 6, so all 5 stored memories are candidates.
        assert_eq!(
            seen.body["questions"].as_object().unwrap().len(),
            5,
            "{engine}"
        );
    }
}

/// `laya`: no key needed (none sent) and no model, so the server routes.
#[test]
fn laya_needs_no_key_and_sends_no_model() {
    let env = Env::with_reranker("laya");
    let (url, seen) = fake_jev(200, score_winner);
    let out = recall_json(&env, &url, "2", "v2");
    assert!(out.status.success(), "recall failed: {out:?}");
    let results: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(results[0].to_string().contains("WINNER"), "{results}");

    let seen = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(seen.auth, "", "no bearer without LAYA_API_KEY");
    assert!(seen.body.get("model").is_none(), "{}", seen.body);
}

/// A reranker that fails leaves `icm recall` working: local order, a
/// warning naming the HTTP status, and neither the key nor the response
/// body (which may quote memory text) printed.
#[test]
fn http_error_keeps_local_order_without_leaking_key_or_body() {
    let env = Env::new();
    let (url, _seen) = fake_jev(500, secret_body);
    let out = recall_json(&env, &url, "2", "v2");
    assert!(
        out.status.success(),
        "recall must survive the reranker: {out:?}"
    );
    let results: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(results.as_array().unwrap().len(), 2, "{results}");
    // The fallback is the plain recall, not the head of the wider pool.
    let plain = recall_json(&Env::with_reranker(""), &url, "2", "v2");
    let plain: Value = serde_json::from_slice(&plain.stdout).unwrap();
    let summaries = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|m| m["summary"].to_string())
            .collect()
    };
    assert_eq!(summaries(&results), summaries(&plain));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("HTTP 500"), "{stderr}");
    for leak in [KEY, "SECRET_RESPONSE_BODY"] {
        assert!(!stderr.contains(leak), "leaked {leak}: {stderr}");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains(leak),
            "leaked {leak}"
        );
    }
}

#[test]
fn missing_jev_key_fails_explicitly() {
    let env = Env::new();
    // Writes the config; the call itself has the key.
    let _ = env.icm(&["topics"], None, None);
    let out = Command::new(ICM)
        .args(["--no-embeddings", "--db"])
        .arg(env.path().join("m.db"))
        .args(["recall", "widget"])
        .env("HOME", env.path())
        .env("ICM_CONFIG", env.path().join("config.toml"))
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("TYPESAFE_API_KEY"));
}

#[test]
fn prompt_hook_never_calls_the_reranker() {
    let env = Env::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());

    let input = json!({ "prompt": "widget", "cwd": env.path() }).to_string();
    let out = env.icm(&["hook", "prompt"], Some(&url), Some(&input));
    assert!(out.status.success(), "hook failed: {out:?}");
    assert!(
        listener.accept().is_err(),
        "per-prompt hook must not make network calls"
    );
}

/// With a token budget, the budget cuts the reranked list: the reranker's
/// best memory survives even when it ranked low locally.
#[test]
fn budget_applies_after_reranking() {
    let env = Env::new();
    let (url, _seen) = fake_jev(200, score_winner);
    let out = env.icm(
        &[
            "recall",
            "widget",
            "-l",
            "5",
            "--max-tokens",
            "40",
            "--format",
            "json",
        ],
        Some(&url),
        None,
    );
    assert!(out.status.success(), "recall failed: {out:?}");
    let results: Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = results.as_array().unwrap();
    assert!(!results.is_empty() && results.len() < 5, "{results:?}");
    assert!(results[0].to_string().contains("WINNER"), "{results:?}");
}
