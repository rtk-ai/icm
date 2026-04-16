//! Web dashboard for ICM — Axum HTTP server with embedded SvelteKit SPA.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};

use icm_core::{FeedbackStore, MemoirStore, MemoryStore};
use icm_store::SqliteStore;

use crate::config::WebConfig;
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
    store: Arc<Mutex<SqliteStore>>,
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

    // Save to credentials file
    if let Some(ref path) = cred_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let entry = format!("ICM_WEB_PASSWORD={generated}\n");
        std::fs::write(path, &entry).ok();
        // Restrict permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
    }

    eprintln!("[icm web] Generated admin password: {generated}");
    Ok(generated)
}

fn credentials_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "icm", "icm")
        .map(|dirs| dirs.config_dir().join("credentials"))
}

// ---------------------------------------------------------------------------
// Basic Auth middleware
// ---------------------------------------------------------------------------

async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // /health is public
    if req.uri().path() == "/health" {
        return next.run(req).await;
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
            Some(user == state.username && pass == state.password)
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
        // Health
        .route("/api/health", get(api_health_all))
        .route("/api/health/decay", post(api_decay))
        .route("/api/health/prune", post(api_prune))
        // Memoirs
        .route("/api/memoirs", get(api_memoirs))
        .route("/api/memoirs/{id}", get(api_memoir_detail))
        // Public
        .route("/health", get(api_health_check))
}

fn spa_router() -> Router<AppState> {
    Router::new()
        .route("/", get(serve_index))
        .fallback(serve_static)
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

#[tokio::main]
pub async fn run_web_server(
    store: SqliteStore,
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

    let app = api_router()
        .merge(spa_router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

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

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

async fn api_health_check() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn api_stats(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
    match store.get_by_topic(&name) {
        Ok(memories) => Json(memories).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_topic_health(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let store = state.store.lock().unwrap();
    match store.topic_health(&name) {
        Ok(health) => Json(health).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_topic_consolidate(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
    match store.search_fts(&params.q, params.limit) {
        Ok(memories) => Json(memories).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_memory_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
    let store = state.store.lock().unwrap();
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
