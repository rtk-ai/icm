use std::io::{self, BufRead, Write};

use serde_json::{json, Value};
use tracing::{debug, error};

use icm_core::Embedder;
use icm_store::SqliteStore;

use crate::protocol::{JsonRpcMessage, JsonRpcResponse};
use crate::tools;

const SERVER_NAME: &str = "icm";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server on stdio. Blocks until stdin is closed.
pub fn run_server(store: &SqliteStore, embedder: Option<&dyn Embedder>) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                error!("stdin read error: {e}");
                break;
            }
        };

        let line = line.trim();
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
            "tools/call" => handle_tools_call(id, &msg.params, store, embedder),
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
RECALL (icm_recall): At the start of a task, search for relevant past context — decisions, \
resolved errors, user preferences. Search only what is relevant, do not dump everything.\n\
\n\
STORE (icm_store): Automatically store important information:\n\
- Architecture decisions → topic: \"decisions-{project}\"\n\
- Resolved errors with solutions → topic: \"errors-resolved\"\n\
- User preferences discovered in session → topic: \"preferences\"\n\
- Project context after significant work → topic: \"context-{project}\"\n\
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
    store: &SqliteStore,
    embedder: Option<&dyn Embedder>,
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

    let result = tools::call_tool(store, embedder, tool_name, &args);
    JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap_or(json!(null)))
}
