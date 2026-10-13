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

use std::io::{BufRead, BufReader, Write};
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

#[test]
fn stdio_proxy_forwards_tools_call_to_http_daemon() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &["--token", "proxy-secret"]);

    let mut proxy = Command::new(ICM)
        .arg("--no-embeddings")
        .arg("serve")
        .arg("--http-proxy")
        .arg(format!("http://{}", server.addr))
        .arg("--token")
        .arg("proxy-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn icm serve --http-proxy");

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "icm_memory_store",
            "arguments": {
                "topic": "proxy-test",
                "content": "stored through stdio proxy"
            }
        }
    });
    {
        let mut stdin = proxy.stdin.take().expect("proxy stdin");
        writeln!(stdin, "{request}").expect("write JSON-RPC request");
    }

    let output = proxy.wait_with_output().expect("wait for proxy");
    assert!(
        output.status.success(),
        "proxy failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("proxy emitted JSON-RPC");
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert!(response.get("result").is_some(), "response: {response}");

    let recall = ureq::post(&format!("http://{}/recall?format=json", server.addr))
        .timeout(Duration::from_secs(5))
        .set("authorization", "Bearer proxy-secret")
        .set("content-type", "application/json")
        .send_string(r#"{"query":"stdio proxy","topic":"proxy-test"}"#)
        .expect("authenticated recall");
    let memories: serde_json::Value =
        serde_json::from_str(&recall.into_string().unwrap()).expect("recall JSON");
    assert!(
        memories.as_array().is_some_and(|items| items
            .iter()
            .any(|item| item["summary"] == "stored through stdio proxy")),
        "proxied store did not reach daemon DB: {memories}"
    );
}

/// Issue #432, over Streamable HTTP: a request declaring a protocol
/// version icm does not speak gets `UnsupportedProtocolVersion` (-32022)
/// with a 400, and a request on the older revision, named by the
/// `MCP-Protocol-Version` header, gets results without the 2026-07-28
/// envelope fields. The stdio proxy passes the -32022 answer on.
#[test]
fn mcp_endpoint_follows_the_requested_protocol_version() {
    let (_dir, db) = temp_db();
    let server = spawn_server(&db, &[]);
    let endpoint = format!("http://{}/mcp", server.addr);

    let unsupported = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "1999-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}
    });
    let resp = ureq::post(&endpoint)
        .timeout(Duration::from_secs(5))
        .set("content-type", "application/json")
        .send_string(&unsupported.to_string());
    let body = match resp {
        Err(ureq::Error::Status(400, r)) => r.into_string().unwrap(),
        other => panic!("expected 400, got {other:?}"),
    };
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["error"]["code"], -32022, "{body}");
    assert_eq!(body["error"]["data"]["requested"], "1999-01-01");

    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    let older = ureq::post(&endpoint)
        .timeout(Duration::from_secs(5))
        .set("content-type", "application/json")
        .set("mcp-protocol-version", "2025-11-25")
        .send_string(list)
        .expect("tools/list on 2025-11-25");
    assert_eq!(older.header("mcp-protocol-version"), Some("2025-11-25"));
    let older: serde_json::Value = older.into_json().unwrap();
    assert!(older["result"]["tools"].is_array(), "{older}");
    assert!(older["result"].get("resultType").is_none(), "{older}");
    assert!(older["result"].get("_meta").is_none(), "{older}");

    let latest = post_json(&server.addr, "/mcp", list);
    // Version-less: answered on icm's default, without naming it.
    assert_eq!(latest.header("mcp-protocol-version"), None);
    let latest: serde_json::Value = latest.into_json().unwrap();
    assert_eq!(latest["result"]["resultType"], "complete", "{latest}");

    let mut mismatch = unsupported.clone();
    mismatch["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        serde_json::json!("2026-07-28");
    let resp = ureq::post(&endpoint)
        .timeout(Duration::from_secs(5))
        .set("content-type", "application/json")
        .set("mcp-protocol-version", "2025-11-25")
        .send_string(&mismatch.to_string());
    match resp {
        Err(ureq::Error::Status(400, r)) => {
            let body: serde_json::Value = r.into_json().unwrap();
            assert_eq!(body["error"]["code"], -32020, "{body}");
        }
        other => panic!("expected 400 on a header mismatch, got {other:?}"),
    }

    let mut proxy = Command::new(ICM)
        .arg("--no-embeddings")
        .arg("serve")
        .arg("--http-proxy")
        .arg(format!("http://{}", server.addr))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn icm serve --http-proxy");
    {
        let mut stdin = proxy.stdin.take().expect("proxy stdin");
        writeln!(stdin, "{unsupported}").expect("write JSON-RPC request");
    }
    let output = proxy.wait_with_output().expect("wait for proxy");
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("proxy output is not JSON-RPC ({e}): {output:?}"));
    assert_eq!(response["id"], 1);
    assert_eq!(response["error"]["code"], -32022, "{response}");
}
