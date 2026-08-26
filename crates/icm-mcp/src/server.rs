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
    extra_instructions: Option<&str>,
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

        if let Some(response) = handle_json_rpc_message(
            msg,
            store,
            embedder,
            compact,
            auto_consolidate,
            &mut calls_since_store,
            extra_instructions,
        ) {
            write_response(&mut stdout, &response)?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn handle_json_rpc_message(
    msg: JsonRpcMessage,
    store: &Store,
    embedder: Option<&dyn Embedder>,
    compact: bool,
    auto_consolidate: AutoConsolidate,
    calls_since_store: &mut u32,
    extra_instructions: Option<&str>,
) -> Option<JsonRpcResponse> {
    let method = msg.method.as_deref().unwrap_or("");
    debug!("MCP request: {method}");

    let id = msg.id?;

    Some(match method {
        "initialize" => handle_initialize(id, extra_instructions),
        "ping" => JsonRpcResponse::ok(id, json!({})),
        "tools/list" => handle_tools_list(id, embedder.is_some()),
        "tools/call" => handle_tools_call(
            id,
            &msg.params,
            store,
            embedder,
            compact,
            auto_consolidate,
            calls_since_store,
        ),
        other => JsonRpcResponse::method_not_found(id, other),
    })
}

fn write_response(stdout: &mut io::Stdout, resp: &JsonRpcResponse) -> anyhow::Result<()> {
    let json = serde_json::to_string(resp)?;
    writeln!(stdout, "{json}")?;
    stdout.flush()?;
    Ok(())
}

/// `extra_instructions` is the operator-configured `[mcp] instructions`
/// value (issue #179 follow-up): previously that config field was parsed
/// but never actually reached the MCP handshake, so setting it in
/// `config.toml` silently did nothing. Appended, not a replacement — the
/// built-in recall/store guidance still applies to every client.
fn handle_initialize(id: Value, extra_instructions: Option<&str>) -> JsonRpcResponse {
    let instructions = match extra_instructions {
        Some(extra) if !extra.trim().is_empty() => {
            format!("{ICM_INSTRUCTIONS}\n\n{}", extra.trim())
        }
        _ => ICM_INSTRUCTIONS.to_string(),
    };
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
            "instructions": instructions
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

    // Nudge: remind the agent to store on every THRESHOLD-th call without a
    // store (10, 20, 30, …) — previously the hint was appended to *every*
    // response past the threshold, a recurring token tax on the client LLM
    // (audit finding).
    if tool_name != "icm_memory_store"
        && *calls_since_store >= STORE_NUDGE_THRESHOLD
        && calls_since_store.is_multiple_of(STORE_NUDGE_THRESHOLD)
    {
        result.append_hint(&format!(
            "\n[ICM: {} tool calls since last store. \
             Consider saving important context with icm_memory_store before it is lost.]",
            calls_since_store
        ));
    }

    JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap_or(json!(null)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #179 follow-up: `[mcp] instructions` in config.toml was parsed
    /// (`config::McpConfig::instructions`) but never actually reached the
    /// MCP `initialize` handshake — setting it silently did nothing. This
    /// locks in that the value now really is appended to the built-in
    /// guidance sent to every client.
    #[test]
    fn initialize_appends_configured_extra_instructions() {
        let resp = handle_initialize(json!(1), Some("Also: this project uses Rust 2021."));
        let instructions = resp.result.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(instructions.contains(ICM_INSTRUCTIONS));
        assert!(instructions.contains("Also: this project uses Rust 2021."));
    }

    #[test]
    fn initialize_omits_extra_instructions_when_unset() {
        let resp = handle_initialize(json!(1), None);
        let instructions = resp.result.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(instructions, ICM_INSTRUCTIONS);
    }

    #[test]
    fn initialize_treats_blank_extra_instructions_as_unset() {
        let resp = handle_initialize(json!(1), Some("   \n  "));
        let instructions = resp.result.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(instructions, ICM_INSTRUCTIONS);
    }
}
