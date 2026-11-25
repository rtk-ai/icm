//! Agent Memory Leaderboard API for `icm serve --http` (`[http] aml = true`).
//!
//! The leaderboard (agentmemories.ai) calls two routes of a memory system
//! and does the answering and scoring itself:
//!
//! - `POST /aml/add` `{request_id, messages: [{role, timestamp, content}],
//!   user_id, session_id}`: each message becomes one memory, dated by its
//!   timestamp (Unix milliseconds) and prefixed with that date and the
//!   role, so the answering model can order events in time;
//! - `POST /aml/search` `{query, options?, user_id, top_k}`: a v2 recall of
//!   that user's memories, the multiple-choice `options` ranked as extra
//!   queries and fused with the question; `{data: [{id, content, score,
//!   created_at}]}`, at most `top_k`.
//!
//! `user_id` is the only isolation key, so each one gets its own SQLite
//! database in `[http] aml_dir` (named by a hash of the id): one user's
//! memories never crowd another's out of the candidates, and the server's
//! own memory store is never touched. The embedding work runs on blocking
//! threads so the server keeps answering while a large batch is added.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, TimeZone, Utc};
use icm_core::{Embedder, Importance, Memory, MemoryStore};
use icm_store::{RecallRequest, Store};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

/// Databases kept open at once; the others are reopened on demand.
const MAX_OPEN: usize = 64;

/// Longest memory a message becomes; a longer message is split.
const CHUNK_BYTES: usize = 60 * 1024;

/// Most results a search returns (the leaderboard asks for 100).
const MAX_TOP_K: usize = 100;

/// Most multiple-choice options ranked as extra queries.
const MAX_OPTIONS: usize = 7;

/// One database per `user_id`, opened lazily.
#[derive(Clone)]
pub struct AmlStores {
    dir: PathBuf,
    dims: usize,
    embedder: Option<Arc<dyn Embedder + Send + Sync>>,
    open: Arc<Mutex<HashMap<String, Arc<Mutex<Store>>>>>,
}

impl AmlStores {
    pub fn new(
        dir: PathBuf,
        dims: usize,
        embedder: Option<Arc<dyn Embedder + Send + Sync>>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            dir,
            dims,
            embedder,
            open: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// The database file of `user_id`: a hash, so any id is a safe name
    /// and two ids never share one.
    fn file_of(&self, user_id: &str) -> PathBuf {
        let hash = icm_core::to_hex(&Sha256::digest(user_id.as_bytes()));
        self.dir.join(format!("{}.db", &hash[..32]))
    }

    /// The store of `user_id`. `create` false: `None` when it has none yet.
    fn store(&self, user_id: &str, create: bool) -> Result<Option<Arc<Mutex<Store>>>> {
        let mut open = self.open.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(store) = open.get(user_id) {
            return Ok(Some(store.clone()));
        }
        let path = self.file_of(user_id);
        if !create && !path.exists() {
            return Ok(None);
        }
        if open.len() >= MAX_OPEN {
            // Any one: a closed database reopens on its next request.
            if let Some(key) = open.keys().next().cloned() {
                open.remove(&key);
            }
        }
        let store = Arc::new(Mutex::new(
            Store::with_dims(&path, self.dims)
                .with_context(|| format!("opening {}", path.display()))?,
        ));
        open.insert(user_id.to_string(), store.clone());
        Ok(Some(store))
    }

    fn embedder(&self) -> Option<&dyn Embedder> {
        self.embedder.as_deref().map(|e| e as &dyn Embedder)
    }
}

#[derive(Debug, Deserialize)]
pub struct AddReq {
    #[serde(default)]
    request_id: String,
    messages: Vec<Message>,
    user_id: String,
    #[serde(default)]
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    role: String,
    /// Unix milliseconds.
    #[serde(default)]
    timestamp: Option<i64>,
    content: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchReq {
    query: String,
    #[serde(default)]
    options: Vec<String>,
    user_id: String,
    #[serde(default)]
    top_k: Option<usize>,
}

fn error(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (
        status,
        Json(json!({"success": false, "error": msg.to_string()})),
    )
        .into_response()
}

/// The memories one message becomes: `[date] role: text`, split when
/// longer than a memory may be.
fn memories_of(msg: &Message, session_id: &str) -> Vec<Memory> {
    let at: Option<DateTime<Utc>> = msg
        .timestamp
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single());
    let date = at.map_or(String::new(), |t| {
        format!("[{}] ", t.format("%Y-%m-%d %H:%M"))
    });
    let role = if msg.role.is_empty() {
        "user"
    } else {
        &msg.role
    };
    let mut out = Vec::new();
    let mut rest = msg.content.trim();
    while !rest.is_empty() {
        let mut cut = rest.len().min(CHUNK_BYTES);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (part, tail) = rest.split_at(cut);
        rest = tail;
        let mut m = Memory::new(
            "aml".into(),
            format!("{date}{role}: {part}"),
            Importance::Medium,
        );
        if let Some(t) = at {
            m.created_at = t;
        }
        if !session_id.is_empty() {
            m.keywords = vec![session_id.to_string()];
        }
        out.push(m);
    }
    out
}

/// `POST /aml/add`: 200 once every message is stored and searchable.
pub async fn handle_add(State(stores): State<AmlStores>, Json(req): Json<AddReq>) -> Response {
    if req.user_id.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "user_id is required");
    }
    let echo = json!({
        "success": true,
        "request_id": req.request_id,
        "user_id": req.user_id,
        "session_id": req.session_id,
    });
    let done = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut memories: Vec<Memory> = req
            .messages
            .iter()
            .flat_map(|m| memories_of(m, &req.session_id))
            .collect();
        if let Some(emb) = stores.embedder() {
            for m in &mut memories {
                m.embedding = Some(emb.embed(&m.summary)?);
            }
        }
        let Some(store) = stores.store(&req.user_id, true)? else {
            return Ok(());
        };
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        for m in memories {
            store.store(m)?;
        }
        Ok(())
    })
    .await;
    match done {
        Ok(Ok(())) => Json(echo).into_response(),
        Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// `POST /aml/search`: the user's memories best matching the question.
pub async fn handle_search(
    State(stores): State<AmlStores>,
    Json(req): Json<SearchReq>,
) -> Response {
    if req.query.trim().is_empty() || req.user_id.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "query and user_id are required");
    }
    let found = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>> {
        let Some(store) = stores.store(&req.user_id, false)? else {
            return Ok(Vec::new());
        };
        let options: Vec<&str> = req
            .options
            .iter()
            .map(|o| o.trim())
            .filter(|o| !o.is_empty())
            .take(MAX_OPTIONS)
            .collect();
        let request = RecallRequest {
            query: &req.query,
            limit: req.top_k.unwrap_or(10).clamp(1, MAX_TOP_K),
            max_tokens: None,
            topic: None,
            keyword: None,
            project: None,
            now: None,
            expand_neighbors: false,
        };
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let outcome =
            request.run_many_read_only(&options, &store, stores.embedder(), &|_, _, _| 0)?;
        Ok(outcome
            .hits
            .into_iter()
            .map(|h| {
                json!({
                    "id": h.memory.id,
                    "content": h.memory.summary,
                    "score": h.score,
                    "created_at": h.memory.created_at.to_rfc3339(),
                })
            })
            .collect())
    })
    .await;
    match found {
        Ok(Ok(data)) => Json(json!({ "data": data })).into_response(),
        Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stores(name: &str) -> (AmlStores, PathBuf) {
        let dir = std::env::temp_dir().join(format!("icm-aml-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (AmlStores::new(dir.clone(), 384, None).unwrap(), dir)
    }

    async fn json_of(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn add(user: &str, texts: &[&str]) -> AddReq {
        serde_json::from_value(json!({
            "request_id": "r1",
            "user_id": user,
            "session_id": "s1",
            "messages": texts.iter().enumerate().map(|(i, t)| json!({
                "role": "user", "timestamp": 1_704_067_200_000i64 + i as i64 * 60_000, "content": t
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn search(user: &str, query: &str) -> SearchReq {
        serde_json::from_value(json!({"query": query, "user_id": user, "top_k": 100})).unwrap()
    }

    #[tokio::test]
    async fn added_messages_come_back_for_their_user_only() {
        let (s, dir) = stores("iso");
        let (status, body) = json_of(
            handle_add(
                State(s.clone()),
                Json(add(
                    "eval:r:locomo:conv-1",
                    &["Caroline adopted a dog named Rex"],
                )),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["session_id"], "s1");
        // A user whose id contains the first one shares nothing with it.
        handle_add(
            State(s.clone()),
            Json(add("eval:r:locomo:conv-10", &["Melanie paints sunsets"])),
        )
        .await;

        let (_, found) = json_of(
            handle_search(
                State(s.clone()),
                Json(search("eval:r:locomo:conv-1", "dog")),
            )
            .await,
        )
        .await;
        let data = found["data"].as_array().unwrap();
        assert_eq!(data.len(), 1, "{found}");
        let content = data[0]["content"].as_str().unwrap();
        assert!(
            content.starts_with("[2024-01-01 00:00] user: Caroline"),
            "{content}"
        );
        assert!(
            data[0]["created_at"]
                .as_str()
                .unwrap()
                .starts_with("2024-01-01")
        );

        let (_, other) = json_of(
            handle_search(
                State(s.clone()),
                Json(search("eval:r:locomo:conv-10", "dog")),
            )
            .await,
        )
        .await;
        assert!(
            other["data"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| !d["content"].as_str().unwrap().contains("Caroline")),
            "{other}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_unknown_user_has_no_memories_and_no_database() {
        let (s, dir) = stores("none");
        let (status, body) =
            json_of(handle_search(State(s.clone()), Json(search("nobody", "anything"))).await)
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"], json!([]));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_message_is_split_and_dated() {
        let msg = Message {
            role: "assistant".into(),
            timestamp: Some(1_704_067_200_000),
            content: "é".repeat(CHUNK_BYTES),
        };
        let parts = memories_of(&msg, "s");
        assert_eq!(parts.len(), 2);
        assert!(
            parts[0]
                .summary
                .starts_with("[2024-01-01 00:00] assistant: ")
        );
        assert_eq!(parts[0].keywords, vec!["s".to_string()]);
        let empty = Message {
            role: String::new(),
            timestamp: None,
            content: "  ".into(),
        };
        assert!(memories_of(&empty, "").is_empty());
    }

    #[test]
    fn user_ids_map_to_distinct_safe_files() {
        let (s, dir) = stores("names");
        let a = s.file_of("../../etc/passwd");
        assert_eq!(a.parent().unwrap(), dir);
        assert_ne!(s.file_of("conv-1"), s.file_of("conv-10"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
