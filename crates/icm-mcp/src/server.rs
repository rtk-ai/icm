use std::io::{self, BufRead, Read, Write};

use serde_json::{json, Value};
use tracing::{debug, error};

use icm_core::Embedder;
use icm_store::Store;

use crate::protocol::{JsonRpcMessage, JsonRpcResponse};
use crate::tools::{self, AutoConsolidate, ToolDefinitionOptions};

const SERVER_NAME: &str = "icm";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const LEGACY_PROTOCOL_VERSION: &str = "2024-11-05";
const PREVIOUS_PROTOCOL_VERSION: &str = "2025-06-18";
const LATEST_LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] = [
    MODERN_PROTOCOL_VERSION,
    LATEST_LEGACY_PROTOCOL_VERSION,
    PREVIOUS_PROTOCOL_VERSION,
    LEGACY_PROTOCOL_VERSION,
];
const DISCOVERY_TTL_MS: u64 = 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ConnectionEra {
    Undetermined,
    Legacy(&'static str),
    Modern,
}

#[derive(Debug, PartialEq)]
enum RequestMetadata {
    Legacy,
    Modern(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DispatchProtocol {
    Legacy(&'static str),
    Modern,
}

#[derive(Debug, PartialEq)]
enum ProtocolSelectionError {
    Invalid(&'static str),
    Unsupported {
        requested: String,
        supported: Vec<&'static str>,
    },
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
) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut stdout = io::stdout();
    let mut calls_since_store: u32 = 0;
    let mut connection_era = ConnectionEra::Undetermined;
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

        let protocol = match select_request_protocol(&mut connection_era, method, &msg.params) {
            Ok(protocol) => protocol,
            Err(error) => {
                write_response(&mut stdout, &protocol_error_response(id, error))?;
                continue;
            }
        };

        let response = match protocol {
            DispatchProtocol::Modern => {
                let mut response = match method {
                    "server/discover" => handle_server_discover(id, embedder.is_some()),
                    "tools/list" => handle_tools_list(
                        id,
                        embedder.is_some(),
                        tool_definition_options(MODERN_PROTOCOL_VERSION),
                    ),
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
                add_modern_result_metadata(&mut response, method);
                response
            }
            DispatchProtocol::Legacy(protocol_version) => match method {
                "initialize" => handle_initialize(id, protocol_version),
                "ping" => JsonRpcResponse::ok(id, json!({})),
                "tools/list" => handle_tools_list(
                    id,
                    embedder.is_some(),
                    tool_definition_options(protocol_version),
                ),
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
            },
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

fn request_metadata(params: &Option<Value>) -> Result<RequestMetadata, &'static str> {
    let Some(meta) = params.as_ref().and_then(|params| params.get("_meta")) else {
        return Ok(RequestMetadata::Legacy);
    };
    let Some(meta) = meta.as_object() else {
        return Err("_meta must be an object");
    };

    let protocol_key = "io.modelcontextprotocol/protocolVersion";
    let capabilities_key = "io.modelcontextprotocol/clientCapabilities";
    let client_info_key = "io.modelcontextprotocol/clientInfo";
    let has_modern_metadata = [protocol_key, capabilities_key, client_info_key]
        .iter()
        .any(|key| meta.contains_key(*key));
    if !has_modern_metadata {
        return Ok(RequestMetadata::Legacy);
    }

    let protocol_version = meta
        .get(protocol_key)
        .and_then(Value::as_str)
        .ok_or("modern requests require a string protocolVersion")?;
    if !meta.get(capabilities_key).is_some_and(Value::is_object) {
        return Err("modern requests require an object clientCapabilities");
    }
    if let Some(client_info) = meta.get(client_info_key) {
        let valid = client_info.as_object().is_some_and(|client_info| {
            client_info.get("name").is_some_and(Value::is_string)
                && client_info.get("version").is_some_and(Value::is_string)
        });
        if !valid {
            return Err("clientInfo must contain string name and version fields");
        }
    }

    Ok(RequestMetadata::Modern(protocol_version.to_owned()))
}

fn select_request_protocol(
    connection_era: &mut ConnectionEra,
    method: &str,
    params: &Option<Value>,
) -> Result<DispatchProtocol, ProtocolSelectionError> {
    let metadata = request_metadata(params).map_err(ProtocolSelectionError::Invalid)?;

    match (*connection_era, metadata) {
        (ConnectionEra::Undetermined, RequestMetadata::Legacy) => {
            let version = if method == "initialize" {
                negotiate_legacy_protocol_version(params)
            } else {
                // Preserve ICM's historical support for legacy clients that
                // call a method before initialize.
                LEGACY_PROTOCOL_VERSION
            };
            *connection_era = ConnectionEra::Legacy(version);
            Ok(DispatchProtocol::Legacy(version))
        }
        (ConnectionEra::Undetermined, RequestMetadata::Modern(requested)) => {
            *connection_era = ConnectionEra::Modern;
            select_modern_version(requested)
        }
        (ConnectionEra::Legacy(version), RequestMetadata::Legacy) => {
            Ok(DispatchProtocol::Legacy(version))
        }
        (ConnectionEra::Legacy(version), RequestMetadata::Modern(requested)) => {
            Err(ProtocolSelectionError::Unsupported {
                requested,
                supported: vec![version],
            })
        }
        (ConnectionEra::Modern, RequestMetadata::Modern(requested)) => {
            select_modern_version(requested)
        }
        (ConnectionEra::Modern, RequestMetadata::Legacy) => Err(ProtocolSelectionError::Invalid(
            "modern connections require per-request metadata",
        )),
    }
}

fn select_modern_version(requested: String) -> Result<DispatchProtocol, ProtocolSelectionError> {
    if requested == MODERN_PROTOCOL_VERSION {
        Ok(DispatchProtocol::Modern)
    } else {
        Err(ProtocolSelectionError::Unsupported {
            requested,
            supported: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        })
    }
}

fn negotiate_legacy_protocol_version(params: &Option<Value>) -> &'static str {
    match params
        .as_ref()
        .and_then(|value| value.get("protocolVersion"))
        .and_then(Value::as_str)
    {
        // The MCP lifecycle requires echoing a requested version when the
        // server supports it.
        Some(LEGACY_PROTOCOL_VERSION) => LEGACY_PROTOCOL_VERSION,
        Some(PREVIOUS_PROTOCOL_VERSION) => PREVIOUS_PROTOCOL_VERSION,
        Some(LATEST_LEGACY_PROTOCOL_VERSION) => LATEST_LEGACY_PROTOCOL_VERSION,
        // Keep accepting clients that omit the required field, matching ICM's
        // pre-negotiation behavior rather than breaking them on upgrade.
        None => LEGACY_PROTOCOL_VERSION,
        // Legacy clients cannot fall forward to the stateless 2026 protocol,
        // so offer the newest handshake-based version instead.
        Some(_) => LATEST_LEGACY_PROTOCOL_VERSION,
    }
}

fn tool_definition_options(protocol_version: &str) -> ToolDefinitionOptions {
    if protocol_version == LEGACY_PROTOCOL_VERSION {
        ToolDefinitionOptions::none()
    } else {
        ToolDefinitionOptions::none()
            .with_annotations()
            .with_output_schemas()
    }
}

fn server_info() -> Value {
    json!({
        "name": SERVER_NAME,
        "version": SERVER_VERSION
    })
}

fn server_capabilities() -> Value {
    json!({
        "tools": {}
    })
}

fn handle_initialize(id: Value, protocol_version: &str) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": protocol_version,
            "capabilities": server_capabilities(),
            "serverInfo": server_info(),
            "instructions": ICM_INSTRUCTIONS
        }),
    )
}

fn handle_server_discover(id: Value, has_embedder: bool) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!({
            "supportedVersions": SUPPORTED_PROTOCOL_VERSIONS,
            "capabilities": server_capabilities(),
            "instructions": ICM_INSTRUCTIONS,
            "ttlMs": DISCOVERY_TTL_MS,
            "cacheScope": "private",
            "_meta": {
                "io.modelcontextprotocol/serverInfo": server_info(),
                "io.icm/embeddingsAvailable": has_embedder
            }
        }),
    )
}

fn protocol_error_response(id: Value, error: ProtocolSelectionError) -> JsonRpcResponse {
    match error {
        ProtocolSelectionError::Invalid(message) => {
            JsonRpcResponse::err(id, -32602, message.into())
        }
        ProtocolSelectionError::Unsupported {
            requested,
            supported,
        } => JsonRpcResponse::err_with_data(
            id,
            -32022,
            "Unsupported protocol version".into(),
            json!({
                "supported": supported,
                "requested": requested
            }),
        ),
    }
}

fn add_modern_result_metadata(response: &mut JsonRpcResponse, method: &str) {
    let Some(result) = response.result.as_mut().and_then(Value::as_object_mut) else {
        return;
    };

    result.insert("resultType".into(), json!("complete"));
    let meta = result
        .entry("_meta")
        .or_insert_with(|| json!({}))
        .as_object_mut();
    if let Some(meta) = meta {
        meta.insert("io.modelcontextprotocol/serverInfo".into(), server_info());
    }

    if method == "tools/list" {
        result.insert("ttlMs".into(), json!(DISCOVERY_TTL_MS));
        result.insert("cacheScope".into(), json!("private"));
    }
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

fn handle_tools_list(
    id: Value,
    has_embedder: bool,
    options: ToolDefinitionOptions,
) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        tools::tool_definitions_with_options(has_embedder, options),
    )
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

    fn initialize_params(protocol_version: Option<&str>) -> Option<Value> {
        protocol_version.map(|version| {
            json!({
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            })
        })
    }

    fn modern_params(protocol_version: &str) -> Option<Value> {
        Some(json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": protocol_version,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }
        }))
    }

    #[test]
    fn protocol_negotiation_echoes_supported_versions() {
        for (version, options) in [
            (LEGACY_PROTOCOL_VERSION, ToolDefinitionOptions::none()),
            (
                PREVIOUS_PROTOCOL_VERSION,
                ToolDefinitionOptions::none()
                    .with_annotations()
                    .with_output_schemas(),
            ),
            (
                LATEST_LEGACY_PROTOCOL_VERSION,
                ToolDefinitionOptions::none()
                    .with_annotations()
                    .with_output_schemas(),
            ),
        ] {
            let params = initialize_params(Some(version));
            let negotiated = negotiate_legacy_protocol_version(&params);
            assert_eq!(negotiated, version);
            assert_eq!(tool_definition_options(negotiated), options);

            let response = handle_initialize(json!(1), negotiated);
            assert_eq!(response.result.unwrap()["protocolVersion"], version);
        }
    }

    #[test]
    fn protocol_negotiation_uses_compatible_fallbacks() {
        assert_eq!(
            negotiate_legacy_protocol_version(&None),
            LEGACY_PROTOCOL_VERSION
        );

        let params = initialize_params(Some("2099-01-01"));
        assert_eq!(
            negotiate_legacy_protocol_version(&params),
            LATEST_LEGACY_PROTOCOL_VERSION
        );
    }

    #[test]
    fn modern_request_metadata_is_validated() {
        let params = modern_params(MODERN_PROTOCOL_VERSION);
        assert_eq!(
            request_metadata(&params),
            Ok(RequestMetadata::Modern(MODERN_PROTOCOL_VERSION.into()))
        );
        assert_eq!(request_metadata(&None), Ok(RequestMetadata::Legacy));

        for (params, expected) in [
            (
                Some(json!({
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION
                    }
                })),
                "modern requests require an object clientCapabilities",
            ),
            (
                Some(json!({
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
                        "io.modelcontextprotocol/clientCapabilities": "invalid"
                    }
                })),
                "modern requests require an object clientCapabilities",
            ),
            (
                Some(json!({
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": 20260728,
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }
                })),
                "modern requests require a string protocolVersion",
            ),
            (
                Some(json!({
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
                        "io.modelcontextprotocol/clientCapabilities": {},
                        "io.modelcontextprotocol/clientInfo": {"name": "missing-version"}
                    }
                })),
                "clientInfo must contain string name and version fields",
            ),
        ] {
            assert_eq!(request_metadata(&params), Err(expected));
        }
    }

    #[test]
    fn connection_era_is_selected_once() {
        let mut legacy = ConnectionEra::Undetermined;
        assert_eq!(
            select_request_protocol(
                &mut legacy,
                "initialize",
                &initialize_params(Some(LEGACY_PROTOCOL_VERSION))
            ),
            Ok(DispatchProtocol::Legacy(LEGACY_PROTOCOL_VERSION))
        );
        assert_eq!(
            select_request_protocol(
                &mut legacy,
                "tools/list",
                &modern_params(MODERN_PROTOCOL_VERSION)
            ),
            Err(ProtocolSelectionError::Unsupported {
                requested: MODERN_PROTOCOL_VERSION.into(),
                supported: vec![LEGACY_PROTOCOL_VERSION]
            })
        );

        let mut modern = ConnectionEra::Undetermined;
        assert_eq!(
            select_request_protocol(
                &mut modern,
                "server/discover",
                &modern_params(MODERN_PROTOCOL_VERSION)
            ),
            Ok(DispatchProtocol::Modern)
        );
        assert_eq!(
            select_request_protocol(&mut modern, "tools/list", &None),
            Err(ProtocolSelectionError::Invalid(
                "modern connections require per-request metadata"
            ))
        );
    }

    #[test]
    fn discovery_advertises_both_protocol_eras() {
        let mut response = handle_server_discover(json!(1), true);
        add_modern_result_metadata(&mut response, "server/discover");
        let result = response.result.unwrap();

        assert_eq!(result["resultType"], "complete");
        assert_eq!(
            result["supportedVersions"],
            json!(SUPPORTED_PROTOCOL_VERSIONS)
        );
        assert_eq!(result["capabilities"]["tools"], json!({}));
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            SERVER_NAME
        );
        assert_eq!(result["_meta"]["io.icm/embeddingsAvailable"], true);
    }

    #[test]
    fn unsupported_modern_version_lists_supported_versions() {
        let error = select_modern_version("2099-01-01".into()).unwrap_err();
        let response = protocol_error_response(json!(7), error);
        let error = response.error.unwrap();
        assert_eq!(error.code, -32022);
        assert_eq!(
            error.data.as_ref().unwrap()["supported"],
            json!(SUPPORTED_PROTOCOL_VERSIONS)
        );
        assert_eq!(error.data.unwrap()["requested"], "2099-01-01");
    }

    #[test]
    fn tools_list_shape_follows_negotiated_protocol() {
        let legacy = handle_tools_list(json!(1), true, ToolDefinitionOptions::none())
            .result
            .unwrap();
        assert!(legacy["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool.get("annotations").is_none()));

        let modern = handle_tools_list(
            json!(2),
            true,
            tool_definition_options(MODERN_PROTOCOL_VERSION),
        )
        .result
        .unwrap();
        assert!(modern["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool.get("annotations").is_some()));

        let mut modern_response = JsonRpcResponse::ok(json!(2), modern);
        add_modern_result_metadata(&mut modern_response, "tools/list");
        let modern = modern_response.result.unwrap();
        assert_eq!(modern["resultType"], "complete");
        assert_eq!(modern["ttlMs"], DISCOVERY_TTL_MS);
        assert_eq!(modern["cacheScope"], "private");
    }
}
