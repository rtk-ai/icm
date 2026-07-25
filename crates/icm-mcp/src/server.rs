use std::io::{self, BufRead, Read, Write};

use serde_json::{json, Value};
use tracing::{debug, error};

use icm_core::Embedder;
use icm_store::Store;

use crate::protocol::{JsonRpcMessage, JsonRpcResponse};
use crate::tools::{self, AutoConsolidate};

const SERVER_NAME: &str = "icm";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Number of non-store tool calls before we nudge the agent to store.
const STORE_NUDGE_THRESHOLD: u32 = 10;

/// Maximum allowed line length (10 MB). The cap is enforced *while reading*
/// (bounded `take` + `read_until`), so an oversized line is never fully
/// buffered — previously the whole line was allocated by `lines()` before
/// the length check ran, defeating the cap (audit finding; same class of
/// bug as the CLI hook-stdin fix in e551c27).
const MAX_LINE_LEN: usize = 10 * 1024 * 1024;

/// Read one `\n`-terminated line into `buf` without ever buffering more than
/// `MAX_LINE_LEN + 1` bytes of it. Returns `Ok(None)` on EOF, `Ok(Some(true))`
/// for a within-limit line, `Ok(Some(false))` for an oversized line (whose
/// remainder has been drained and discarded in bounded chunks).
fn read_capped_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> io::Result<Option<bool>> {
    buf.clear();
    let n = reader
        .take(MAX_LINE_LEN as u64 + 1)
        .read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(None); // EOF
    }
    // Oversized iff we exhausted the read budget without hitting the newline.
    if buf.last() != Some(&b'\n') && n == MAX_LINE_LEN + 1 {
        // Drain the rest of the line in bounded chunks so the next read
        // starts on a fresh line.
        let mut scratch = Vec::with_capacity(64 * 1024);
        loop {
            scratch.clear();
            let m = reader.take(1024 * 1024).read_until(b'\n', &mut scratch)?;
            if m == 0 || scratch.last() == Some(&b'\n') {
                break;
            }
        }
        return Ok(Some(false));
    }
    Ok(Some(true))
}

/// Run the MCP server on stdio. Blocks until stdin is closed.
pub fn run_server(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    compact: bool,
    auto_consolidate: AutoConsolidate,
) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut stdout = io::stdout();
    let mut calls_since_store: u32 = 0;
    let mut buf: Vec<u8> = Vec::new();

    loop {
        let within_limit = match read_capped_line(&mut reader, &mut buf) {
            Ok(Some(ok)) => ok,
            Ok(None) => break, // EOF
            Err(e) => {
                error!("stdin read error: {e}");
                break;
            }
        };

        if !within_limit {
            error!("line too long (max {MAX_LINE_LEN} bytes)");
            let resp = JsonRpcResponse::err(
                Value::Null,
                -32600,
                format!("line too long (max {MAX_LINE_LEN} bytes)"),
            );
            write_response(&mut stdout, &resp)?;
            continue;
        }

        let line_owned = String::from_utf8_lossy(&buf);
        let line = line_owned.trim();
        if line.is_empty() {
            continue;
        }

        let msg: JsonRpcMessage = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(e) => {
                error!("invalid JSON-RPC: {e}");
                // Send parse error if we can
                let resp = JsonRpcResponse::err(Value::Null, -32700, format!("parse error: {e}"));
                write_response(&mut stdout, &resp)?;
                continue;
            }
        };

        let method = msg.method.as_deref().unwrap_or("");
        debug!("MCP request: {method}");

        // Notifications have no id — don't respond
        let id = match msg.id {
            Some(id) => id,
            None => continue,
        };

        let response = match method {
            "initialize" => handle_initialize(id),
            "ping" => JsonRpcResponse::ok(id, json!({})),
            "tools/list" => handle_tools_list(id, embedder.is_some()),
            "tools/call" => handle_tools_call(
                id,
                &msg.params,
                store,
                embedder,
                compact,
                auto_consolidate,
                &mut calls_since_store,
            ),
            other => JsonRpcResponse::method_not_found(id, other),
        };

        write_response(&mut stdout, &response)?;
    }

    Ok(())
}

fn write_response(stdout: &mut io::Stdout, resp: &JsonRpcResponse) -> anyhow::Result<()> {
    let json = serde_json::to_string(resp)?;
    writeln!(stdout, "{json}")?;
    stdout.flush()?;
    Ok(())
}

fn handle_initialize(id: Value) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            },
            "instructions": ICM_INSTRUCTIONS
        }),
    )
}

const ICM_INSTRUCTIONS: &str = "\
Use ICM (Infinite Context Memory) proactively to maintain long-term memory across sessions.\n\
\n\
RECALL (icm_memory_recall): At the start of a task, search for relevant past context — decisions, \
resolved errors, user preferences. Search only what is relevant, do not dump everything.\n\
\n\
STORE (icm_memory_store): You MUST store when ANY of these triggers occur:\n\
1. Error resolved → topic: \"errors-resolved\", importance: high\n\
2. Architecture/design decision made → topic: \"decisions-{project}\", importance: high\n\
3. User preference discovered (correction, feedback) → topic: \"preferences\", importance: critical\n\
4. Significant task completed (feature, fix, config, review) → topic: \"context-{project}\", importance: high\n\
5. Conversation exceeds ~20 tool calls without a store → store a progress summary\n\
\n\
Do this BEFORE responding to the user. Not after. Not later. Immediately.\n\
\n\
Do NOT store: trivial details, information already in CLAUDE.md, ephemeral state.\n\
\n\
Importance levels: critical (never forgotten), high (slow decay), medium (normal), low (fast decay).";

fn handle_tools_list(id: Value, has_embedder: bool) -> JsonRpcResponse {
    JsonRpcResponse::ok(id, tools::tool_definitions(has_embedder))
}

fn handle_tools_call(
    id: Value,
    params: &Option<Value>,
    store: &Store,
    embedder: Option<&dyn Embedder>,
    compact: bool,
    auto_consolidate: AutoConsolidate,
    calls_since_store: &mut u32,
) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::err(id, -32602, "missing params".into());
        }
    };

    let tool_name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return JsonRpcResponse::err(id, -32602, "missing tool name".into());
        }
    };

    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Track store calls to nudge the agent
    if tool_name == "icm_memory_store" {
        *calls_since_store = 0;
    } else {
        *calls_since_store += 1;
    }

    let mut result =
        tools::call_tool_with_config(store, embedder, tool_name, &args, compact, auto_consolidate);

    // Nudge: append a store reminder if too many calls without storing
    if *calls_since_store >= STORE_NUDGE_THRESHOLD && tool_name != "icm_memory_store" {
        result.append_hint(&format!(
            "\n[ICM: {} tool calls since last store. \
             Consider saving important context with icm_memory_store before it is lost.]",
            calls_since_store
        ));
    }

    JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap_or(json!(null)))
}
