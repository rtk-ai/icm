use std::io::{self, BufRead, Read, Write};

use serde_json::{json, Value};
use tracing::{debug, error};

use icm_core::Embedder;
use icm_store::Store;

use crate::protocol::{JsonRpcMessage, JsonRpcResponse};
use crate::tools::{self, AutoConsolidate};

const SERVER_NAME: &str = "icm";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// MCP protocol revisions icm can speak, newest first. `initialize` and
/// `server/discover` both derive their answers from this single list so the
/// two methods can never advertise different windows (issues #432, #433:
/// `initialize` used to ignore the client's requested version entirely and
/// always echo a hardcoded, unadvertised `"2024-11-05"` — outside the window
/// icm itself claims to support).
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2026-07-28", "2025-11-25"];

/// icm's own preferred revision — what it answers with when the client's
/// requested version isn't one icm supports (or no version was requested at
/// all). Per the spec's own `InitializeResult.protocolVersion` doc: "This
/// may not match the version that the client requested. If the client
/// cannot support this version, it MUST disconnect." A server is free to
/// answer with any version *it* actually speaks — it must never echo back a
/// version outside its own advertised list.
const DEFAULT_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];

/// Cache-freshness hint (`ttlMs`, HTTP `max-age` analogue) shared by every
/// `CacheableResult`-envelope response icm sends (`server/discover`,
/// `tools/list`). All of them describe build-time-fixed data — icm's
/// capabilities, supported-version window, and tool catalog cannot change
/// for the life of the process — so a long, constant TTL is honest: the
/// response is not merely *allowed* to be cached this long, it genuinely
/// never changes within a run. No server-side caching state is needed to
/// honor this; returning the same deterministic content on every call
/// already satisfies "stable across calls within its own TTL".
const STATIC_RESULT_TTL_MS: u64 = 3_600_000; // 1 hour

/// `_meta["io.modelcontextprotocol/serverInfo"]` block every
/// `CacheableResult`-envelope response includes (issue #432: "results
/// identify the server in _meta") — the same identity `initialize` and
/// `server/discover` already report in their own `serverInfo`/`_meta`.
fn server_info_meta() -> Value {
    json!({
        "io.modelcontextprotocol/serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        }
    })
}

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
        "initialize" => handle_initialize(id, &msg.params, extra_instructions),
        "server/discover" => handle_discover(id, extra_instructions),
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
///
/// `params` is the raw `initialize` request body (issues #432/#433): icm
/// used to ignore the client's requested `protocolVersion` entirely and
/// always echo a hardcoded `"2024-11-05"` — a revision outside the window
/// icm itself now advertises via `server/discover`. Negotiation here is
/// intentionally the simple, pre-2026-07-28 handshake style (one version,
/// once, at `initialize`): if the client asked for a revision icm actually
/// speaks, echo it back; otherwise answer with icm's own default. Per the
/// spec's own `InitializeResult.protocolVersion` doc, a server is free to
/// answer with any version *it* supports — "if the client cannot support
/// this version, it MUST disconnect." icm's request handling doesn't
/// actually branch on protocol version anywhere, so this is honest: every
/// version in `SUPPORTED_PROTOCOL_VERSIONS` gets identical behavior.
fn handle_initialize(
    id: Value,
    params: &Option<Value>,
    extra_instructions: Option<&str>,
) -> JsonRpcResponse {
    let requested_version = params
        .as_ref()
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str());
    let protocol_version = match requested_version {
        Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
        _ => DEFAULT_PROTOCOL_VERSION,
    };

    let instructions = match extra_instructions {
        Some(extra) if !extra.trim().is_empty() => {
            format!("{ICM_INSTRUCTIONS}\n\n{}", extra.trim())
        }
        _ => ICM_INSTRUCTIONS.to_string(),
    };
    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": protocol_version,
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

/// `server/discover` (spec revision 2026-07-28, issue #432): lets a client
/// learn icm's supported protocol versions and capabilities without opening
/// a session first. Response shape verified against the real
/// `@hasmcp/mcp-spec-test` package's vendored `DiscoverResult` schema
/// (`spec/2026-07-28/schema.json`) — required fields are `cacheScope`,
/// `capabilities`, `resultType`, `supportedVersions`, `ttlMs`; server
/// identity goes in `_meta["io.modelcontextprotocol/serverInfo"]`
/// (`Implementation`), not a top-level `serverInfo` — `DiscoverResult` has
/// no such property.
///
/// icm's capabilities and supported-version window are fixed at build time
/// and never change within a run, so returning the same deterministic
/// content on every call already satisfies "stable across calls within its
/// own TTL" — no server-side caching state is needed to honor `ttlMs`.
fn handle_discover(id: Value, extra_instructions: Option<&str>) -> JsonRpcResponse {
    let instructions = match extra_instructions {
        Some(extra) if !extra.trim().is_empty() => {
            format!("{ICM_INSTRUCTIONS}\n\n{}", extra.trim())
        }
        _ => ICM_INSTRUCTIONS.to_string(),
    };
    JsonRpcResponse::ok(
        id,
        json!({
            "resultType": "complete",
            "cacheScope": "public",
            "ttlMs": STATIC_RESULT_TTL_MS,
            "supportedVersions": SUPPORTED_PROTOCOL_VERSIONS,
            "capabilities": {
                "tools": {}
            },
            "instructions": instructions,
            "_meta": server_info_meta()
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
Importance levels: critical (never forgotten), high (slow decay), medium (normal), low (fast decay).\n\
\n\
MEMOIR (icm_memoir_create / icm_memoir_add_concept / icm_memoir_refine): a separate, permanent \
knowledge layer — unlike memory_store, concepts here never decay or get pruned. Reach for it, not \
memory_store, for durable foundational knowledge a project will keep relying on indefinitely: \
canonical architecture decisions, stable domain/API definitions, core conventions — not day-to-day \
context, which belongs in memory_store and is expected to fade. Create one memoir per project, then \
icm_memoir_add_concept as durable facts emerge; use icm_memoir_refine to update an existing concept \
rather than adding a duplicate.";

/// `ListToolsResult` needs the same `CacheableResult` envelope as
/// `server/discover` (issue #432: "cacheable list results carry the
/// schema-required cache hints" / "results identify the server in
/// _meta") — the tool catalog is fixed for the life of the process (it only
/// varies with `has_embedder`, a build/startup-time property, never per
/// request), so it's exactly as static as `discover`'s own content.
fn handle_tools_list(id: Value, has_embedder: bool) -> JsonRpcResponse {
    let mut result = tools::tool_definitions(has_embedder);
    if let Some(obj) = result.as_object_mut() {
        obj.insert("resultType".into(), json!("complete"));
        obj.insert("cacheScope".into(), json!("public"));
        obj.insert("ttlMs".into(), json!(STATIC_RESULT_TTL_MS));
        obj.insert("_meta".into(), server_info_meta());
    }
    JsonRpcResponse::ok(id, result)
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

    // `resultType` (issue #432): `CallToolResult` requires it in the
    // 2026-07-28 revision, unlike the CacheableResult trio on discover/list
    // results above — a tool call's outcome isn't cacheable, it just needs
    // this one field so the client knows how to parse the result.
    let mut value = serde_json::to_value(result).unwrap_or(json!(null));
    if let Some(obj) = value.as_object_mut() {
        obj.insert("resultType".into(), json!("complete"));
    }
    JsonRpcResponse::ok(id, value)
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
        let resp = handle_initialize(json!(1), &None, Some("Also: this project uses Rust 2021."));
        let instructions = resp.result.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(instructions.contains(ICM_INSTRUCTIONS));
        assert!(instructions.contains("Also: this project uses Rust 2021."));
    }

    #[test]
    fn initialize_omits_extra_instructions_when_unset() {
        let resp = handle_initialize(json!(1), &None, None);
        let instructions = resp.result.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(instructions, ICM_INSTRUCTIONS);
    }

    #[test]
    fn initialize_treats_blank_extra_instructions_as_unset() {
        let resp = handle_initialize(json!(1), &None, Some("   \n  "));
        let instructions = resp.result.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(instructions, ICM_INSTRUCTIONS);
    }

    /// Issues #432/#433: icm used to ignore the client's requested
    /// `protocolVersion` entirely and always echo a hardcoded, unadvertised
    /// `"2024-11-05"`. When the client asks for a revision icm actually
    /// supports, echo it back rather than silently substituting a different
    /// one.
    #[test]
    fn initialize_echoes_a_supported_requested_version() {
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let params = Some(json!({"protocolVersion": v}));
            let resp = handle_initialize(json!(1), &params, None);
            assert_eq!(
                resp.result.unwrap()["protocolVersion"].as_str(),
                Some(*v),
                "requesting {v} should be echoed back"
            );
        }
    }

    /// An unsupported/unrecognized request (or none at all — a client that
    /// omits `protocolVersion`, or sends the old hardcoded value this server
    /// used to always answer with) falls back to icm's own default, which
    /// must itself be one of the versions `server/discover` advertises —
    /// never an out-of-window value like the old "2024-11-05".
    #[test]
    fn initialize_falls_back_to_default_for_unsupported_or_missing_version() {
        for params in [
            None,
            Some(json!({"protocolVersion": "2024-11-05"})),
            Some(json!({"protocolVersion": "1999-01-01"})),
            Some(json!({})),
        ] {
            let resp = handle_initialize(json!(1), &params, None);
            let negotiated = resp.result.unwrap()["protocolVersion"]
                .as_str()
                .unwrap()
                .to_string();
            assert_eq!(negotiated, DEFAULT_PROTOCOL_VERSION);
            assert!(SUPPORTED_PROTOCOL_VERSIONS.contains(&negotiated.as_str()));
        }
    }

    /// `server/discover`'s response shape verified against the real
    /// `@hasmcp/mcp-spec-test` package's vendored schema (issue #432):
    /// `DiscoverResult` requires `cacheScope`, `capabilities`, `resultType`,
    /// `supportedVersions`, `ttlMs`; server identity lives under
    /// `_meta["io.modelcontextprotocol/serverInfo"]`, not a top-level
    /// `serverInfo` field (`DiscoverResult` has no such property).
    #[test]
    fn discover_response_has_every_schema_required_field() {
        let resp = handle_discover(json!(1), None);
        let result = resp.result.unwrap();

        assert!(result["resultType"].is_string());
        assert!(matches!(
            result["cacheScope"].as_str(),
            Some("public") | Some("private")
        ));
        let ttl = result["ttlMs"].as_i64().expect("ttlMs must be a number");
        assert!(ttl >= 0);

        let versions = result["supportedVersions"]
            .as_array()
            .expect("supportedVersions must be an array");
        assert!(!versions.is_empty());
        for v in versions {
            let v = v.as_str().unwrap();
            assert_eq!(v.len(), 10, "{v} is not a YYYY-MM-DD revision date");
        }

        assert!(result["capabilities"].is_object());

        let server_info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
        assert!(
            server_info["name"].as_str().is_some(),
            "expected _meta[\"io.modelcontextprotocol/serverInfo\"].name, got {:?}",
            result["_meta"]
        );
    }

    /// The suite's `server/discover advertises a revision this suite
    /// supports` check needs the negotiated default itself to be one of
    /// the advertised versions, or `initialize` and `discover` disagree
    /// about what icm actually speaks.
    #[test]
    fn discover_advertises_the_negotiated_default_version() {
        let result = handle_discover(json!(1), None).result.unwrap();
        let versions: Vec<&str> = result["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(versions.contains(&DEFAULT_PROTOCOL_VERSION));
    }

    /// "stable across calls within its own TTL" — two immediate calls must
    /// return identical `supportedVersions` (no caching state needed since
    /// icm's capabilities are fixed at build time, but this locks that in).
    #[test]
    fn discover_is_stable_across_repeated_calls() {
        let a = handle_discover(json!(1), None).result.unwrap();
        let b = handle_discover(json!(2), None).result.unwrap();
        assert_eq!(a["supportedVersions"], b["supportedVersions"]);
    }

    /// `tools/list` needs the same `CacheableResult` envelope as
    /// `server/discover` (issue #432) — verified against the real
    /// `ListToolsResult` schema, which requires `cacheScope`, `resultType`,
    /// `tools`, `ttlMs`, plus the same `_meta` server-identity block.
    #[test]
    fn tools_list_response_has_the_cacheable_envelope() {
        let result = handle_tools_list(json!(1), false).result.unwrap();

        assert!(result["tools"].is_array(), "must still return tools");
        assert!(result["resultType"].is_string());
        assert!(matches!(
            result["cacheScope"].as_str(),
            Some("public") | Some("private")
        ));
        assert!(result["ttlMs"].as_i64().unwrap() >= 0);
        assert!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"]
                .as_str()
                .is_some(),
            "expected _meta[\"io.modelcontextprotocol/serverInfo\"].name, got {:?}",
            result["_meta"]
        );
    }
}
