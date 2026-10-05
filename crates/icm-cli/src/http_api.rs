//! Persistent local HTTP API for ICM — warm-model fast path (issue #290).
//!
//! The CLI reloads the embedding model on every invocation (~9 s on
//! CPU), which makes semantic recall impractical for high-frequency
//! callers (scripts, agents, loops). `icm serve` already keeps the
//! model warm but only over an MCP/stdio JSON-RPC transport that is
//! awkward to call from non-MCP clients.
//!
//! This module adds an axum HTTP server (`icm serve --http
//! 127.0.0.1:11435`) that shares ONE warm [`Store`] and ONE
//! embedder across all requests via [`Arc`]. The endpoints mirror the
//! existing MCP tools and route through the SAME store methods so
//! behavior stays consistent.
//!
//! Response format defaults to TOON (the project's existing compact
//! representation, identical to `icm recall -f toon`). `?format=json`
//! or `Accept: application/json` returns the JSON variant. TOON keeps
//! token cost low for LLM-facing pipes; JSON suits programmatic
//! parsers.
//!
//! Bound to `127.0.0.1` by default — the user has to type any other
//! bind explicitly. An optional `--token` enables `Authorization:
//! Bearer <token>` checking; a loopback bind may run without one
//! ("open localhost API"), but any other interface without a token
//! is refused at startup — otherwise the full memory store would be
//! reachable, unauthenticated, to anyone on that interface.

use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use icm_mcp::protocol::{JsonRpcMessage, JsonRpcResponse};

use icm_core::{
    is_preference_topic, keyword_matches, project_matches, topic_matches, Embedder, Importance,
    Memory, MemoryStore, MSG_NO_MEMORIES,
};
use icm_store::{RecallEngine, RecallRequest, Store};

use crate::recall_format::{self, RecallFormat};

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Arc-shared so every axum handler reads the SAME warm store + embedder.
/// `Store` wraps a `rusqlite::Connection`, which is `Send` but
/// not `Sync`, so the same `Arc<Mutex<…>>` pattern as the web
/// dashboard (see `web.rs`) serializes DB access. Embedders are
/// already `Send + Sync` (see `icm-core::embedder::Embedder`).
/// `None` skips semantic recall — the `--no-embeddings` path.
#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
    embedder: Option<Arc<dyn Embedder + Send + Sync>>,
    mcp_calls_since_store: Arc<Mutex<u32>>,
    auto_consolidate: icm_mcp::AutoConsolidate,
    /// Operator-configured `[mcp] instructions` (issue #179 follow-up),
    /// appended to the built-in MCP handshake instructions.
    mcp_instructions: Option<Arc<str>>,
    /// When set, every request must carry `Authorization: Bearer <token>`.
    token: Option<String>,
}

/// Audit finding: every store access here treated a poisoned Mutex as a
/// *permanent* fault ("store poisoned", 500) rather than recovering, unlike
/// `web.rs::lock_store` (fixed in #372) — a single panic anywhere in Store
/// while the lock was held (a future bug, an upstream edge case) would
/// permanently 500 all five endpoints (recall/store/consolidate/stats/
/// topics) for the rest of the process, recoverable only by restarting
/// `icm serve --http`. A stdlib Mutex poison flag carries no corruption
/// guarantee for a plain data store — the guard's data is still valid,
/// just possibly mid-mutation from the panicking call, which the store's
/// own operations are already robust to (each is a self-contained SQL
/// statement/transaction).
fn lock_store(state: &AppState) -> std::sync::MutexGuard<'_, Store> {
    state
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AppState {
    fn embedder_ref(&self) -> Option<&dyn Embedder> {
        self.embedder.as_deref().map(|e| e as &dyn Embedder)
    }
}

// ---------------------------------------------------------------------------
// Response format negotiation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct FormatQuery {
    /// `toon` (default) or `json`. Accepts the same value `icm recall
    /// -f` accepts so muscle memory carries over.
    #[serde(default)]
    format: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum OutputFormat {
    Toon,
    Json,
}

impl OutputFormat {
    /// Resolve from `?format=` first, then `Accept` header. TOON is
    /// the default because the whole point of #290 is the low token
    /// cost on LLM-side reads.
    fn resolve(query: &FormatQuery, headers: &HeaderMap) -> Self {
        if let Some(q) = query.format.as_deref() {
            match q.to_ascii_lowercase().as_str() {
                "json" => return Self::Json,
                "toon" => return Self::Toon,
                _ => {}
            }
        }
        if let Some(a) = headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_ascii_lowercase)
        {
            // Honor explicit JSON requests; everything else (text/plain,
            // text/toon, */*, anything ambiguous) stays on TOON.
            if a.contains("application/json") && !a.contains("text/") {
                return Self::Json;
            }
        }
        Self::Toon
    }
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RecallReq {
    query: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    keyword: Option<String>,
    /// Empty string disables the project filter (matches the MCP tool
    /// convention). Omitted → no filter is applied; HTTP callers
    /// usually run outside any project so the cwd-based fallback that
    /// MCP uses is intentionally not replicated here.
    #[serde(default)]
    project: Option<String>,
    /// Token budget over the response body, estimated at four characters
    /// per token: each memory is charged what it takes in the format
    /// the response is rendered in (a TOON row, or its whole JSON
    /// object). Raises the `limit` ceiling from 100 to 500. Refused with
    /// the legacy engine, which cuts by count.
    #[serde(default)]
    max_tokens: Option<usize>,
    /// `v2` (default) or `legacy`, the engine before v2. Omitted:
    /// `$ICM_RECALL_ENGINE`, else v2.
    #[serde(default)]
    engine: Option<String>,
    /// Instant that date expressions in the query ("last week") are
    /// resolved against; v2 only. Same forms as `StoreReq::created_at`.
    /// Omitted: the server clock.
    #[serde(default)]
    now: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StoreReq {
    topic: String,
    content: String,
    #[serde(default)]
    importance: Option<String>,
    /// Accept either a CSV string (`"a,b,c"`) or a JSON array
    /// (`["a","b","c"]`). The CSV form mirrors `icm store -k a,b,c`.
    #[serde(default)]
    keywords: Option<Value>,
    #[serde(default)]
    raw: Option<String>,
    /// Date the content is from, when it is not "now" (imported
    /// documents, past conversations): RFC 3339, `YYYY-MM-DDTHH:MM:SS`
    /// (UTC) or `YYYY-MM-DD`. Sets `created_at` only.
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConsolidateReq {
    topic: String,
    #[serde(default)]
    keep_originals: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct McpQuery {
    #[serde(default)]
    compact: bool,
}

// ---------------------------------------------------------------------------
// Server entry
// ---------------------------------------------------------------------------

/// Pure guard, factored out for unit testing: reject a non-loopback bind
/// with no token. Returns `Err(message)` when the combination is unsafe.
fn check_bind_requires_token(addr: &SocketAddr, token: &Option<String>) -> Result<(), String> {
    if token.is_none() && !addr.ip().is_loopback() {
        return Err(format!(
            "refusing to bind {addr}: only loopback addresses (127.0.0.1, ::1) may run \
             without --token. Pass --token <TOKEN> to expose on other interfaces."
        ));
    }
    Ok(())
}

/// Run the HTTP server until it's interrupted. Loads NOTHING beyond
/// what the caller has already loaded — the warm store and embedder
/// are pre-built by `cmd_serve` and handed to us as `Arc`s.
#[tokio::main]
pub async fn run_http_server(
    store: Store,
    embedder: Option<Box<dyn Embedder + Send + Sync>>,
    addr: SocketAddr,
    token: Option<String>,
    auto_consolidate: icm_mcp::AutoConsolidate,
    mcp_instructions: Option<String>,
) -> Result<()> {
    // A non-loopback bind with no token exposes the full memory store —
    // recall, store, consolidate — to anyone who can reach the interface,
    // with zero authentication (security audit finding). Loopback-only
    // still works without a token, matching the doc comment's original
    // intent ("absent token = open localhost API").
    if let Err(msg) = check_bind_requires_token(&addr, &token) {
        anyhow::bail!(msg);
    }
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        embedder: embedder.map(Arc::from),
        mcp_calls_since_store: Arc::new(Mutex::new(0)),
        auto_consolidate,
        mcp_instructions: mcp_instructions.map(Arc::from),
        token,
    };

    let app = Router::new()
        .route("/mcp", post(handle_mcp))
        .route("/recall", post(handle_recall))
        .route("/store", post(handle_store))
        .route("/consolidate", post(handle_consolidate))
        .route("/stats", get(handle_stats))
        .route("/topics", get(handle_topics))
        .route("/health", get(handle_health))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
    let local = listener.local_addr().unwrap_or(addr);
    eprintln!("[icm http] listening on http://{local}");

    axum::serve(listener, app).await?;
    Ok(())
}

pub fn run_mcp_stdio_proxy(base_url: &str, token: Option<&str>, compact: bool) -> Result<()> {
    let endpoint = mcp_proxy_endpoint(base_url, compact)?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match post_mcp_json_rpc(&endpoint, token, line) {
            Ok(Some(body)) => {
                writeln!(stdout, "{body}")?;
                stdout.flush()?;
            }
            Ok(None) => {}
            Err(e) => {
                let id = json_rpc_id(line);
                let response = JsonRpcResponse::err(id, -32000, e.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
                stdout.flush()?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // Health is always reachable so an unauth'd liveness probe works.
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let Some(expected) = state.token.as_deref() else {
        return next.run(request).await;
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim);
    match presented {
        // Constant-time compare: a naive `==` leaks a timing side-channel an
        // attacker can use to brute-force the token byte-by-byte (audit
        // finding).
        Some(tok) if constant_time_eq(tok.as_bytes(), expected.as_bytes()) => {
            next.run(request).await
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            "missing or invalid Bearer token\n",
        )
            .into_response(),
    }
}

/// Compare two byte strings in time independent of where they first differ.
/// Still short-circuits on length (safe: lengths aren't secret here).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn handle_mcp(
    State(state): State<AppState>,
    Query(q): Query<McpQuery>,
    Json(raw): Json<Value>,
) -> Response {
    let msg: JsonRpcMessage = match serde_json::from_value(raw) {
        Ok(msg) => msg,
        Err(e) => {
            let response = JsonRpcResponse::err(Value::Null, -32700, format!("parse error: {e}"));
            return Json(response).into_response();
        }
    };

    let store = lock_store(&state);
    let mut calls_since_store = state
        .mcp_calls_since_store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    match icm_mcp::server::handle_json_rpc_message(
        msg,
        &store,
        state.embedder_ref(),
        q.compact,
        state.auto_consolidate,
        &mut calls_since_store,
        state.mcp_instructions.as_deref(),
    ) {
        Some(response) => Json(response).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

fn mcp_proxy_endpoint(base_url: &str, compact: bool) -> Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("--http-proxy URL must not be empty");
    }
    let mut endpoint = if trimmed.ends_with("/mcp") || trimmed.contains("/mcp?") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/mcp")
    };
    endpoint.push(if endpoint.contains('?') { '&' } else { '?' });
    endpoint.push_str("compact=");
    endpoint.push_str(if compact { "true" } else { "false" });
    Ok(endpoint)
}

fn post_mcp_json_rpc(endpoint: &str, token: Option<&str>, body: &str) -> Result<Option<String>> {
    let mut req = ureq::post(endpoint).set("content-type", "application/json");
    if let Some(token) = token {
        req = req.set("authorization", &format!("Bearer {token}"));
    }

    match req.send_string(body) {
        Ok(resp) => {
            if resp.status() == StatusCode::NO_CONTENT.as_u16() {
                return Ok(None);
            }
            let body = resp.into_string()?;
            if body.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(body))
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            anyhow::bail!("HTTP {code} from ICM daemon: {}", text.trim());
        }
        Err(e) => Err(e.into()),
    }
}

fn json_rpc_id(line: &str) -> Value {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Handler: /recall
// ---------------------------------------------------------------------------

async fn handle_recall(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FormatQuery>,
    Json(req): Json<RecallReq>,
) -> Response {
    let format = OutputFormat::resolve(&q, &headers);
    let results = RecallEngine::resolve(req.engine.as_deref(), req.max_tokens.is_some())
        .map_err(anyhow::Error::from)
        .and_then(|engine| run_recall_with(&state, &req, engine, format));
    match results {
        Ok(results) => render_recall(&results, format),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e.to_string(), format),
    }
}

/// Run a recall on the given engine. `Legacy` is the pre-v2 code path,
/// untouched; the v2 fields of the request are ignored there. `format`
/// is what the response will be rendered in: the v2 budget is spent on it.
fn run_recall_with(
    state: &AppState,
    req: &RecallReq,
    engine: RecallEngine,
    format: OutputFormat,
) -> Result<Vec<(Memory, Option<f32>)>> {
    match engine {
        RecallEngine::Legacy => run_recall(state, req),
        RecallEngine::V2 => run_recall_v2(state, req, format),
    }
}

/// The renderer format behind a response format.
fn recall_format_of(format: OutputFormat) -> RecallFormat {
    match format {
        OutputFormat::Toon => RecallFormat::Toon,
        OutputFormat::Json => RecallFormat::Json,
    }
}

/// `limit` of a v2 request: default and ceiling. Without a budget they are
/// the legacy ones. With a budget the budget does the cutting and `limit`
/// is only a ceiling on the item count.
const V2_DEFAULT_LIMIT: usize = 5;
const V2_MAX_LIMIT: usize = 100;
const V2_DEFAULT_LIMIT_WITH_BUDGET: usize = 200;
const V2_MAX_LIMIT_WITH_BUDGET: usize = 500;

/// Recall through the shared v2 pipeline (`icm_store::RecallRequest`). No
/// neighbor expansion, like the legacy HTTP path, and the same response
/// shape: a `score` is rendered only when the server has an embedder (a
/// keyword-only ranking has no similarity to report). Under `max_tokens`,
/// the body rendered in `format` fits the budget.
fn run_recall_v2(
    state: &AppState,
    req: &RecallReq,
    format: OutputFormat,
) -> Result<Vec<(Memory, Option<f32>)>> {
    if req.query.trim().is_empty() {
        anyhow::bail!("missing required field: query");
    }
    let now = match req.now.as_deref() {
        Some(raw) => Some(icm_core::parse_instant(raw).map_err(|e| anyhow::anyhow!("now: {e}"))?),
        None => None,
    };
    let (default_limit, max_limit) = if req.max_tokens.is_some() {
        (V2_DEFAULT_LIMIT_WITH_BUDGET, V2_MAX_LIMIT_WITH_BUDGET)
    } else {
        (V2_DEFAULT_LIMIT, V2_MAX_LIMIT)
    };
    let with_score = state.embedder.is_some();

    let rendered_as = recall_format_of(format);
    let envelope_tokens = crate::recall_header_tokens(with_score, rendered_as);
    let request = RecallRequest {
        query: &req.query,
        limit: req.limit.unwrap_or(default_limit).clamp(1, max_limit),
        max_tokens: req.max_tokens.map(|b| b.saturating_sub(envelope_tokens)),
        topic: req.topic.as_deref(),
        keyword: req.keyword.as_deref(),
        project: req.project.as_deref(),
        now,
        expand_neighbors: false,
    };

    let store = lock_store(state);
    let outcome = request.run_with_cost(&store, state.embedder_ref(), &|m, score| {
        crate::rendered_recall_tokens(m, with_score.then_some(score), rendered_as)
    })?;
    Ok(outcome
        .hits
        .into_iter()
        .map(|h| (h.memory, with_score.then_some(h.score)))
        .collect())
}

/// Recall logic mirrored from `icm-mcp::tools::tool_recall` but
/// returning the raw `Vec<(Memory, Option<f32>)>` so HTTP can format
/// it with the project's `recall_format` renderer (TOON or JSON).
/// Reuses the same store methods so behavior stays consistent across
/// transports.
fn run_recall(state: &AppState, req: &RecallReq) -> Result<Vec<(Memory, Option<f32>)>> {
    if req.query.trim().is_empty() {
        anyhow::bail!("missing required field: query");
    }
    let store = lock_store(state);
    if let Err(e) = store.maybe_auto_decay() {
        tracing::warn!(error = %e, "auto-decay failed during /recall");
    }

    let limit = req.limit.unwrap_or(5).clamp(1, 100);

    let project_filter = |m: &Memory| -> bool {
        match req.project.as_deref() {
            None | Some("") => true,
            Some(p) => is_preference_topic(&m.topic) || project_matches(&m.topic, Some(p)),
        }
    };

    let scored: Vec<(Memory, Option<f32>)> = if let Some(emb) = state.embedder_ref() {
        match emb.embed_query(&req.query) {
            Ok(q_emb) => match store.search_hybrid(&req.query, &q_emb, limit) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|(m, _)| project_filter(m))
                    .filter(|(m, _)| {
                        req.topic
                            .as_deref()
                            .is_none_or(|t| topic_matches(&m.topic, t))
                    })
                    .filter(|(m, _)| {
                        req.keyword
                            .as_deref()
                            .is_none_or(|k| keyword_matches(&m.keywords, k))
                    })
                    .map(|(m, s)| (m, Some(s)))
                    .collect(),
                Err(_) => fts_fallback(&store, req, &project_filter, limit)?,
            },
            Err(_) => fts_fallback(&store, req, &project_filter, limit)?,
        }
    } else {
        fts_fallback(&store, req, &project_filter, limit)?
    };

    // Best-effort access bookkeeping (matches the MCP path).
    let ids: Vec<&str> = scored.iter().map(|(m, _)| m.id.as_str()).collect();
    let _ = store.batch_update_access(&ids);

    Ok(scored)
}

fn fts_fallback<F>(
    store: &Store,
    req: &RecallReq,
    project_filter: &F,
    limit: usize,
) -> Result<Vec<(Memory, Option<f32>)>>
where
    F: Fn(&Memory) -> bool,
{
    let mut rows = store.search_fts(&req.query, limit)?;
    if rows.is_empty() {
        let keywords: Vec<&str> = req.query.split_whitespace().collect();
        rows = store.search_by_keywords(&keywords, limit)?;
    }
    rows.retain(project_filter);
    if let Some(t) = req.topic.as_deref() {
        rows.retain(|m| topic_matches(&m.topic, t));
    }
    if let Some(k) = req.keyword.as_deref() {
        rows.retain(|m| keyword_matches(&m.keywords, k));
    }
    Ok(rows.into_iter().map(|m| (m, None)).collect())
}

fn render_recall(results: &[(Memory, Option<f32>)], format: OutputFormat) -> Response {
    if results.is_empty() {
        return text_response(MSG_NO_MEMORIES, format);
    }
    match format {
        OutputFormat::Toon => match recall_format::render(results, RecallFormat::Toon) {
            Ok(body) => toon_response(body),
            Err(e) => err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("toon render failed: {e}"),
                format,
            ),
        },
        OutputFormat::Json => match recall_format::render(results, RecallFormat::Json) {
            Ok(body) => json_string_response(body),
            Err(e) => err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("json render failed: {e}"),
                format,
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Handler: /store
// ---------------------------------------------------------------------------

async fn handle_store(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FormatQuery>,
    Json(req): Json<StoreReq>,
) -> Response {
    let format = OutputFormat::resolve(&q, &headers);
    if req.topic.trim().is_empty() || req.content.trim().is_empty() {
        return err_response(
            StatusCode::BAD_REQUEST,
            "topic and content must be non-empty",
            format,
        );
    }
    let importance = match parse_importance(req.importance.as_deref()) {
        Ok(i) => i,
        Err(e) => return err_response(StatusCode::BAD_REQUEST, &e, format),
    };
    let keywords = parse_keywords_value(req.keywords.as_ref());

    let created_at = match req.created_at.as_deref().map(icm_core::parse_instant) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            return err_response(StatusCode::BAD_REQUEST, &format!("created_at: {e}"), format)
        }
        None => None,
    };

    let mut mem = Memory::new(req.topic.clone(), req.content.clone(), importance);
    mem.keywords = keywords;
    // `updated_at` and `last_accessed` stay at the time of the write.
    if let Some(t) = created_at {
        mem.created_at = t;
    }
    if let Some(raw) = req.raw.as_deref().filter(|s| !s.is_empty()) {
        mem.raw_excerpt = Some(raw.to_string());
    }
    if let Some(emb) = state.embedder_ref() {
        if let Ok(v) = emb.embed(&format!("{} {}", mem.topic, mem.summary)) {
            mem.embedding = Some(v);
        }
    }

    let outcome = lock_store(&state).store(mem.clone());
    match outcome {
        Ok(id) => {
            let mut stored = mem;
            stored.id = id;
            render_recall(&[(stored, None)], format)
        }
        Err(e) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("store failed: {e}"),
            format,
        ),
    }
}

fn parse_importance(s: Option<&str>) -> Result<Importance, String> {
    let raw = s.unwrap_or("medium").to_ascii_lowercase();
    match raw.as_str() {
        "critical" => Ok(Importance::Critical),
        "high" => Ok(Importance::High),
        "medium" => Ok(Importance::Medium),
        "low" => Ok(Importance::Low),
        other => Err(format!(
            "invalid importance {other:?}; expected one of: critical, high, medium, low"
        )),
    }
}

fn parse_keywords_value(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(csv)) => csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Handler: /consolidate
// ---------------------------------------------------------------------------

async fn handle_consolidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FormatQuery>,
    Json(req): Json<ConsolidateReq>,
) -> Response {
    let format = OutputFormat::resolve(&q, &headers);
    if req.topic.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "topic required", format);
    }
    let store = lock_store(&state);
    match store.count_by_topic(&req.topic) {
        Ok(0) => {
            return err_response(
                StatusCode::NOT_FOUND,
                &format!("no memories under topic {:?}", req.topic),
                format,
            )
        }
        Ok(_) => {}
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("topic lookup failed: {e}"),
                format,
            )
        }
    }
    // Same bug class as #400 (cmd_consolidate/tool_consolidate): this is a
    // third, independent /consolidate implementation that had the same gap
    // — never attached an embedding to the merged memory it creates.
    let embedder = state.embedder_ref();
    let mut build = |_covered: &[&Memory], summary: String| {
        // The engine sets the importance from what the pass covers.
        let mut consolidated = Memory::new(req.topic.clone(), summary, Importance::Medium);
        if let Some(emb) = embedder {
            if let Ok(v) = emb.embed(&consolidated.embed_text()) {
                consolidated.embedding = Some(v);
            }
        }
        consolidated
    };

    // The lexical join, through the same engine as `icm consolidate`: only
    // the memories that are in the join are removed, pass by pass when the
    // topic holds more than one read or one summary can carry — never the
    // whole topic by name, which also took whatever had not been read or
    // was stored in the meantime.
    let run = crate::consolidate_in_passes(
        &store,
        &req.topic,
        &crate::PassWriter::Lexical,
        req.keep_originals,
        crate::MAX_CONSOLIDATION_PASSES,
        &mut build,
    );
    match run {
        Ok(run) => {
            // The body is what it always was: the memory that now stands
            // for the topic. With nothing to merge (one memory, or only
            // critical ones) that is what the topic already holds.
            let shown: Vec<(Memory, Option<f32>)> = match run.summary {
                Some(summary) => vec![(summary, None)],
                None => store
                    .get_by_topic(&req.topic)
                    .unwrap_or_default()
                    .into_iter()
                    .take(1)
                    .map(|m| (m, None))
                    .collect(),
            };
            let mut resp = render_recall(&shown, format);
            // Additive headers.
            for (name, value) in [
                ("x-icm-consolidated", run.replaced),
                ("x-icm-left-in-place", run.others_left),
            ] {
                if let Ok(value) = axum::http::HeaderValue::from_str(&value.to_string()) {
                    resp.headers_mut().insert(name, value);
                }
            }
            resp
        }
        Err(e) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("consolidate failed: {e}"),
            format,
        ),
    }
}

// ---------------------------------------------------------------------------
// Handler: /stats
// ---------------------------------------------------------------------------

async fn handle_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FormatQuery>,
) -> Response {
    let format = OutputFormat::resolve(&q, &headers);
    let store = lock_store(&state);
    match store.stats() {
        Ok(s) => {
            let payload = json!({
                "total_memories": s.total_memories,
                "total_topics": s.total_topics,
                "avg_weight": s.avg_weight,
                "oldest_memory": s.oldest_memory.map(|d| d.to_rfc3339()),
                "newest_memory": s.newest_memory.map(|d| d.to_rfc3339()),
            });
            match format {
                OutputFormat::Json => json_value_response(payload),
                // TOON for a single key-value object: emit a 1-row table.
                OutputFormat::Toon => {
                    let body = format!(
                        "stats[1]{{total_memories,total_topics,avg_weight,oldest,newest}}:\n  \
                         {},{},{:.3},{},{}\n",
                        s.total_memories,
                        s.total_topics,
                        s.avg_weight,
                        s.oldest_memory
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_else(|| "-".into()),
                        s.newest_memory
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_else(|| "-".into()),
                    );
                    toon_response(body)
                }
            }
        }
        Err(e) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("stats failed: {e}"),
            format,
        ),
    }
}

// ---------------------------------------------------------------------------
// Handler: /topics
// ---------------------------------------------------------------------------

async fn handle_topics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FormatQuery>,
) -> Response {
    let format = OutputFormat::resolve(&q, &headers);
    let store = lock_store(&state);
    match store.list_topics() {
        Ok(rows) => match format {
            OutputFormat::Json => json_value_response(json!(rows
                .iter()
                .map(|(t, n)| json!({"topic": t, "count": n}))
                .collect::<Vec<_>>())),
            OutputFormat::Toon => {
                let mut body = format!("topics[{}]{{topic,count}}:\n", rows.len());
                for (t, n) in &rows {
                    let topic = if t.contains(',') || t.contains('"') {
                        format!("\"{}\"", t.replace('"', "\"\""))
                    } else {
                        t.clone()
                    };
                    body.push_str(&format!("  {topic},{n}\n"));
                }
                toon_response(body)
            }
        },
        Err(e) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("topics failed: {e}"),
            format,
        ),
    }
}

// ---------------------------------------------------------------------------
// Handler: /health (unauthenticated, used by integration tests + probes)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Health {
    status: &'static str,
    has_embedder: bool,
}

async fn handle_health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "ok",
        has_embedder: state.embedder.is_some(),
    })
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

fn toon_response(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

fn json_string_response(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

fn json_value_response(v: Value) -> Response {
    Json(v).into_response()
}

fn text_response(body: &str, format: OutputFormat) -> Response {
    match format {
        OutputFormat::Json => json_value_response(json!({"message": body, "results": []})),
        OutputFormat::Toon => toon_response(format!("{body}\n")),
    }
}

fn err_response(status: StatusCode, msg: &str, format: OutputFormat) -> Response {
    match format {
        OutputFormat::Json => (status, Json(json!({"error": msg}))).into_response(),
        OutputFormat::Toon => (
            status,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("error: {msg}\n"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(name: &'static str, val: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(val).unwrap());
        h
    }

    #[test]
    fn output_format_defaults_to_toon() {
        let q = FormatQuery::default();
        let hs = HeaderMap::new();
        assert!(matches!(OutputFormat::resolve(&q, &hs), OutputFormat::Toon));
    }

    #[test]
    fn output_format_query_json_wins() {
        let q = FormatQuery {
            format: Some("json".into()),
        };
        let hs = HeaderMap::new();
        assert!(matches!(OutputFormat::resolve(&q, &hs), OutputFormat::Json));
    }

    #[test]
    fn output_format_query_toon_explicit() {
        let q = FormatQuery {
            format: Some("toon".into()),
        };
        // Even if the client sends `Accept: application/json`, explicit
        // `?format=toon` wins so the user has the last word.
        let hs = h("accept", "application/json");
        assert!(matches!(OutputFormat::resolve(&q, &hs), OutputFormat::Toon));
    }

    #[test]
    fn output_format_accept_application_json() {
        let q = FormatQuery::default();
        let hs = h("accept", "application/json");
        assert!(matches!(OutputFormat::resolve(&q, &hs), OutputFormat::Json));
    }

    #[test]
    fn output_format_accept_text_plain_stays_toon() {
        let q = FormatQuery::default();
        let hs = h("accept", "text/plain");
        assert!(matches!(OutputFormat::resolve(&q, &hs), OutputFormat::Toon));
    }

    #[test]
    fn parse_importance_accepts_known_values() {
        assert!(matches!(
            parse_importance(Some("critical")),
            Ok(Importance::Critical)
        ));
        assert!(matches!(
            parse_importance(Some("HIGH")),
            Ok(Importance::High)
        ));
        assert!(matches!(parse_importance(None), Ok(Importance::Medium)));
        assert!(parse_importance(Some("bogus")).is_err());
    }

    #[test]
    fn parse_keywords_value_handles_string_and_array() {
        let s = Value::String("a, b ,c".into());
        let v = parse_keywords_value(Some(&s));
        assert_eq!(v, vec!["a", "b", "c"]);

        let arr = json!(["foo", "", "bar"]);
        let v = parse_keywords_value(Some(&arr));
        assert_eq!(v, vec!["foo", "bar"]);

        assert!(parse_keywords_value(None).is_empty());
        assert!(parse_keywords_value(Some(&json!(42))).is_empty());
    }

    /// Audit regression: a non-loopback bind with no `--token` exposes the
    /// full memory store (recall/store/consolidate) unauthenticated to
    /// anyone who can reach the interface — must be rejected.
    #[test]
    fn non_loopback_bind_without_token_is_rejected() {
        let addr: SocketAddr = "0.0.0.0:8420".parse().unwrap();
        let err = check_bind_requires_token(&addr, &None).unwrap_err();
        assert!(err.contains("--token"));

        let addr: SocketAddr = "203.0.113.5:8420".parse().unwrap();
        assert!(check_bind_requires_token(&addr, &None).is_err());
    }

    #[test]
    fn loopback_bind_without_token_is_still_allowed() {
        // Loopback-only stays usable without a token — same intent as the
        // module doc comment ("absent token = open localhost API"), just
        // now scoped to loopback instead of any address.
        let addr: SocketAddr = "127.0.0.1:8420".parse().unwrap();
        assert!(check_bind_requires_token(&addr, &None).is_ok());
        let addr: SocketAddr = "[::1]:8420".parse().unwrap();
        assert!(check_bind_requires_token(&addr, &None).is_ok());
    }

    #[test]
    fn non_loopback_bind_with_token_is_allowed() {
        let addr: SocketAddr = "0.0.0.0:8420".parse().unwrap();
        assert!(check_bind_requires_token(&addr, &Some("secret".into())).is_ok());
    }

    #[test]
    fn constant_time_eq_matches_naive_equality() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"wrong!"));
        assert!(!constant_time_eq(b"short", b"longer-string"));
        assert!(constant_time_eq(b"", b""));
    }

    /// Audit regression: every store access here treated a poisoned Mutex
    /// as a permanent fault ("store poisoned", 500), unlike web.rs's
    /// lock_store (fixed in #372) — a single panic anywhere in Store while
    /// the lock was held would permanently break recall/store/consolidate/
    /// stats/topics for the rest of the process.
    #[test]
    fn lock_store_recovers_from_a_poisoned_mutex() {
        let state = AppState {
            store: Arc::new(Mutex::new(Store::in_memory().unwrap())),
            embedder: None,
            mcp_calls_since_store: Arc::new(Mutex::new(0)),
            auto_consolidate: icm_mcp::AutoConsolidate {
                enabled: false,
                threshold: 10,
                queue: false,
            },
            mcp_instructions: None,
            token: None,
        };

        // Poison the mutex: panic while holding the guard on another thread.
        let poisoner = state.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.store.lock().unwrap();
            panic!("intentional poison");
        })
        .join();
        assert!(state.store.is_poisoned(), "setup: mutex must be poisoned");

        // The recovering lock still yields a working store.
        let store = lock_store(&state);
        assert!(
            store.stats().is_ok(),
            "store must remain usable after poison"
        );
    }

    /// Manual-testing finding (against the real HTTP server): a third,
    /// independent /consolidate implementation — same bug class as #400
    /// (cmd_consolidate/tool_consolidate) — never attached an embedding
    /// to the merged memory it creates, even though `state.embedder_ref()`
    /// is right there and every sibling handler (store/recall/embed_all)
    /// already uses it.
    #[tokio::test]
    async fn handle_consolidate_attaches_an_embedding_to_the_merged_memory() {
        use icm_core::IcmResult;

        struct StubEmbedder;
        impl Embedder for StubEmbedder {
            fn embed(&self, _text: &str) -> IcmResult<Vec<f32>> {
                Ok(vec![0.4_f32; 64])
            }
            fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
                texts.iter().map(|t| self.embed(t)).collect()
            }
            fn dimensions(&self) -> usize {
                64
            }
        }

        let store = Store::in_memory_with_dims(64).unwrap();
        store
            .store(Memory::new(
                "http-test".into(),
                "expendable 1".into(),
                Importance::Medium,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "http-test".into(),
                "expendable 2".into(),
                Importance::Medium,
            ))
            .unwrap();

        let state = AppState {
            store: Arc::new(Mutex::new(store)),
            embedder: Some(Arc::new(StubEmbedder)),
            mcp_calls_since_store: Arc::new(Mutex::new(0)),
            auto_consolidate: icm_mcp::AutoConsolidate {
                enabled: false,
                threshold: 10,
                queue: false,
            },
            mcp_instructions: None,
            token: None,
        };

        let resp = handle_consolidate(
            State(state.clone()),
            HeaderMap::new(),
            Query(FormatQuery::default()),
            Json(ConsolidateReq {
                topic: "http-test".into(),
                keep_originals: false,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let store = lock_store(&state);
        let memories = store.get_by_topic("http-test").unwrap();
        assert_eq!(memories.len(), 1);
        assert!(
            memories[0].embedding.is_some(),
            "consolidated memory must have an embedding attached"
        );
    }

    /// `POST /consolidate` reads the topic, builds the join, then replaces.
    /// A memory stored in between — by another process on the same database
    /// — was deleted with the topic without being in the join. The "other
    /// process" here is a second connection, opened from the embedder: the
    /// one step that runs between the read and the write.
    #[tokio::test]
    async fn handle_consolidate_keeps_a_memory_stored_while_it_runs() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct LateWriter {
            db: std::path::PathBuf,
            done: AtomicBool,
        }
        impl Embedder for LateWriter {
            fn embed(&self, _text: &str) -> icm_core::IcmResult<Vec<f32>> {
                if !self.done.swap(true, Ordering::SeqCst) {
                    let other = Store::with_dims(&self.db, 64).unwrap();
                    other
                        .store(Memory::new(
                            "http-test".into(),
                            "LATE-ARRIVAL".into(),
                            Importance::Medium,
                        ))
                        .unwrap();
                }
                Ok(vec![0.4_f32; 64])
            }
            fn embed_batch(&self, texts: &[&str]) -> icm_core::IcmResult<Vec<Vec<f32>>> {
                texts.iter().map(|t| self.embed(t)).collect()
            }
            fn dimensions(&self) -> usize {
                64
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("memories.db");
        let store = Store::with_dims(&db, 64).unwrap();
        for i in 0..3 {
            store
                .store(Memory::new(
                    "http-test".into(),
                    format!("expendable {i}"),
                    Importance::Medium,
                ))
                .unwrap();
        }
        let mut state = test_state(store, false);
        state.embedder = Some(Arc::new(LateWriter {
            db,
            done: AtomicBool::new(false),
        }));

        let resp = handle_consolidate(
            State(state.clone()),
            HeaderMap::new(),
            Query(FormatQuery::default()),
            Json(ConsolidateReq {
                topic: "http-test".into(),
                keep_originals: false,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["x-icm-consolidated"], "3");
        assert_eq!(resp.headers()["x-icm-left-in-place"], "1");

        let store = lock_store(&state);
        let mut left: Vec<String> = store
            .get_by_topic("http-test")
            .unwrap()
            .into_iter()
            .map(|m| m.summary)
            .collect();
        left.sort();
        assert_eq!(left.len(), 2, "{left:?}");
        assert_eq!(
            left[0], "LATE-ARRIVAL",
            "stored mid-consolidation, then deleted"
        );
        for i in 0..3 {
            assert!(left[1].contains(&format!("expendable {i}")), "{}", left[1]);
        }
    }

    /// A topic larger than one read of the store (500 memories): every fact
    /// must be in the result. The 20 that were never read used to be
    /// deleted with the rest.
    #[tokio::test]
    async fn handle_consolidate_never_removes_memories_it_did_not_join() {
        let store = Store::in_memory_with_dims(TEST_DIMS).unwrap();
        for i in 0..520 {
            store
                .store(Memory::new(
                    "many".into(),
                    format!("fact {i:03};"),
                    Importance::Medium,
                ))
                .unwrap();
        }
        let state = test_state(store, false);
        let resp = handle_consolidate(
            State(state.clone()),
            HeaderMap::new(),
            Query(FormatQuery::default()),
            Json(ConsolidateReq {
                topic: "many".into(),
                keep_originals: false,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["x-icm-consolidated"], "520");
        assert_eq!(resp.headers()["x-icm-left-in-place"], "0");

        let store = lock_store(&state);
        let after = store.get_by_topic("many").unwrap();
        assert_eq!(after.len(), 1, "two passes: 500, then the join + 20");
        for i in 0..520 {
            assert!(
                after[0].summary.contains(&format!("fact {i:03};")),
                "fact {i} was deleted without being joined"
            );
        }
    }

    // --- v2 engine, token budget, caller-supplied dates ---------------------

    const TEST_DIMS: usize = 64;

    /// Deterministic bag-of-words embedder: texts sharing words are close.
    struct WordEmbedder;
    impl Embedder for WordEmbedder {
        fn embed(&self, text: &str) -> icm_core::IcmResult<Vec<f32>> {
            let mut v = vec![0.0_f32; TEST_DIMS];
            for word in text.split(|c: char| !c.is_alphanumeric()) {
                if !word.is_empty() {
                    let bucket = word.to_lowercase().bytes().fold(7usize, |acc, b| {
                        acc.wrapping_mul(31).wrapping_add(usize::from(b))
                    }) % TEST_DIMS;
                    v[bucket] += 1.0;
                }
            }
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                v.iter_mut().for_each(|x| *x /= norm);
            } else {
                v[0] = 1.0;
            }
            Ok(v)
        }
        fn embed_batch(&self, texts: &[&str]) -> icm_core::IcmResult<Vec<Vec<f32>>> {
            texts.iter().map(|t| self.embed(t)).collect()
        }
        fn dimensions(&self) -> usize {
            TEST_DIMS
        }
    }

    fn test_state(store: Store, with_embedder: bool) -> AppState {
        AppState {
            store: Arc::new(Mutex::new(store)),
            embedder: with_embedder
                .then(|| Arc::new(WordEmbedder) as Arc<dyn Embedder + Send + Sync>),
            mcp_calls_since_store: Arc::new(Mutex::new(0)),
            auto_consolidate: icm_mcp::AutoConsolidate {
                enabled: false,
                threshold: 10,
                queue: false,
            },
            mcp_instructions: None,
            token: None,
        }
    }

    /// 120 embedded memories that all match "deploy", about 50 tokens each.
    fn seeded_state() -> AppState {
        let store = Store::in_memory_with_dims(TEST_DIMS).unwrap();
        for i in 0..120 {
            let mut m = Memory::new(
                "ops-notes".into(),
                format!(
                    "deploy note {i:03}: the release pipeline step {i} needs a manual check \
                     before the rollout continues, see the runbook entry number {i} for the \
                     exact order of the commands and the rollback."
                ),
                Importance::Medium,
            );
            m.embedding = Some(WordEmbedder.embed(&m.embed_text()).unwrap());
            store.store(m).unwrap();
        }
        test_state(store, true)
    }

    /// The handler tests that rely on defaults are meaningless when the
    /// developer's shell overrides the recall engine or depth.
    fn recall_env_is_clean() -> bool {
        std::env::var_os("ICM_RECALL_ENGINE").is_none()
            && std::env::var_os("ICM_RECALL_DEPTH").is_none()
    }

    /// POST a JSON body to `/recall?format=json`; returns status and body.
    async fn post_recall(state: &AppState, body: Value) -> (StatusCode, String) {
        post_recall_as(state, body, "json").await
    }

    /// POST a JSON body to `/recall?format=<format>`.
    async fn post_recall_as(state: &AppState, body: Value, format: &str) -> (StatusCode, String) {
        let req: RecallReq = serde_json::from_value(body).unwrap();
        let resp = handle_recall(
            State(state.clone()),
            HeaderMap::new(),
            Query(FormatQuery {
                format: Some(format.into()),
            }),
            Json(req),
        )
        .await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// POST a JSON body to `/store?format=json`; returns status and body.
    async fn post_store(state: &AppState, body: Value) -> (StatusCode, String) {
        let req: StoreReq = serde_json::from_value(body).unwrap();
        let resp = handle_store(
            State(state.clone()),
            HeaderMap::new(),
            Query(FormatQuery {
                format: Some("json".into()),
            }),
            Json(req),
        )
        .await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn rows(body: &str) -> Vec<Value> {
        serde_json::from_str::<Value>(body)
            .unwrap()
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("expected a JSON array, got: {body}"))
    }

    fn ids(rows: &[Value]) -> Vec<String> {
        rows.iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// The legacy engine cannot return more than 40 memories whatever the
    /// limit; under a token budget v2 fills the budget instead. The budget
    /// is checked on the body the client receives, not on the summaries.
    #[tokio::test]
    async fn recall_v2_with_max_tokens_fills_the_budget_past_forty_rows() {
        if !recall_env_is_clean() {
            return;
        }
        let state = seeded_state();
        for (format, budget) in [("json", 12_000), ("toon", 4000)] {
            let (status, body) = post_recall_as(
                &state,
                json!({"query": "deploy release pipeline", "engine": "v2", "max_tokens": budget}),
                format,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let hits = body.matches("deploy note").count();
            assert!(hits > 40, "{format}: got {hits} rows");
            assert!(hits < 120, "{format}: the budget must cut, got {hits}");

            let rendered = icm_core::estimate_tokens(&body);
            assert!(
                rendered <= budget,
                "{format}: body is {rendered} tokens for a budget of {budget}"
            );
            assert!(
                rendered * 10 >= budget * 9,
                "{format}: body uses only {rendered} of {budget} tokens"
            );
        }

        // Scores are the fused ones, best first.
        let (_, body) = post_recall(
            &state,
            json!({"query": "deploy release pipeline", "max_tokens": 12_000}),
        )
        .await;
        let rows = rows(&body);
        assert!(rows[0]["score"].as_f64().unwrap() >= rows[1]["score"].as_f64().unwrap());
    }

    /// Memories whose rendering is much larger than their summary: raw
    /// excerpt, keywords, characters that need escaping.
    fn awkward_store() -> Store {
        let store = Store::in_memory().unwrap();
        for i in 0..60 {
            let mut m = Memory::new(
                "errors-resolved".into(),
                format!("budget probe entry {i}, \"quoted\",\nsecond line about the deploy"),
                Importance::Medium,
            );
            m.keywords = vec!["deploy".into(), "pipeline".into(), "budget".into()];
            if i % 2 == 0 {
                m.raw_excerpt = Some(format!(
                    "{i} {}",
                    "error: \"connection refused\"\n\tat step; ".repeat(40)
                ));
            }
            store.store(m).unwrap();
        }
        store
    }

    /// What is not the summary is charged to the budget too.
    #[tokio::test]
    async fn recall_budget_bounds_the_body_of_memories_with_raw_excerpts() {
        if !recall_env_is_clean() {
            return;
        }
        let state = test_state(awkward_store(), false);
        for format in ["json", "toon"] {
            for budget in [600, 3000] {
                let (status, body) = post_recall_as(
                    &state,
                    json!({"query": "budget probe entry", "max_tokens": budget}),
                    format,
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{body}");
                let hits = body.matches("budget probe entry").count();
                let rendered = icm_core::estimate_tokens(&body);
                assert!(
                    hits >= 1 && rendered <= budget,
                    "{format}: {hits} hits, body of {rendered} tokens for a budget of {budget}"
                );
            }
        }
    }

    /// `icm recall --max-tokens` goes through `icm_store::recall_v2`, which
    /// does not know the output format: whichever one the CLI renders, the
    /// budget must hold.
    #[test]
    fn recall_v2_default_cost_bounds_every_cli_format() {
        let store = awkward_store();
        for budget in [1500, 6000] {
            let outcome = icm_store::recall_v2(
                &store,
                None,
                &RecallRequest {
                    query: "budget probe entry",
                    limit: 200,
                    max_tokens: Some(budget),
                    topic: None,
                    keyword: None,
                    project: None,
                    now: None,
                    expand_neighbors: true,
                },
            )
            .unwrap();
            let results: Vec<(Memory, Option<f32>)> = outcome
                .hits
                .into_iter()
                .map(|h| (h.memory, Some(h.score)))
                .collect();
            assert!(results.len() >= 2, "{} hits", results.len());
            for format in [
                RecallFormat::Json,
                RecallFormat::Toml,
                RecallFormat::Detail,
                RecallFormat::Toon,
            ] {
                let rendered = recall_format::render(&results, format).unwrap();
                let tokens = icm_core::estimate_tokens(&rendered);
                assert!(
                    tokens <= budget,
                    "{format:?}: {tokens} tokens rendered for a budget of {budget}"
                );
            }
        }
    }

    #[tokio::test]
    async fn recall_max_tokens_alone_selects_v2() {
        if !recall_env_is_clean() {
            return;
        }
        let state = seeded_state();
        let (status, body) =
            post_recall(&state, json!({"query": "deploy", "max_tokens": 12_000})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(rows(&body).len() > 40);

        // The legacy engine cuts by count: asking it for a budget is
        // refused instead of being dropped without a word.
        let (status, body) = post_recall(
            &state,
            json!({"query": "deploy", "max_tokens": 4000, "engine": "legacy"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.contains("max_tokens") && body.contains("legacy"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn recall_v2_without_budget_cuts_by_limit() {
        if !recall_env_is_clean() {
            return;
        }
        let state = seeded_state();
        let (_, body) = post_recall(&state, json!({"query": "deploy", "engine": "v2"})).await;
        assert_eq!(rows(&body).len(), 5);

        let (_, body) = post_recall(
            &state,
            json!({"query": "deploy", "engine": "v2", "limit": 60}),
        )
        .await;
        assert_eq!(rows(&body).len(), 60);

        // The budget cuts first, `limit` caps the item count.
        let (_, body) = post_recall(
            &state,
            json!({"query": "deploy", "max_tokens": 100000, "limit": 12}),
        )
        .await;
        assert_eq!(rows(&body).len(), 12);
    }

    /// The legacy engine, on request, runs the pre-v2 code and returns what
    /// `search_hybrid` ranks.
    #[tokio::test]
    async fn recall_legacy_engine_matches_search_hybrid() {
        // Graded overlap with the query so the top five have distinct
        // scores: `search_hybrid` orders exact ties arbitrarily.
        let store = Store::in_memory_with_dims(TEST_DIMS).unwrap();
        let fruits = ["kiwi", "mango", "papaya", "guava", "lychee"];
        let mut texts: Vec<String> = (1..=fruits.len()).map(|k| fruits[..k].join(" ")).collect();
        texts.extend((0..30).map(|i| format!("build cache entry {i} for the compiler")));
        for text in texts {
            let mut m = Memory::new("fruits".into(), text, Importance::Medium);
            m.embedding = Some(WordEmbedder.embed(&m.embed_text()).unwrap());
            store.store(m).unwrap();
        }
        let state = test_state(store, true);

        let query = "kiwi mango papaya guava lychee";
        let expected: Vec<String> = {
            let store = lock_store(&state);
            let q_emb = WordEmbedder.embed_query(query).unwrap();
            let hits = store.search_hybrid(query, &q_emb, 5).unwrap();
            assert_eq!(hits.len(), 5);
            assert!(
                hits.windows(2).all(|w| w[0].1 > w[1].1),
                "test data must not produce tied scores"
            );
            hits.into_iter().map(|(m, _)| m.id).collect()
        };

        let req: RecallReq = serde_json::from_value(json!({"query": query})).unwrap();
        assert!(req.engine.is_none() && req.max_tokens.is_none() && req.now.is_none());
        let got: Vec<String> =
            run_recall_with(&state, &req, RecallEngine::Legacy, OutputFormat::Json)
                .unwrap()
                .into_iter()
                .map(|(m, _)| m.id)
                .collect();
        assert_eq!(got, expected);

        // Through the handler, selected by the request: explicit wins over
        // any environment value.
        let (status, body) = post_recall(&state, json!({"query": query, "engine": "legacy"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids(&rows(&body)), expected);
    }

    /// A request that names no engine now runs v2. Visible on a count the
    /// legacy engine cannot reach: its candidate pool stops at 40.
    #[tokio::test]
    async fn recall_default_engine_is_v2() {
        if !recall_env_is_clean() {
            return;
        }
        let state = seeded_state();
        let (status, body) = post_recall(&state, json!({"query": "deploy", "limit": 100})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(rows(&body).len(), 100);

        let (_, body) = post_recall(
            &state,
            json!({"query": "deploy", "limit": 100, "engine": "legacy"}),
        )
        .await;
        assert!(rows(&body).len() <= 40, "{}", rows(&body).len());
    }

    /// The default engine keeps the response contract: default limit 5,
    /// ceiling 100 without a budget, the same fields, and a score only
    /// when the server has an embedder.
    #[tokio::test]
    async fn recall_default_engine_keeps_the_response_shape() {
        if !recall_env_is_clean() {
            return;
        }
        let state = seeded_state();
        let (_, body) = post_recall(&state, json!({"query": "deploy"})).await;
        assert_eq!(rows(&body).len(), 5);
        let (_, body) = post_recall(&state, json!({"query": "deploy", "limit": 400})).await;
        assert_eq!(rows(&body).len(), 100);
        // A budget raises the ceiling.
        let (_, body) = post_recall(
            &state,
            json!({"query": "deploy", "limit": 400, "max_tokens": 1_000_000}),
        )
        .await;
        assert_eq!(rows(&body).len(), 120);

        let keys = |body: &str| -> Vec<String> {
            let mut keys: Vec<String> =
                rows(body)[0].as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let header = |body: &str| body.lines().next().unwrap_or_default().to_string();

        // With an embedder: same JSON fields and same TOON header as legacy.
        let v2 = json!({"query": "deploy release"});
        let legacy = json!({"query": "deploy release", "engine": "legacy"});
        let (_, v2_json) = post_recall(&state, v2.clone()).await;
        let (_, legacy_json) = post_recall(&state, legacy.clone()).await;
        assert_eq!(keys(&v2_json), keys(&legacy_json));
        assert!(keys(&v2_json).contains(&"score".to_string()));
        let (_, v2_toon) = post_recall_as(&state, v2.clone(), "toon").await;
        let (_, legacy_toon) = post_recall_as(&state, legacy.clone(), "toon").await;
        assert_eq!(header(&v2_toon), header(&legacy_toon));
        assert_eq!(
            header(&v2_toon),
            "memories[5]{score,id,topic,importance,weight,summary}:"
        );

        // Keyword-only server: no score on either engine.
        let store = Store::in_memory().unwrap();
        for i in 0..8 {
            store
                .store(Memory::new(
                    "notes".into(),
                    format!("deploy release note {i}"),
                    Importance::Medium,
                ))
                .unwrap();
        }
        let state = test_state(store, false);
        let (_, v2_json) = post_recall(&state, v2.clone()).await;
        let (_, legacy_json) = post_recall(&state, legacy.clone()).await;
        assert_eq!(keys(&v2_json), keys(&legacy_json));
        assert!(!keys(&v2_json).contains(&"score".to_string()));
        let (_, v2_toon) = post_recall_as(&state, v2, "toon").await;
        let (_, legacy_toon) = post_recall_as(&state, legacy, "toon").await;
        assert_eq!(header(&v2_toon), header(&legacy_toon));
        assert_eq!(
            header(&v2_toon),
            "memories[5]{id,topic,importance,weight,summary}:"
        );
    }

    #[tokio::test]
    async fn recall_v2_with_no_match_keeps_the_plain_message() {
        // Keyword-only: with an embedder the vector arm always has
        // nearest neighbors to offer, so the list is never empty.
        let store = Store::in_memory().unwrap();
        store
            .store(Memory::new(
                "notes".into(),
                "deploy checklist".into(),
                Importance::Medium,
            ))
            .unwrap();
        let state = test_state(store, false);
        let (status, body) = post_recall(&state, json!({"query": "zebra", "engine": "v2"})).await;
        assert_eq!(status, StatusCode::OK);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["message"], MSG_NO_MEMORIES);
    }

    #[tokio::test]
    async fn recall_rejects_unknown_engine_and_invalid_now() {
        let state = seeded_state();
        let (status, body) =
            post_recall(&state, json!({"query": "deploy", "engine": "turbo"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("turbo"), "{body}");

        let (status, body) = post_recall(
            &state,
            json!({"query": "deploy", "engine": "v2", "now": "last tuesday"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("now"), "{body}");

        let (status, _) = post_recall(&state, json!({"query": "  ", "engine": "v2"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// `now` anchors relative dates: the memory dated the day before the
    /// anchor comes first for a "yesterday" query.
    #[tokio::test]
    async fn store_created_at_and_recall_now_drive_the_time_window() {
        let state = test_state(Store::in_memory_with_dims(TEST_DIMS).unwrap(), false);
        for i in 0..6 {
            let (status, _) = post_store(
                &state,
                json!({"topic": "notes", "content": format!("standup summary number {i}")}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, body) = post_store(
            &state,
            json!({
                "topic": "notes",
                "content": "standup summary about the outage",
                "created_at": "2023-05-09T09:00:00Z",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let dated_id = ids(&rows(&body)).remove(0);

        let (status, body) = post_recall(
            &state,
            json!({
                "query": "standup summary yesterday",
                "engine": "v2",
                "limit": 10,
                "now": "2023-05-10T12:00:00Z",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(ids(&rows(&body))[0], dated_id);
    }

    #[tokio::test]
    async fn store_with_created_at_is_read_back_from_the_store() {
        use chrono::{TimeZone, Utc};

        let state = test_state(Store::in_memory().unwrap(), false);
        let before = Utc::now();

        // RFC 3339 with an offset is converted to UTC.
        let (status, body) = post_store(
            &state,
            json!({
                "topic": "conversations",
                "content": "Caroline went to the support group",
                "created_at": "2023-05-08T13:56:00+02:00",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let returned = rows(&body);
        let id = returned[0]["id"].as_str().unwrap();

        let stored = lock_store(&state).get(id).unwrap().unwrap();
        assert_eq!(
            stored.created_at,
            Utc.with_ymd_and_hms(2023, 5, 8, 11, 56, 0).unwrap()
        );
        assert!(stored.updated_at >= before);
        assert!(stored.last_accessed >= before);

        // Date only: midnight UTC.
        let (_, body) = post_store(
            &state,
            json!({"topic": "conversations", "content": "second", "created_at": "2023-05-09"}),
        )
        .await;
        let returned = rows(&body);
        let id = returned[0]["id"].as_str().unwrap();
        let stored = lock_store(&state).get(id).unwrap().unwrap();
        assert_eq!(
            stored.created_at,
            Utc.with_ymd_and_hms(2023, 5, 9, 0, 0, 0).unwrap()
        );

        // Omitted: the time of the write, as before.
        let (_, body) = post_store(
            &state,
            json!({"topic": "conversations", "content": "third"}),
        )
        .await;
        let returned = rows(&body);
        let id = returned[0]["id"].as_str().unwrap();
        let stored = lock_store(&state).get(id).unwrap().unwrap();
        assert!(stored.created_at >= before);
    }

    #[tokio::test]
    async fn store_rejects_invalid_created_at_without_writing() {
        let state = test_state(Store::in_memory().unwrap(), false);
        let (status, body) = post_store(
            &state,
            json!({"topic": "conversations", "content": "x", "created_at": "8 May 2023"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("created_at"), "{body}");
        assert_eq!(lock_store(&state).count().unwrap(), 0);
    }

    #[test]
    fn mcp_proxy_endpoint_points_at_mcp_route_with_compact_flag() {
        assert_eq!(
            mcp_proxy_endpoint("http://127.0.0.1:11435", true).unwrap(),
            "http://127.0.0.1:11435/mcp?compact=true"
        );
        assert_eq!(
            mcp_proxy_endpoint("http://127.0.0.1:11435/mcp", false).unwrap(),
            "http://127.0.0.1:11435/mcp?compact=false"
        );
        assert_eq!(
            mcp_proxy_endpoint("http://127.0.0.1:11435/", false).unwrap(),
            "http://127.0.0.1:11435/mcp?compact=false"
        );
    }
}
