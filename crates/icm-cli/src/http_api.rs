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
//! Bearer <token>` checking; absent token = open localhost API.

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

use icm_core::{
    is_preference_topic, keyword_matches, project_matches, topic_matches, Embedder, Importance,
    Memory, MemoryStore, MSG_NO_MEMORIES,
};
use icm_store::Store;

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
    /// When set, every request must carry `Authorization: Bearer <token>`.
    token: Option<String>,
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
}

#[derive(Debug, Deserialize)]
pub struct ConsolidateReq {
    topic: String,
    #[serde(default)]
    keep_originals: bool,
}

// ---------------------------------------------------------------------------
// Server entry
// ---------------------------------------------------------------------------

/// Run the HTTP server until it's interrupted. Loads NOTHING beyond
/// what the caller has already loaded — the warm store and embedder
/// are pre-built by `cmd_serve` and handed to us as `Arc`s.
#[tokio::main]
pub async fn run_http_server(
    store: Store,
    embedder: Option<Box<dyn Embedder + Send + Sync>>,
    addr: SocketAddr,
    token: Option<String>,
) -> Result<()> {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        embedder: embedder.map(Arc::from),
        token,
    };

    let app = Router::new()
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
        Some(tok) if tok == expected => next.run(request).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "missing or invalid Bearer token\n",
        )
            .into_response(),
    }
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
    match run_recall(&state, &req) {
        Ok(results) => render_recall(&results, format),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e.to_string(), format),
    }
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
    let store = state
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("store poisoned"))?;
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

    let mut mem = Memory::new(req.topic.clone(), req.content.clone(), importance);
    mem.keywords = keywords;
    if let Some(raw) = req.raw.as_deref().filter(|s| !s.is_empty()) {
        mem.raw_excerpt = Some(raw.to_string());
    }
    if let Some(emb) = state.embedder_ref() {
        if let Ok(v) = emb.embed(&format!("{} {}", mem.topic, mem.summary)) {
            mem.embedding = Some(v);
        }
    }

    let outcome = match state.store.lock() {
        Ok(store) => store.store(mem.clone()),
        Err(_) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, "store poisoned", format),
    };
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
    let store = match state.store.lock() {
        Ok(s) => s,
        Err(_) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, "store poisoned", format),
    };
    let topic_memories = match store.get_by_topic(&req.topic) {
        Ok(ms) => ms,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("topic lookup failed: {e}"),
                format,
            )
        }
    };
    if topic_memories.is_empty() {
        return err_response(
            StatusCode::NOT_FOUND,
            &format!("no memories under topic {:?}", req.topic),
            format,
        );
    }
    let summary = topic_memories
        .iter()
        .map(|m| m.summary.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    let consolidated = Memory::new(req.topic.clone(), summary, Importance::High);

    let result = if req.keep_originals {
        store.store(consolidated.clone()).map(|_| ())
    } else {
        store.consolidate_topic(&req.topic, consolidated.clone())
    };
    match result {
        Ok(()) => render_recall(&[(consolidated, None)], format),
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
    let store = match state.store.lock() {
        Ok(s) => s,
        Err(_) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, "store poisoned", format),
    };
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
    let store = match state.store.lock() {
        Ok(s) => s,
        Err(_) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, "store poisoned", format),
    };
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
}
