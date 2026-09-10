//! Web dashboard for ICM — Axum HTTP server with embedded SvelteKit SPA.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};

use icm_core::{FeedbackStore, Importance, MemoirStore, MemoryStore};
use icm_store::Store;

use crate::cloud::write_secret_file;

use crate::config::WebConfig;
use crate::graph_layout;
use crate::truncate_at_char_boundary;

// ---------------------------------------------------------------------------
// Embedded SPA assets (compiled SvelteKit output)
// ---------------------------------------------------------------------------

#[derive(Embed)]
#[folder = "web/dist/"]
struct WebAssets;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
    username: String,
    password: String,
}

// ---------------------------------------------------------------------------
// Password resolution
// ---------------------------------------------------------------------------

/// Resolve the web dashboard password.
/// Priority: ICM_WEB_PASSWORD env > config.toml [web].password > auto-generate.
pub fn resolve_password(cfg: &WebConfig) -> Result<String> {
    // 1. Environment variable
    if let Ok(p) = std::env::var("ICM_WEB_PASSWORD") {
        if !p.is_empty() {
            return Ok(p);
        }
    }

    // 2. Config file
    if !cfg.password.is_empty() {
        return Ok(cfg.password.clone());
    }

    // 3. Credentials file
    let cred_path = credentials_path();
    if let Some(ref path) = cred_path {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    if let Some(val) = line.strip_prefix("ICM_WEB_PASSWORD=") {
                        if !val.is_empty() {
                            return Ok(val.to_string());
                        }
                    }
                }
            }
        }
    }

    // 4. Auto-generate
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf)
        .map_err(|e| anyhow::anyhow!("failed to generate password: {e}"))?;
    let generated: String = buf.iter().map(|b| format!("{b:02x}")).collect();

    // Save to credentials file. Owner-only (0600) from the moment the file
    // is created on Unix — the prior fs::write-then-set_permissions left a
    // window where the freshly-generated dashboard password was readable
    // under the process umask (often world-readable) before the chmod ran
    // (audit finding, same TOCTOU class already fixed in cloud.rs).
    if let Some(ref path) = cred_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let entry = format!("ICM_WEB_PASSWORD={generated}\n");
        let _ = write_secret_file(path, &entry);
    }

    // Don't print the password to stderr — it would land in CI logs, shell
    // history, and `script(1)` recordings. Point the user at the 0600 file
    // instead. If credentials_path() failed (no project dir), surface a
    // single fallback line so the user still knows where to retrieve it.
    match cred_path {
        Some(path) => eprintln!(
            "[icm web] Generated admin password (saved to {}). Run `cat {}` to read it.",
            path.display(),
            path.display()
        ),
        None => eprintln!(
            "[icm web] Generated admin password — set ICM_WEB_PASSWORD or [web] password in config to control it."
        ),
    }
    Ok(generated)
}

fn credentials_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "icm", "icm")
        .map(|dirs| dirs.config_dir().join("credentials"))
}

// ---------------------------------------------------------------------------
// Basic Auth middleware
// ---------------------------------------------------------------------------

/// Mutating dashboard routes that take no request body: `/api/health/decay`,
/// `/api/health/prune`, `/api/topics/{name}/consolidate`. Basic Auth
/// credentials are auto-reattached by the browser cross-origin (unlike a
/// Bearer token, which JS must explicitly set), so a plain HTML form on
/// another origin can POST to these without any preflight and ride the
/// victim's cached credentials — CSRF (audit finding). `DELETE
/// /api/memories/{id}` isn't exploitable this way: a non-simple method
/// triggers a CORS preflight, which fails here since no CorsLayer is
/// mounted.
fn is_csrf_sensitive_post(method: &Method, path: &str) -> bool {
    if method != Method::POST {
        return false;
    }
    path == "/api/health/decay"
        || path == "/api/health/prune"
        || (path.starts_with("/api/topics/") && path.ends_with("/consolidate"))
}

/// Host portion of an `Origin`/`Referer` header value (strips scheme and
/// any path/port-following segments left by a naive strip).
fn header_host(v: &str) -> Option<&str> {
    v.strip_prefix("http://")
        .or_else(|| v.strip_prefix("https://"))
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

/// Whether this request is same-origin, judged from `Origin` (preferred) or
/// `Referer` (fallback — some browsers omit `Origin` on same-origin POSTs).
/// Browsers always attach at least one of these on a cross-origin POST, so
/// requests carrying neither are treated as a non-browser client (curl,
/// scripts) using Basic Auth directly, not a forged browser request, and are
/// allowed through.
fn is_same_origin(req: &Request<Body>) -> bool {
    let Some(host) = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        return header_host(origin) == Some(host);
    }
    if let Some(referer) = req
        .headers()
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
    {
        return header_host(referer) == Some(host);
    }
    true
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // /healthz is public (liveness probe)
    if req.uri().path() == "/healthz" {
        return next.run(req).await;
    }

    // Basic Auth credentials are auto-reattached by the browser cross-origin
    // (unlike a Bearer token, which JS must explicitly set), so a plain HTML
    // form on another origin can POST to these mutating, bodyless endpoints
    // without any preflight and ride the victim's cached credentials — CSRF
    // (audit finding). `DELETE /api/memories/{id}` isn't exploitable this
    // way: a non-simple method triggers a CORS preflight, which fails here
    // since no CorsLayer is mounted.
    if is_csrf_sensitive_post(req.method(), req.uri().path()) && !is_same_origin(&req) {
        return (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response();
    }

    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| {
            let decoded = base64_decode(b64)?;
            let s = String::from_utf8(decoded).ok()?;
            let (user, pass) = s.split_once(':')?;
            // Constant-time compare — a naive `==` leaks a timing
            // side-channel for brute-forcing the dashboard password
            // (audit finding).
            Some(
                constant_time_eq(user.as_bytes(), state.username.as_bytes())
                    && constant_time_eq(pass.as_bytes(), state.password.as_bytes()),
            )
        })
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"icm\"")],
            "Unauthorized",
        )
            .into_response()
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

/// Simple base64 decode (avoid pulling in a full crate).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input.as_bytes() {
        if b == b'=' {
            break;
        }
        let val = TABLE.iter().position(|&c| c == b)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

fn api_router() -> Router<AppState> {
    Router::new()
        // Overview
        .route("/api/stats", get(api_stats))
        // Topics
        .route("/api/topics", get(api_topics))
        .route("/api/topics/{name}", get(api_topic_detail))
        .route("/api/topics/{name}/health", get(api_topic_health))
        .route(
            "/api/topics/{name}/consolidate",
            post(api_topic_consolidate),
        )
        // Memories
        .route("/api/memories", get(api_memories))
        .route("/api/memories/search", get(api_memories_search))
        .route("/api/memories/{id}", delete(api_memory_delete))
        // Graph (memory-relationship view; issue: visualize auto_link.rs's
        // related_ids graph, which existed only as unused data before this)
        .route("/api/graph", get(api_graph))
        // Health
        .route("/api/health", get(api_health_all))
        .route("/api/health/decay", post(api_decay))
        .route("/api/health/prune", post(api_prune))
        // Memoirs
        .route("/api/memoirs", get(api_memoirs))
        .route("/api/memoirs/{id}", get(api_memoir_detail))
        // Public. NOT "/health" — the SvelteKit dashboard has its own
        // page at that exact path (routes/health/+page.svelte), and
        // axum matches an exact route before ever falling through to the
        // SPA's catch-all: a bare GET /health (a page reload, a
        // bookmark, a shared link) hit this liveness JSON instead of the
        // dashboard page, with no error and no visible sign anything was
        // wrong (audit finding from a UX review).
        .route("/healthz", get(api_health_check))
}

fn spa_router() -> Router<AppState> {
    Router::new()
        .route("/", get(serve_index))
        .fallback(serve_static)
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Builds the full router: API (auth-gated) merged with the SPA shell
/// (public). Split out from [`run_web_server`] so a test can exercise it
/// with `tower::ServiceExt::oneshot` without binding a real TCP listener.
///
/// Auth wraps ONLY the API sub-router, not the merged whole. This used
/// to wrap the merged app, which meant the SPA shell itself — the
/// static HTML/JS/CSS, including the custom /login page's own assets —
/// required a valid `Authorization: Basic` header before axum would
/// serve so much as a byte of it. A raw `curl /login` against the real
/// binary (not the Vite dev proxy, which never exercises this) 401'd:
/// in a real browser, a first-ever visit with no cached credentials
/// for this origin hits that 401 (which carries `WWW-Authenticate:
/// Basic`) on a top-level navigation, which every browser answers with
/// its own native credential prompt — before any SvelteKit JS, and
/// therefore the custom login page it was built to replace, ever runs
/// (audit finding from a UX review; the bug predates this fix and was
/// invisible all session because testing went through Vite's dev
/// proxy, which only forwards /api/* and doesn't reproduce this gate
/// on other paths). Static shell content isn't sensitive on its own —
/// gating only the API, which is what actually returns memory data,
/// is the standard SPA security model and is what the client's
/// `api.ts` already assumes (it attaches the header itself and
/// redirects to /login on a real 401).
fn build_app(state: AppState) -> Router {
    let api = api_router().layer(middleware::from_fn_with_state(
        state.clone(),
        auth_middleware,
    ));
    api.merge(spa_router()).with_state(state)
}

#[tokio::main]
pub async fn run_web_server(
    store: Store,
    host: &str,
    port: u16,
    username: String,
    password: String,
) -> Result<()> {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        username,
        password,
    };
    let app = build_app(state);

    let bind = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {bind}: {e}"))?;

    eprintln!("[icm web] Dashboard running on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SPA handlers
// ---------------------------------------------------------------------------

async fn serve_index() -> impl IntoResponse {
    match WebAssets::get("index.html") {
        Some(content) => Html(String::from_utf8_lossy(content.data.as_ref()).to_string())
            .into_response(),
        None => Html(
            "<h1>ICM Dashboard</h1><p>Frontend not built. Run <code>cd web && bun run build</code></p>"
                .to_string(),
        )
        .into_response(),
    }
}

async fn serve_static(req: Request<Body>) -> impl IntoResponse {
    let path = req.uri().path().trim_start_matches('/');

    // Try exact file match
    if let Some(content) = WebAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref().to_string())],
            content.data.to_vec(),
        )
            .into_response();
    }

    // SPA fallback: serve index.html for client-side routing
    match WebAssets::get("index.html") {
        Some(content) => {
            Html(String::from_utf8_lossy(content.data.as_ref()).to_string()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StatsResponse {
    total_memories: usize,
    total_topics: usize,
    avg_weight: f32,
    oldest_memory: Option<String>,
    newest_memory: Option<String>,
    total_memoirs: usize,
    total_concepts: usize,
    total_links: usize,
    total_feedback: usize,
}

#[derive(Serialize)]
struct TopicEntry {
    name: String,
    count: usize,
}

#[derive(Serialize)]
struct MemoirEntry {
    id: String,
    name: String,
    description: String,
    concepts: usize,
    links: usize,
}

#[derive(Deserialize)]
struct PaginationParams {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize {
    50
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

fn default_search_limit() -> usize {
    20
}

#[derive(Serialize)]
struct ActionResult {
    ok: bool,
    message: String,
}

#[derive(Deserialize)]
struct GraphParams {
    /// Restrict the graph to one topic. Without it, every memory is a node
    /// (auto_link.rs only links within reasonable similarity anyway, so
    /// most graphs are naturally sparse even unfiltered).
    topic: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
struct GraphNode {
    id: String,
    topic: String,
    importance: &'static str,
    weight: f32,
    summary: String,
    /// Pre-computed layout position (see `graph_layout::compute_force_layout_3d`).
    /// Computed here rather than in the browser: tested against a real
    /// 3286-memory store, client-side JS physics needed ~5.4M pairwise
    /// force calculations *per animation frame* and never visibly
    /// converged. The client just renders these positions directly.
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Serialize, Debug, PartialEq)]
struct GraphEdge {
    source: String,
    target: String,
    /// Cosine similarity between the two memories' embeddings, recomputed
    /// here rather than stored at link time (auto_link.rs only persists
    /// which ids are related, not the score) — always reflects the current
    /// embeddings, and needs no schema/migration to add.
    similarity: f32,
}

/// Cosine similarity between two embedding vectors. `None` if either is
/// missing/empty or they don't (which shouldn't happen for two memories
/// produced by the same embedder, but a mismatched length would panic on
/// the zip otherwise) share a dimension.
fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return None;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    Some(dot / (norm_a * norm_b))
}

#[derive(Serialize, Debug, PartialEq)]
struct GraphResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

fn importance_str(imp: Importance) -> &'static str {
    match imp {
        Importance::Critical => "critical",
        Importance::High => "high",
        Importance::Medium => "medium",
        Importance::Low => "low",
    }
}

/// Lock the store, recovering from a poisoned mutex. The store keeps its own
/// consistency through SQLite transactions, so a panic in one handler must
/// not permanently kill every subsequent request — with a plain `unwrap()`,
/// one poisoned lock cascaded panics across all handlers for the lifetime of
/// the process (audit finding; also the largest cluster of forbidden
/// `unwrap()` calls in the repo).
fn lock_store(state: &AppState) -> std::sync::MutexGuard<'_, Store> {
    state
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

async fn api_health_check() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn api_stats(State(state): State<AppState>) -> impl IntoResponse {
    let store = lock_store(&state);
    let stats = match store.stats() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let feedback_count = store.feedback_stats().map(|f| f.total).unwrap_or(0);

    // Count memoirs, concepts, links
    let memoirs = store.list_memoirs().unwrap_or_default();
    let (mut concepts, mut links) = (0usize, 0usize);
    for m in &memoirs {
        if let Ok(ms) = store.memoir_stats(&m.id) {
            concepts += ms.total_concepts;
            links += ms.total_links;
        }
    }

    Json(StatsResponse {
        total_memories: stats.total_memories,
        total_topics: stats.total_topics,
        avg_weight: stats.avg_weight,
        oldest_memory: stats.oldest_memory.map(|d| d.to_rfc3339()),
        newest_memory: stats.newest_memory.map(|d| d.to_rfc3339()),
        total_memoirs: memoirs.len(),
        total_concepts: concepts,
        total_links: links,
        total_feedback: feedback_count,
    })
    .into_response()
}

async fn api_topics(State(state): State<AppState>) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.list_topics() {
        Ok(topics) => Json(
            topics
                .into_iter()
                .map(|(name, count)| TopicEntry { name, count })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_topic_detail(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.get_by_topic(&name) {
        Ok(memories) => Json(memories).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_topic_health(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.topic_health(&name) {
        Ok(health) => Json(health).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_topic_consolidate(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    let memories = match store.get_by_topic(&name) {
        Ok(m) => m,
        Err(e) => {
            return Json(ActionResult {
                ok: false,
                message: e.to_string(),
            })
            .into_response()
        }
    };

    if memories.is_empty() {
        return Json(ActionResult {
            ok: false,
            message: "No memories in topic".into(),
        })
        .into_response();
    }

    // Build consolidated summary
    let summary: String = memories
        .iter()
        .map(|m| m.summary.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    let truncated = if summary.len() > 500 {
        format!("{}...", truncate_at_char_boundary(&summary, 500))
    } else {
        summary
    };

    let mut consolidated = memories[0].clone();
    consolidated.id = format!(
        "{:032X}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    consolidated.summary = truncated;
    consolidated.access_count = 0;
    consolidated.weight = 1.0;

    match store.consolidate_topic(&name, consolidated) {
        Ok(_) => Json(ActionResult {
            ok: true,
            message: format!("Consolidated {} memories", memories.len()),
        })
        .into_response(),
        Err(e) => Json(ActionResult {
            ok: false,
            message: e.to_string(),
        })
        .into_response(),
    }
}

async fn api_memories(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.list_all() {
        Ok(mut memories) => {
            memories.sort_by(|a, b| {
                b.weight
                    .partial_cmp(&a.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let page: Vec<_> = memories
                .into_iter()
                .skip(params.offset)
                .take(params.limit)
                .collect();
            Json(page).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_memories_search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.search_fts(&params.q, params.limit) {
        Ok(memories) => Json(memories).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Nodes + edges for the memory-relationship graph. `related_ids` (populated
/// by `auto_link.rs` at store time from embedding cosine similarity) is the
/// edge data; this endpoint is the first thing that actually exposes it —
/// previously it only fed `main.rs`'s one-hop "graph-aware expansion"
/// during recall, with no way to see the graph itself.
async fn api_graph(
    State(state): State<AppState>,
    Query(params): Query<GraphParams>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    let memories = match &params.topic {
        Some(t) => store.get_by_topic(t),
        None => store.list_all(),
    };
    match memories {
        Ok(m) => Json(build_graph_response(&m)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Pure node/edge-building logic, split out from [`api_graph`] so it's
/// testable without spinning up an async handler + `AppState`.
fn build_graph_response(memories: &[icm_core::Memory]) -> GraphResponse {
    let ids: std::collections::HashSet<&str> = memories.iter().map(|m| m.id.as_str()).collect();
    let embeddings: std::collections::HashMap<&str, &[f32]> = memories
        .iter()
        .filter_map(|m| m.embedding.as_deref().map(|e| (m.id.as_str(), e)))
        .collect();
    let index_of: std::collections::HashMap<&str, usize> = memories
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.as_str(), i))
        .collect();

    // auto_link.rs adds backrefs (a links to b => b also links back to a),
    // so dedupe to one undirected edge per pair. Drop any related_id that
    // points outside the current (possibly topic-filtered) node set rather
    // than erroring — a dangling/foreign reference isn't this endpoint's
    // problem to fail on.
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    let mut index_edges = Vec::new();
    for m in memories {
        for related in &m.related_ids {
            if !ids.contains(related.as_str()) || related == &m.id {
                continue;
            }
            let key = if m.id < *related {
                (m.id.clone(), related.clone())
            } else {
                (related.clone(), m.id.clone())
            };
            if seen.insert(key.clone()) {
                let similarity = match (
                    embeddings.get(key.0.as_str()),
                    embeddings.get(key.1.as_str()),
                ) {
                    (Some(a), Some(b)) => cosine_similarity(a, b).unwrap_or(0.0),
                    // Missing embeddings (e.g. --no-embeddings stores) means
                    // there's no real score to show; 0.0 reads as "unknown"
                    // rather than a false "identical" (1.0) or "unrelated".
                    _ => 0.0,
                };
                if let (Some(&a), Some(&b)) =
                    (index_of.get(key.0.as_str()), index_of.get(key.1.as_str()))
                {
                    index_edges.push((a, b));
                }
                edges.push(GraphEdge {
                    source: key.0,
                    target: key.1,
                    similarity,
                });
            }
        }
    }

    let node_ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();
    // Dense 0..k topic index per node, assigned in first-appearance order
    // (order doesn't matter to the layout — anchors are placed on a
    // sphere regardless of index — only that same-topic nodes share an
    // index). Drives the layout's topic-clustering force: see
    // `compute_force_layout_3d`'s doc comment for why plain repulsion
    // alone produces an unreadable hollow-sphere shape at real-store
    // scale.
    let mut topic_index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let clusters: Vec<usize> = memories
        .iter()
        .map(|m| {
            let next = topic_index.len();
            *topic_index.entry(m.topic.as_str()).or_insert(next)
        })
        .collect();
    // 200 iterations is the same budget the TUI's 2D layout uses — enough
    // for ALPHA_DECAY to reach a negligible temperature (see the constant's
    // doc comment) regardless of node count, since it's a fixed schedule,
    // not a convergence check that could loop indefinitely.
    let positions = graph_layout::compute_force_layout_3d(&node_ids, &index_edges, &clusters, 200);

    let nodes: Vec<GraphNode> = memories
        .iter()
        .map(|m| {
            let (x, y, z) = positions.get(&m.id).copied().unwrap_or((0.0, 0.0, 0.0));
            GraphNode {
                id: m.id.clone(),
                topic: m.topic.clone(),
                importance: importance_str(m.importance),
                weight: m.weight,
                summary: truncate_at_char_boundary(&m.summary, 80).to_string(),
                x,
                y,
                z,
            }
        })
        .collect();

    GraphResponse { nodes, edges }
}

async fn api_memory_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.delete(&id) {
        Ok(_) => Json(ActionResult {
            ok: true,
            message: format!("Deleted {id}"),
        })
        .into_response(),
        Err(e) => Json(ActionResult {
            ok: false,
            message: e.to_string(),
        })
        .into_response(),
    }
}

async fn api_health_all(State(state): State<AppState>) -> impl IntoResponse {
    let store = lock_store(&state);
    let topics = match store.list_topics() {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let mut health_list = Vec::new();
    for (name, _) in &topics {
        if let Ok(h) = store.topic_health(name) {
            health_list.push(h);
        }
    }

    Json(health_list).into_response()
}

async fn api_decay(State(state): State<AppState>) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.apply_decay(0.95) {
        Ok(n) => Json(ActionResult {
            ok: true,
            message: format!("Decayed {n} memories"),
        }),
        Err(e) => Json(ActionResult {
            ok: false,
            message: e.to_string(),
        }),
    }
}

async fn api_prune(State(state): State<AppState>) -> impl IntoResponse {
    let store = lock_store(&state);
    match store.prune(0.1) {
        Ok(n) => Json(ActionResult {
            ok: true,
            message: format!("Pruned {n} memories"),
        }),
        Err(e) => Json(ActionResult {
            ok: false,
            message: e.to_string(),
        }),
    }
}

async fn api_memoirs(State(state): State<AppState>) -> impl IntoResponse {
    let store = lock_store(&state);
    let memoirs = match store.list_memoirs() {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let entries: Vec<MemoirEntry> = memoirs
        .into_iter()
        .map(|m| {
            let ms = store.memoir_stats(&m.id);
            let (concepts, links) = ms
                .map(|s| (s.total_concepts, s.total_links))
                .unwrap_or((0, 0));
            MemoirEntry {
                id: m.id,
                name: m.name,
                description: m.description,
                concepts,
                links,
            }
        })
        .collect();

    Json(entries).into_response()
}

async fn api_memoir_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = lock_store(&state);
    let memoir = match store.get_memoir(&id) {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "Memoir not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let concepts = store.list_concepts(&id).unwrap_or_default();
    let links = store.get_links_for_memoir(&id).unwrap_or_default();

    Json(serde_json::json!({
        "memoir": memoir,
        "concepts": concepts,
        "links": links,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            store: Arc::new(Mutex::new(Store::in_memory().unwrap())),
            username: "admin".into(),
            password: "secret".into(),
        }
    }

    /// Regression for a real bug found by a UX review, not introduced by
    /// this test: auth used to wrap the merged app (API + SPA shell), so
    /// the SPA's own static HTML/JS/CSS — including the custom /login
    /// page's assets — required valid Basic Auth credentials before axum
    /// would serve any of it. A first-ever browser visit with no cached
    /// credentials for the origin hits that 401 on a top-level
    /// navigation, which every browser answers with its own native
    /// credential prompt, before the custom login page (built
    /// specifically to avoid that prompt) ever has a chance to run. The
    /// SPA shell must be reachable with no `Authorization` header at all;
    /// only the API — which is what actually returns memory data — stays
    /// gated.
    #[tokio::test]
    async fn spa_shell_is_public_but_api_requires_auth() {
        let app = build_app(test_state());

        let unauthenticated_get =
            |path: &str| Request::builder().uri(path).body(Body::empty()).unwrap();

        for path in ["/", "/login", "/graph", "/healthz"] {
            let res = app
                .clone()
                .oneshot(unauthenticated_get(path))
                .await
                .unwrap();
            assert_ne!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "{path} must be reachable without credentials (SPA shell / public liveness check)"
            );
        }

        let res = app
            .clone()
            .oneshot(unauthenticated_get("/api/stats"))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "/api/* must still require credentials"
        );
    }

    /// Audit regression: a panic inside one handler used to poison the store
    /// mutex, making every subsequent `lock().unwrap()` panic in cascade for
    /// the lifetime of the process. `lock_store` must recover the guard.
    #[test]
    fn lock_store_recovers_from_a_poisoned_mutex() {
        let state = AppState {
            store: Arc::new(Mutex::new(Store::in_memory().unwrap())),
            username: "u".into(),
            password: "p".into(),
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

    #[test]
    fn is_csrf_sensitive_post_matches_only_the_mutating_bodyless_routes() {
        assert!(is_csrf_sensitive_post(&Method::POST, "/api/health/decay"));
        assert!(is_csrf_sensitive_post(&Method::POST, "/api/health/prune"));
        assert!(is_csrf_sensitive_post(
            &Method::POST,
            "/api/topics/my-topic/consolidate"
        ));
        // Different method or path: not covered by this check.
        assert!(!is_csrf_sensitive_post(&Method::GET, "/api/health/decay"));
        assert!(!is_csrf_sensitive_post(&Method::POST, "/api/memories"));
        assert!(!is_csrf_sensitive_post(
            &Method::DELETE,
            "/api/memories/abc"
        ));
    }

    /// Audit regression: `api_topic_consolidate`/`api_decay`/`api_prune`
    /// took no request body, so Basic Auth's browser-side credential
    /// auto-reattachment let a plain cross-origin HTML form POST to them
    /// and ride the victim's cached credentials (CSRF) — these three POSTs
    /// don't trigger a CORS preflight, unlike the DELETE route.
    #[test]
    fn is_same_origin_rejects_cross_origin_and_accepts_matching_or_absent() {
        let req = |origin: Option<&str>, referer: Option<&str>| {
            let mut b = Request::builder()
                .method(Method::POST)
                .header(header::HOST, "127.0.0.1:8787");
            if let Some(o) = origin {
                b = b.header(header::ORIGIN, o);
            }
            if let Some(r) = referer {
                b = b.header(header::REFERER, r);
            }
            b.body(Body::empty()).unwrap()
        };

        // A forged cross-origin form POST carries an Origin that doesn't
        // match Host — must be rejected.
        assert!(!is_same_origin(&req(Some("http://evil.example"), None)));
        // Legit same-origin fetch() from the dashboard itself.
        assert!(is_same_origin(&req(Some("http://127.0.0.1:8787"), None)));
        // Some browsers omit Origin on same-origin POSTs; Referer must
        // still be checked and matched.
        assert!(is_same_origin(&req(
            None,
            Some("http://127.0.0.1:8787/dashboard")
        )));
        assert!(!is_same_origin(&req(
            None,
            Some("http://evil.example/lure")
        )));
        // Neither header: a non-browser client (curl/scripts) using Basic
        // Auth directly, not a forged browser request — allowed.
        assert!(is_same_origin(&req(None, None)));
    }

    fn mem(topic: &str, summary: &str, imp: Importance, related: &[&str]) -> icm_core::Memory {
        let mut m = icm_core::Memory::new(topic.into(), summary.into(), imp);
        m.related_ids = related.iter().map(|s| s.to_string()).collect();
        m
    }

    #[test]
    fn builds_one_node_per_memory_and_dedupes_backref_edges() {
        let a = mem("t", "alpha", Importance::High, &[]);
        let mut b = mem("t", "beta", Importance::Medium, &[]);
        // auto_link.rs links both directions: a -> b and b -> a.
        let mut a = a;
        a.related_ids.push(b.id.clone());
        b.related_ids.push(a.id.clone());
        let c = mem("t", "gamma", Importance::Low, &[]);

        let memories = vec![a.clone(), b.clone(), c.clone()];
        let graph = build_graph_response(&memories);

        assert_eq!(graph.nodes.len(), 3);
        assert!(graph
            .nodes
            .iter()
            .any(|n| n.id == a.id && n.importance == "high"));
        assert!(graph
            .nodes
            .iter()
            .any(|n| n.id == c.id && n.importance == "low"));

        // Exactly one edge for the a<->b pair, not two.
        assert_eq!(graph.edges.len(), 1);
        let edge = &graph.edges[0];
        let endpoints = [&edge.source, &edge.target];
        assert!(endpoints.contains(&&a.id));
        assert!(endpoints.contains(&&b.id));
    }

    #[test]
    fn edge_similarity_is_the_real_cosine_similarity_of_the_embeddings() {
        let mut a = mem("t", "alpha", Importance::High, &[]);
        let mut b = mem("t", "beta", Importance::Medium, &[]);
        a.embedding = Some(vec![1.0, 0.0]);
        b.embedding = Some(vec![1.0, 1.0]);
        a.related_ids.push(b.id.clone());
        b.related_ids.push(a.id.clone());

        let graph = build_graph_response(&[a, b]);

        assert_eq!(graph.edges.len(), 1);
        // cos(45 deg) between (1,0) and (1,1), normalized: 1/sqrt(2).
        let expected = 1.0 / std::f32::consts::SQRT_2;
        assert!(
            (graph.edges[0].similarity - expected).abs() < 1e-6,
            "expected {expected}, got {}",
            graph.edges[0].similarity
        );
    }

    #[test]
    fn edge_similarity_falls_back_to_zero_without_embeddings() {
        let mut a = mem("t", "alpha", Importance::High, &[]);
        let mut b = mem("t", "beta", Importance::Medium, &[]);
        a.related_ids.push(b.id.clone());
        b.related_ids.push(a.id.clone());
        let graph = build_graph_response(&[a, b]);
        assert_eq!(graph.edges[0].similarity, 0.0);
    }

    #[test]
    fn drops_related_ids_pointing_outside_the_node_set() {
        let a = mem("t", "alpha", Importance::High, &["does-not-exist"]);
        let graph = build_graph_response(&[a]);
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn ignores_a_self_referencing_related_id() {
        let mut a = mem("t", "alpha", Importance::High, &[]);
        let self_id = a.id.clone();
        a.related_ids.push(self_id);
        let graph = build_graph_response(&[a]);
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn empty_store_yields_an_empty_graph_not_an_error() {
        let graph = build_graph_response(&[]);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }
}
