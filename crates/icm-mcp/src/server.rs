use std::io::{self, BufRead, Read, Write};

use serde_json::{Value, json};
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

/// First revision whose results carry the envelope fields (`resultType`,
/// `cacheScope`, `ttlMs`, `_meta` server identity). A client on an older
/// revision must not receive them (issue #432). Revision ids are ISO dates,
/// so string order is chronological order.
const RESULT_ENVELOPE_SINCE: &str = "2026-07-28";

/// Where a 2026-07-28 request declares its version: `params._meta`.
const PROTOCOL_VERSION_META_KEY: &str = "io.modelcontextprotocol/protocolVersion";

/// The supported revision equal to `version`, if icm speaks it.
pub fn supported_protocol_version(version: &str) -> Option<&'static str> {
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|v| *v == version)
}

/// The version a request declares for itself in `params._meta` (spec
/// 2026-07-28: the version is declared per request, not only once at
/// `initialize`).
pub fn declared_protocol_version(params: &Option<Value>) -> Option<&str> {
    params
        .as_ref()?
        .get("_meta")?
        .get(PROTOCOL_VERSION_META_KEY)?
        .as_str()
}

/// The version `initialize` settles on: the client's if icm speaks it,
/// icm's default otherwise.
fn negotiated_protocol_version(params: &Option<Value>) -> &'static str {
    params
        .as_ref()
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .and_then(supported_protocol_version)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION)
}

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
    let mut session_version: Option<&'static str> = None;
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
            &mut session_version,
            extra_instructions,
        ) {
            write_response(&mut stdout, &response)?;
        }
    }

    Ok(())
}

/// `session_version` is the version `initialize` settled on for this
/// connection, `None` before any `initialize`. A request that declares its
/// own version in `_meta` is answered on that one instead; a request that
/// declares nothing, on the session's, else on icm's default.
#[allow(clippy::too_many_arguments)]
pub fn handle_json_rpc_message(
    msg: JsonRpcMessage,
    store: &Store,
    embedder: Option<&dyn Embedder>,
    compact: bool,
    auto_consolidate: AutoConsolidate,
    calls_since_store: &mut u32,
    session_version: &mut Option<&'static str>,
    extra_instructions: Option<&str>,
) -> Option<JsonRpcResponse> {
    let method = msg.method.as_deref().unwrap_or("");
    debug!("MCP request: {method}");

    let id = msg.id?;

    let version = match declared_protocol_version(&msg.params) {
        Some(requested) => match supported_protocol_version(requested) {
            Some(v) => v,
            None => {
                return Some(JsonRpcResponse::unsupported_protocol_version(
                    id,
                    requested,
                    SUPPORTED_PROTOCOL_VERSIONS,
                ));
            }
        },
        None => session_version.unwrap_or(DEFAULT_PROTOCOL_VERSION),
    };
    let envelope = version >= RESULT_ENVELOPE_SINCE;

    Some(match method {
        "initialize" => {
            *session_version = Some(negotiated_protocol_version(&msg.params));
            handle_initialize(id, &msg.params, extra_instructions)
        }
        "server/discover" => handle_discover(id, extra_instructions),
        "ping" => JsonRpcResponse::ok(id, json!({})),
        "tools/list" => handle_tools_list(id, embedder.is_some(), envelope),
        "tools/call" => handle_tools_call(
            id,
            &msg.params,
            store,
            embedder,
            compact,
            auto_consolidate,
            calls_since_store,
            envelope,
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
/// this version, it MUST disconnect." The settled version becomes the
/// session's: later results leave out the fields a revision older than
/// `RESULT_ENVELOPE_SINCE` does not have.
fn handle_initialize(
    id: Value,
    params: &Option<Value>,
    extra_instructions: Option<&str>,
) -> JsonRpcResponse {
    let protocol_version = negotiated_protocol_version(params);

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
/// `envelope` is false for a client on an older revision, which gets the
/// bare list.
fn handle_tools_list(id: Value, has_embedder: bool, envelope: bool) -> JsonRpcResponse {
    let mut result = tools::tool_definitions(has_embedder);
    if !envelope {
        return JsonRpcResponse::ok(id, result);
    }
    if let Some(obj) = result.as_object_mut() {
        obj.insert("resultType".into(), json!("complete"));
        obj.insert("cacheScope".into(), json!("public"));
        obj.insert("ttlMs".into(), json!(STATIC_RESULT_TTL_MS));
        obj.insert("_meta".into(), server_info_meta());
    }
    JsonRpcResponse::ok(id, result)
}

#[allow(clippy::too_many_arguments)]
fn handle_tools_call(
    id: Value,
    params: &Option<Value>,
    store: &Store,
    embedder: Option<&dyn Embedder>,
    compact: bool,
    auto_consolidate: AutoConsolidate,
    calls_since_store: &mut u32,
    envelope: bool,
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
        let hint = format!(
            "[ICM: {} tool calls since last store. \
             Consider saving important context with icm_memory_store before it is lost.]",
            calls_since_store
        );
        // An answer asked for as JSON is parsed by the client: the hint
        // goes in a block of its own, not after the closing bracket.
        if args.get("format").and_then(Value::as_str) == Some("json") {
            result.push_note(&hint);
        } else {
            result.append_hint(&format!("\n{hint}"));
        }
    }

    // `resultType` (issue #432): `CallToolResult` requires it in the
    // 2026-07-28 revision, unlike the CacheableResult trio on discover/list
    // results above — a tool call's outcome isn't cacheable, it just needs
    // this one field so the client knows how to parse the result.
    let mut value = serde_json::to_value(result).unwrap_or(json!(null));
    if envelope && let Some(obj) = value.as_object_mut() {
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
        let result = handle_tools_list(json!(1), false, true).result.unwrap();

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

    /// One `tools/call` through the server, as the 10th call since the last
    /// store: the one that carries the reminder to store.
    fn nudged_call(store: &Store, name: &str, arguments: Value) -> Value {
        let msg: JsonRpcMessage = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }))
        .unwrap();
        let mut calls_since_store = STORE_NUDGE_THRESHOLD - 1;
        let resp = handle_json_rpc_message(
            msg,
            store,
            None,
            true,
            AutoConsolidate::default(),
            &mut calls_since_store,
            &mut None,
            None,
        )
        .expect("a response");
        assert_eq!(calls_since_store, STORE_NUDGE_THRESHOLD);
        resp.result.expect("a result")
    }

    fn store_with_two_linked_memories() -> (Store, String) {
        use icm_core::{Importance, Memory, MemoryStore};
        let store = Store::in_memory().unwrap();
        let mut a = Memory::new(
            "context-alpha".into(),
            "Alpha stores sessions in Redis".into(),
            Importance::High,
        );
        let b = Memory::new(
            "context-alpha".into(),
            "Redis runs with append-only persistence".into(),
            Importance::High,
        );
        a.related_ids = vec![b.id.clone()];
        let id = a.id.clone();
        store.store(a).unwrap();
        store.store(b).unwrap();
        (store, id)
    }

    /// The reminder used to be appended to the answer itself. After a JSON
    /// answer that gave `[...]` followed by a line of text: every 10th
    /// call, a client parsing the result failed on trailing data.
    #[test]
    fn the_store_reminder_does_not_break_a_json_answer() {
        let (store, id) = store_with_two_linked_memories();
        for (name, arguments) in [
            (
                "icm_memory_recall",
                json!({"query": "sessions Redis", "project": "alpha", "format": "json"}),
            ),
            (
                "icm_memory_related",
                json!({"id": id, "project": "alpha", "format": "json"}),
            ),
        ] {
            let result = nudged_call(&store, name, arguments);
            let content = result["content"].as_array().unwrap();
            assert_eq!(content.len(), 2, "{name}: {result}");
            let records: Value = serde_json::from_str(content[0]["text"].as_str().unwrap())
                .unwrap_or_else(|e| panic!("{name}: the answer is not JSON any more ({e})"));
            assert!(!records.as_array().unwrap().is_empty(), "{name}");
            let reminder = content[1]["text"].as_str().unwrap();
            assert!(reminder.starts_with("[ICM: 10 tool calls since last store."));
        }
    }

    /// A text answer keeps the reminder where it was: at the end of the
    /// same block.
    #[test]
    fn the_store_reminder_still_ends_a_text_answer() {
        let (store, _) = store_with_two_linked_memories();
        let result = nudged_call(
            &store,
            "icm_memory_recall",
            json!({"query": "sessions Redis", "project": "alpha"}),
        );
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.starts_with("[context-alpha] "), "{text}");
        assert!(
            text.contains("\n[ICM: 10 tool calls since last store."),
            "{text}"
        );
    }

    /// One request through the server, on the connection whose
    /// `initialize` settled on `session`. `version` goes in `params._meta`
    /// when given, the way a 2026-07-28 client declares it.
    fn rpc(
        session: &mut Option<&'static str>,
        method: &str,
        params: Value,
        version: Option<&str>,
    ) -> JsonRpcResponse {
        let mut params = params;
        if let Some(v) = version {
            params["_meta"] = json!({
                PROTOCOL_VERSION_META_KEY: v,
                "io.modelcontextprotocol/clientCapabilities": {}
            });
        }
        let msg: JsonRpcMessage = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": method,
            "params": params
        }))
        .unwrap();
        let store = Store::in_memory().unwrap();
        handle_json_rpc_message(
            msg,
            &store,
            None,
            true,
            AutoConsolidate::default(),
            &mut 0,
            session,
            None,
        )
        .expect("a response")
    }

    const ENVELOPE_FIELDS: [&str; 4] = ["resultType", "cacheScope", "ttlMs", "_meta"];

    /// Issue #432: a version icm does not speak, declared in `_meta`, is
    /// refused with `UnsupportedProtocolVersion` naming the versions to
    /// retry with — the ones `server/discover` advertises. It used to be
    /// ignored and the request served anyway.
    #[test]
    fn an_unsupported_declared_version_is_refused_with_the_supported_list() {
        for method in ["tools/list", "tools/call", "server/discover", "ping"] {
            let resp = rpc(
                &mut None,
                method,
                json!({"name": "icm_memory_stats", "arguments": {}}),
                Some("1999-01-01"),
            );
            assert!(resp.result.is_none(), "{method}");
            let error = resp.error.expect("an error");
            assert_eq!(error.code, -32022, "{method}");
            let data = error.data.expect("error data");
            assert_eq!(data["requested"], "1999-01-01");
            assert_eq!(data["supported"], json!(SUPPORTED_PROTOCOL_VERSIONS));
        }
    }

    /// Issue #432: a 2025-11-25 client must not receive the fields
    /// 2026-07-28 added to results, whether it declares its version per
    /// request or settled on it at `initialize`.
    #[test]
    fn a_client_on_the_older_revision_gets_no_envelope_fields() {
        let mut declared = None;
        let mut negotiated = None;
        rpc(
            &mut negotiated,
            "initialize",
            json!({"protocolVersion": "2025-11-25"}),
            None,
        );
        assert_eq!(negotiated, Some("2025-11-25"));

        for (session, version) in [(&mut declared, Some("2025-11-25")), (&mut negotiated, None)] {
            let list = rpc(session, "tools/list", json!({}), version)
                .result
                .unwrap();
            assert!(list["tools"].is_array());
            let call = rpc(
                session,
                "tools/call",
                json!({"name": "icm_memory_stats", "arguments": {}}),
                version,
            )
            .result
            .unwrap();
            assert!(call["content"].is_array());
            for field in ENVELOPE_FIELDS {
                assert!(
                    list.get(field).is_none(),
                    "tools/list leaks {field}: {list}"
                );
                assert!(
                    call.get(field).is_none(),
                    "tools/call leaks {field}: {call}"
                );
            }
        }
    }

    /// The same requests on 2026-07-28 — declared, negotiated, or not
    /// stated at all (icm's default) — keep the envelope.
    #[test]
    fn the_latest_revision_and_the_default_keep_the_envelope() {
        let mut negotiated = None;
        rpc(
            &mut negotiated,
            "initialize",
            json!({"protocolVersion": "2026-07-28"}),
            None,
        );
        for (mut session, version) in [(None, Some("2026-07-28")), (negotiated, None), (None, None)]
        {
            let list = rpc(&mut session, "tools/list", json!({}), version)
                .result
                .unwrap();
            for field in ENVELOPE_FIELDS {
                assert!(list.get(field).is_some(), "tools/list lacks {field}");
            }
            let call = rpc(
                &mut session,
                "tools/call",
                json!({"name": "icm_memory_stats", "arguments": {}}),
                version,
            )
            .result
            .unwrap();
            assert_eq!(call["resultType"], "complete");
        }
    }

    /// A version declared in `_meta` applies to that request only: it
    /// wins over the session's, and does not change it.
    #[test]
    fn a_declared_version_overrides_the_session_for_one_request() {
        let mut session = Some("2025-11-25");
        let list = rpc(&mut session, "tools/list", json!({}), Some("2026-07-28"))
            .result
            .unwrap();
        assert_eq!(list["resultType"], "complete");
        assert_eq!(session, Some("2025-11-25"));
        let list = rpc(&mut session, "tools/list", json!({}), None)
            .result
            .unwrap();
        assert!(list.get("resultType").is_none());
    }

    /// `server/discover` carries its envelope on every revision: it is
    /// how a client learns which versions to use in the first place.
    #[test]
    fn discover_keeps_its_envelope_for_an_older_client() {
        let result = rpc(&mut None, "server/discover", json!({}), Some("2025-11-25"))
            .result
            .unwrap();
        assert_eq!(result["resultType"], "complete");
        assert!(result["supportedVersions"].is_array());
    }
}
