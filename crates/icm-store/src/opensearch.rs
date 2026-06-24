//! OpenSearch storage backend (issue #301, opt-in via `--features opensearch`).
//!
//! A search-native shared store: BM25 full-text and `knn_vector` HNSW
//! vector search live in one engine, so horizontally-scaled ICM replicas
//! share one memory store (a node-local SQLite file cannot be shared).
//!
//! Design notes:
//!
//! - **Blocking REST.** OpenSearch is an HTTP/JSON service, so this talks
//!   to it with the blocking `ureq` client and `serde_json` bodies. The
//!   store traits are synchronous, so — like the PostgreSQL backend —
//!   there is no async runtime and no sync-over-async bridge.
//! - **Vector search** uses a `knn_vector` field (HNSW, cosine space);
//!   similarity is reported from the kNN `_score`.
//! - **Full-text search** uses BM25 `match` queries; the hybrid path
//!   blends normalized BM25 and vector scores 30/70 to match the SQLite
//!   and PostgreSQL backends.
//! - **Connection** from `ICM_OPENSEARCH_URL` (e.g. `http://localhost:9200`),
//!   with optional basic auth from `ICM_OPENSEARCH_USER` /
//!   `ICM_OPENSEARCH_PASSWORD`.
//!
//! Scope mirrors the PostgreSQL backend: the full [`MemoryStore`] surface
//! plus the ancillary store/recall/hook tables (hook telemetry, the
//! extraction queue, code areas, key/value metadata). The heavier
//! subsystems (memoir graph, transcripts, structured facts, feedback,
//! pattern mining) return [`IcmError::Unsupported`]; they stay fully
//! available on the default SQLite backend.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};

use icm_core::{
    Concept, ConceptLink, Embedder, Fact, FactsStats, FactsStore, Feedback, FeedbackStats,
    FeedbackStore, IcmError, IcmResult, Importance, Label, Memoir, MemoirStats, MemoirStore,
    Memory, MemorySource, MemoryStore, Message, PatternCluster, Relation, Role, Scope, Session,
    StoreStats, TopicHealth, TranscriptHit, TranscriptStats, TranscriptStore,
};

// Shared public row types live in `crate::common` (issue #301) so every
// backend can be compiled into one binary without colliding definitions.
pub use crate::common::{CodeArea, HookEvent, HookEventInsert, HookStatsRow, PendingRow};

// ---------------------------------------------------------------------------
// Index names
// ---------------------------------------------------------------------------

const IDX_MEMORIES: &str = "icm_memories";
const IDX_METADATA: &str = "icm_metadata";
const IDX_HOOKS: &str = "icm_hook_events";
const IDX_PENDING: &str = "icm_pending_extractions";
const IDX_CODE_AREAS: &str = "icm_code_areas";

// ---------------------------------------------------------------------------
// Pure helpers (self-contained, mirror the other backends)
// ---------------------------------------------------------------------------

fn source_type(source: &MemorySource) -> &'static str {
    match source {
        MemorySource::ClaudeCode { .. } => "claude_code",
        MemorySource::Conversation { .. } => "conversation",
        MemorySource::Manual => "manual",
    }
}

fn source_data(source: &MemorySource) -> Option<String> {
    match source {
        MemorySource::Manual => None,
        other => serde_json::to_string(other).ok(),
    }
}

fn parse_source(source_type_str: &str, source_data_str: Option<String>) -> MemorySource {
    match source_type_str {
        "manual" => MemorySource::Manual,
        _ => source_data_str
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or(MemorySource::Manual),
    }
}

fn importance_rank(i: Importance) -> u8 {
    match i {
        Importance::Low => 0,
        Importance::Medium => 1,
        Importance::High => 2,
        Importance::Critical => 3,
    }
}

fn max_importance(a: Importance, b: Importance) -> Importance {
    if importance_rank(a) >= importance_rank(b) {
        a
    } else {
        b
    }
}

/// SHA-256 over the normalized `(topic, summary)` pair, hex-encoded.
/// Normalization: trim + lowercase + collapse whitespace, joined by `\0`.
fn summary_hash(topic: &str, summary: &str) -> String {
    use sha2::{Digest, Sha256};
    let topic_n = topic.trim().to_lowercase();
    let summary_n = summary
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase();
    let mut h = Sha256::new();
    h.update(topic_n.as_bytes());
    h.update(b"\0");
    h.update(summary_n.as_bytes());
    format!("{:x}", h.finalize())
}

/// Validate and normalize a memory before storing (mirror of the other
/// backends): non-empty topic/summary, generate an id if missing, and
/// stamp timestamps.
fn validate_and_normalize(mut memory: Memory) -> IcmResult<Memory> {
    if memory.topic.trim().is_empty() {
        return Err(IcmError::InvalidInput("topic cannot be empty".into()));
    }
    if memory.summary.trim().is_empty() {
        return Err(IcmError::InvalidInput("summary cannot be empty".into()));
    }
    if memory.id.trim().is_empty() {
        memory.id = ulid::Ulid::new().to_string();
    }
    memory.topic = memory.topic.trim().to_string();
    Ok(memory)
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// OpenSearch-backed store. Cheap to clone-free share via `&self`; every
/// method is a blocking REST round-trip.
pub struct OpenSearchStore {
    agent: ureq::Agent,
    base: String,
    auth: Option<String>,
    embedding_dims: usize,
    readonly: bool,
}

impl OpenSearchStore {
    fn conn_url() -> IcmResult<String> {
        std::env::var("ICM_OPENSEARCH_URL")
            .or_else(|_| std::env::var("OPENSEARCH_URL"))
            .map_err(|_| {
                IcmError::Config(
                    "OpenSearch backend: set ICM_OPENSEARCH_URL to the cluster endpoint, \
                     e.g. http://localhost:9200"
                        .into(),
                )
            })
    }

    fn auth_header() -> Option<String> {
        let user = std::env::var("ICM_OPENSEARCH_USER").ok()?;
        let pass = std::env::var("ICM_OPENSEARCH_PASSWORD").unwrap_or_default();
        let token = B64.encode(format!("{user}:{pass}"));
        Some(format!("Basic {token}"))
    }

    /// Perform a request, returning the parsed JSON body. `expected_404`
    /// makes a 404 return `Ok(None)` instead of an error (used by `get`).
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        allow_404: bool,
    ) -> IcmResult<Option<Value>> {
        let url = format!(
            "{}/{}",
            self.base.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut req = self.agent.request(method, &url);
        if let Some(a) = &self.auth {
            req = req.set("Authorization", a);
        }
        let resp = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match resp {
            Ok(r) => {
                let v = r
                    .into_json::<Value>()
                    .map_err(|e| IcmError::Database(format!("opensearch decode: {e}")))?;
                Ok(Some(v))
            }
            Err(ureq::Error::Status(404, _)) if allow_404 => Ok(None),
            Err(ureq::Error::Status(code, r)) => {
                let txt = r.into_string().unwrap_or_default();
                Err(IcmError::Database(format!(
                    "opensearch {method} {path} -> {code}: {txt}"
                )))
            }
            Err(e) => Err(IcmError::Database(format!(
                "opensearch {method} {path}: {e}"
            ))),
        }
    }

    fn get_json(&self, path: &str) -> IcmResult<Option<Value>> {
        self.request("GET", path, None, true)
    }

    fn post(&self, path: &str, body: Value) -> IcmResult<Value> {
        self.request("POST", path, Some(body), false)
            .map(|o| o.unwrap_or(Value::Null))
    }

    /// Open or create a store with the default embedding dimension.
    pub fn new(_path: &Path) -> IcmResult<Self> {
        Self::connect(icm_core::DEFAULT_EMBEDDING_DIMS, false)
    }

    /// Open or create a store with a specific embedding dimension.
    pub fn with_dims(_path: &Path, embedding_dims: usize) -> IcmResult<Self> {
        Self::connect(embedding_dims, false)
    }

    /// Open the store read-only (issue #263). OpenSearch has no read-only
    /// connection mode, so this just flags the store and makes mutating
    /// methods error.
    pub fn open_readonly(_path: &Path) -> IcmResult<Self> {
        Self::connect(icm_core::DEFAULT_EMBEDDING_DIMS, true)
    }

    /// In-memory variant is not meaningful for a remote backend; connect
    /// from the environment instead.
    pub fn in_memory() -> IcmResult<Self> {
        Self::connect(icm_core::DEFAULT_EMBEDDING_DIMS, false)
    }

    /// See [`Self::in_memory`].
    pub fn in_memory_with_dims(embedding_dims: usize) -> IcmResult<Self> {
        Self::connect(embedding_dims, false)
    }

    /// Read the stored embedding dimension without committing to a full
    /// open. Returns `Ok(None)` when unreachable so callers can fall back.
    pub fn read_stored_embedding_dims(_path: &Path) -> IcmResult<Option<usize>> {
        let Ok(url) = Self::conn_url() else {
            return Ok(None);
        };
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        let store = OpenSearchStore {
            agent,
            base: url,
            auth: Self::auth_header(),
            embedding_dims: icm_core::DEFAULT_EMBEDDING_DIMS,
            readonly: true,
        };
        match store.get_metadata_int("embedding_dims") {
            Ok(Some(v)) => Ok(Some(v as usize)),
            _ => Ok(None),
        }
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// No-op on this backend (kept for API parity with the SQLite store).
    pub fn ensure_vec_init() {}

    fn connect(requested_dims: usize, readonly: bool) -> IcmResult<Self> {
        let url = Self::conn_url()?;
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build();
        let store = OpenSearchStore {
            agent,
            base: url,
            auth: Self::auth_header(),
            embedding_dims: requested_dims,
            readonly,
        };
        // Probe connectivity early with a clear error.
        store
            .get_json("/")
            .map_err(|e| IcmError::Database(format!("cannot reach OpenSearch: {e}")))?;

        // An existing database's stored dims are authoritative.
        let dims = match store.get_metadata_int("embedding_dims")? {
            Some(d) => d as usize,
            None => requested_dims,
        };
        let mut store = store;
        store.embedding_dims = dims;

        if !readonly {
            store.init_indices(dims)?;
            store.set_metadata_int("embedding_dims", dims as i64)?;
        }
        Ok(store)
    }

    fn index_exists(&self, idx: &str) -> IcmResult<bool> {
        let url = format!("{}/{}", self.base.trim_end_matches('/'), idx);
        let mut req = self.agent.request("HEAD", &url);
        if let Some(a) = &self.auth {
            req = req.set("Authorization", a);
        }
        match req.call() {
            Ok(_) => Ok(true),
            Err(ureq::Error::Status(404, _)) => Ok(false),
            Err(e) => Err(IcmError::Database(format!("opensearch HEAD {idx}: {e}"))),
        }
    }

    fn create_index(&self, idx: &str, body: Value) -> IcmResult<()> {
        if self.index_exists(idx)? {
            return Ok(());
        }
        // A racing replica may create it between the check and here; treat
        // "resource_already_exists_exception" as success.
        match self.request("PUT", idx, Some(body), false) {
            Ok(_) => Ok(()),
            Err(IcmError::Database(msg)) if msg.contains("resource_already_exists_exception") => {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn init_indices(&self, dims: usize) -> IcmResult<()> {
        if !(64..=4096).contains(&dims) {
            return Err(IcmError::Config(format!(
                "embedding_dims must be between 64 and 4096, got {dims}"
            )));
        }
        self.create_index(
            IDX_MEMORIES,
            json!({
                "settings": { "index": { "knn": true } },
                "mappings": { "properties": {
                    "created_at": {"type": "date"},
                    "updated_at": {"type": "date"},
                    "last_accessed": {"type": "date"},
                    "access_count": {"type": "integer"},
                    "weight": {"type": "float"},
                    "topic": {"type": "text", "fields": {"keyword": {"type": "keyword", "ignore_above": 1024}}},
                    "summary": {"type": "text"},
                    "raw_excerpt": {"type": "text"},
                    "keywords": {"type": "keyword"},
                    "importance": {"type": "keyword"},
                    "source_type": {"type": "keyword"},
                    "source_data": {"type": "text", "index": false},
                    "related_ids": {"type": "keyword"},
                    "summary_hash": {"type": "keyword"},
                    "embedding": {
                        "type": "knn_vector",
                        "dimension": dims,
                        "method": {"name": "hnsw", "space_type": "cosinesimil", "engine": "lucene"}
                    }
                }}
            }),
        )?;
        self.create_index(IDX_METADATA, json!({"mappings": {"properties": {"value": {"type": "double"}, "text_value": {"type": "keyword"}}}}))?;
        self.create_index(
            IDX_HOOKS,
            json!({"mappings": {"properties": {
                "id": {"type": "long"},
                "ts": {"type": "date"},
                "event": {"type": "keyword"},
                "project": {"type": "keyword"},
                "session_id": {"type": "keyword"},
                "tool_name": {"type": "keyword"},
                "duration_ms": {"type": "long"},
                "exit_code": {"type": "integer"},
                "payload_size": {"type": "long"},
                "note": {"type": "text"}
            }}}),
        )?;
        self.create_index(
            IDX_PENDING,
            json!({"mappings": {"properties": {
                "project": {"type": "keyword"},
                "tool_name": {"type": "keyword"},
                "raw_output": {"type": "text", "index": false},
                "captured_at": {"type": "date"}
            }}}),
        )?;
        self.create_index(
            IDX_CODE_AREAS,
            json!({"mappings": {"properties": {
                "project": {"type": "keyword"},
                "file_path": {"type": "keyword"},
                "description": {"type": "text"},
                "session_id": {"type": "keyword"},
                "tool_name": {"type": "keyword"},
                "touch_count": {"type": "long"},
                "first_touched_at": {"type": "date"},
                "last_touched_at": {"type": "date"}
            }}}),
        )?;
        Ok(())
    }

    // --- metadata kv helpers ---

    fn get_metadata_int(&self, key: &str) -> IcmResult<Option<i64>> {
        let path = format!("{IDX_METADATA}/_doc/{key}");
        match self.get_json(&path)? {
            Some(v) => Ok(v
                .get("_source")
                .and_then(|s| s.get("value"))
                .and_then(|n| n.as_f64())
                .map(|f| f as i64)),
            None => Ok(None),
        }
    }

    fn set_metadata_int(&self, key: &str, value: i64) -> IcmResult<()> {
        let path = format!("{IDX_METADATA}/_doc/{key}?refresh=true");
        self.request("PUT", &path, Some(json!({"value": value})), false)?;
        Ok(())
    }

    fn check_dims(&self, memory: &Memory) -> IcmResult<()> {
        if let Some(emb) = memory.embedding.as_ref() {
            if emb.len() != self.embedding_dims {
                return Err(IcmError::InvalidInput(format!(
                    "embedding has {} dimensions, but this store uses {}",
                    emb.len(),
                    self.embedding_dims
                )));
            }
        }
        Ok(())
    }

    // --- (de)serialization ---

    fn memory_to_source(memory: &Memory) -> Value {
        let mut doc = json!({
            "created_at": memory.created_at.to_rfc3339(),
            "updated_at": memory.updated_at.to_rfc3339(),
            "last_accessed": memory.last_accessed.to_rfc3339(),
            "access_count": memory.access_count,
            "weight": memory.weight,
            "topic": memory.topic,
            "summary": memory.summary,
            "raw_excerpt": memory.raw_excerpt,
            "keywords": memory.keywords,
            "importance": memory.importance.to_string(),
            "source_type": source_type(&memory.source),
            "source_data": source_data(&memory.source),
            "related_ids": memory.related_ids,
            "summary_hash": summary_hash(&memory.topic, &memory.summary),
        });
        if let Some(emb) = memory.embedding.as_ref() {
            doc["embedding"] = json!(emb);
        }
        doc
    }

    fn source_to_memory(id: &str, src: &Value) -> Memory {
        let get_str = |k: &str| {
            src.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let opt_str = |k: &str| {
            src.get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        };
        let arr = |k: &str| {
            src.get(k)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default()
        };
        let importance = get_str("importance").parse().unwrap_or(Importance::Medium);
        let source = parse_source(&get_str("source_type"), opt_str("source_data"));
        let embedding = src.get("embedding").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>()
        });
        Memory {
            id: id.to_string(),
            created_at: parse_dt(&get_str("created_at")),
            updated_at: parse_dt(&get_str("updated_at")),
            last_accessed: parse_dt(&get_str("last_accessed")),
            access_count: src
                .get("access_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            weight: src.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            topic: get_str("topic"),
            summary: get_str("summary"),
            raw_excerpt: opt_str("raw_excerpt"),
            keywords: arr("keywords"),
            importance,
            source,
            related_ids: arr("related_ids"),
            embedding,
            scope: Scope::default(),
        }
    }

    /// Map a `_search` response's hits to memories paired with `_score`.
    fn hits_to_scored(resp: &Value) -> Vec<(Memory, f32)> {
        resp.get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let id = h.get("_id")?.as_str()?;
                        let src = h.get("_source")?;
                        let score = h.get("_score").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                        Some((Self::source_to_memory(id, src), score))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn hits_to_memories(resp: &Value) -> Vec<Memory> {
        Self::hits_to_scored(resp)
            .into_iter()
            .map(|(m, _)| m)
            .collect()
    }

    fn refresh_param(&self) -> &'static str {
        // Force a refresh so writes are immediately visible to subsequent
        // searches (dedup, counts, the multi-replica path). ICM writes are
        // low-frequency curated memories, so the cost is acceptable.
        "refresh=true"
    }

    fn store_inner(&self, memory: &Memory) -> IcmResult<String> {
        let hash = summary_hash(&memory.topic, &memory.summary);
        // Dedup: an existing memory with the same (topic, summary_hash)
        // wins; merge importance (max) + keywords (union) + raw_excerpt
        // (prefer new) and return the existing id.
        let existing = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 1,
                "query": {"bool": {"filter": [
                    {"term": {"topic.keyword": memory.topic}},
                    {"term": {"summary_hash": hash}}
                ]}}
            }),
        )?;
        if let Some(hit) = existing
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .and_then(|a| a.first())
        {
            let existing_id = hit
                .get("_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let src = hit.get("_source").cloned().unwrap_or(Value::Null);
            let existing_importance: Importance = src
                .get("importance")
                .and_then(|v| v.as_str())
                .unwrap_or("medium")
                .parse()
                .unwrap_or(Importance::Medium);
            let merged_importance = max_importance(existing_importance, memory.importance);
            let mut merged_keywords: Vec<String> = src
                .get("keywords")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            for kw in &memory.keywords {
                if !merged_keywords.contains(kw) {
                    merged_keywords.push(kw.clone());
                }
            }
            let raw = memory.raw_excerpt.clone().or_else(|| {
                src.get("raw_excerpt")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
            self.request(
                "POST",
                &format!(
                    "{IDX_MEMORIES}/_update/{existing_id}?{}",
                    self.refresh_param()
                ),
                Some(json!({"doc": {
                    "importance": merged_importance.to_string(),
                    "keywords": merged_keywords,
                    "raw_excerpt": raw,
                    "updated_at": Utc::now().to_rfc3339(),
                }})),
                false,
            )?;
            return Ok(existing_id);
        }

        self.request(
            "PUT",
            &format!("{IDX_MEMORIES}/_doc/{}?{}", memory.id, self.refresh_param()),
            Some(Self::memory_to_source(memory)),
            false,
        )?;
        Ok(memory.id.clone())
    }
}

impl MemoryStore for OpenSearchStore {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        if self.readonly {
            return Err(IcmError::ReadOnly("store".into()));
        }
        let memory = validate_and_normalize(memory)?;
        self.check_dims(&memory)?;
        self.store_inner(&memory)
    }

    fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
        let path = format!("{IDX_MEMORIES}/_doc/{id}");
        match self.get_json(&path)? {
            Some(v) => {
                if v.get("found").and_then(|f| f.as_bool()).unwrap_or(false) {
                    let src = v.get("_source").cloned().unwrap_or(Value::Null);
                    Ok(Some(Self::source_to_memory(id, &src)))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn update(&self, memory: &Memory) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("update".into()));
        }
        self.check_dims(memory)?;
        let mut doc = Self::memory_to_source(memory);
        doc["updated_at"] = json!(Utc::now().to_rfc3339());
        // Replace the document wholesale (index by id).
        self.request(
            "PUT",
            &format!("{IDX_MEMORIES}/_doc/{}?{}", memory.id, self.refresh_param()),
            Some(doc),
            false,
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("delete".into()));
        }
        self.request(
            "DELETE",
            &format!("{IDX_MEMORIES}/_doc/{id}?{}", self.refresh_param()),
            None,
            true,
        )?;
        Ok(())
    }

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }
        let joined = keywords.join(" ");
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": limit,
                "query": {"bool": {"should": [
                    {"terms": {"keywords": keywords}},
                    {"multi_match": {"query": joined, "fields": ["summary", "topic"]}}
                ], "minimum_should_match": 1}}
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": limit,
                "query": {"multi_match": {
                    "query": query,
                    "fields": ["summary^2", "topic", "keywords"]
                }}
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn search_by_embedding(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": limit,
                "query": {"knn": {"embedding": {"vector": embedding, "k": limit}}}
            }),
        )?;
        Ok(Self::hits_to_scored(&resp))
    }

    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let limit = limit.min(1000);
        let pool = limit * 4;

        // FTS candidates (BM25).
        let mut fts_scores: HashMap<String, f32> = HashMap::new();
        let mut memories: HashMap<String, Memory> = HashMap::new();
        if !query.trim().is_empty() {
            let resp = self.post(
                &format!("{IDX_MEMORIES}/_search"),
                json!({
                    "size": pool,
                    "query": {"multi_match": {"query": query, "fields": ["summary^2", "topic", "keywords"]}}
                }),
            )?;
            for (m, s) in Self::hits_to_scored(&resp) {
                fts_scores.insert(m.id.clone(), s);
                memories.insert(m.id.clone(), m);
            }
        }

        // Vector candidates.
        let mut vec_scores: HashMap<String, f32> = HashMap::new();
        for (m, s) in self.search_by_embedding(embedding, pool)? {
            vec_scores.insert(m.id.clone(), s);
            memories.entry(m.id.clone()).or_insert(m);
        }

        // Min-max normalize each score family to [0, 1] before blending.
        let norm = |scores: &HashMap<String, f32>| -> HashMap<String, f32> {
            if scores.is_empty() {
                return HashMap::new();
            }
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &v in scores.values() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let span = (hi - lo).max(f32::EPSILON);
            scores
                .iter()
                .map(|(k, v)| (k.clone(), (v - lo) / span))
                .collect()
        };
        let fts_n = norm(&fts_scores);
        let vec_n = norm(&vec_scores);

        let mut scored: Vec<(String, f32)> = memories
            .keys()
            .map(|id| {
                let f = fts_n.get(id).copied().unwrap_or(0.0);
                let v = vec_n.get(id).copied().unwrap_or(0.0);
                (id.clone(), 0.3 * f + 0.7 * v)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .filter_map(|(id, s)| memories.remove(&id).map(|m| (m, s)))
            .collect())
    }

    fn update_access(&self, id: &str) -> IcmResult<()> {
        if self.readonly {
            return Ok(());
        }
        // Best-effort; a missing doc is not an error for recall bookkeeping.
        let _ = self.request(
            "POST",
            &format!("{IDX_MEMORIES}/_update/{id}"),
            Some(json!({
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.access_count = (ctx._source.access_count == null ? 1 : ctx._source.access_count + 1); ctx._source.last_accessed = params.now;",
                    "params": {"now": Utc::now().to_rfc3339()}
                }
            })),
            true,
        )?;
        Ok(())
    }

    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize> {
        if self.readonly || ids.is_empty() {
            return Ok(0);
        }
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_update_by_query?{}&conflicts=proceed", self.refresh_param()),
            json!({
                "query": {"ids": {"values": ids}},
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.access_count = (ctx._source.access_count == null ? 1 : ctx._source.access_count + 1); ctx._source.last_accessed = params.now;",
                    "params": {"now": Utc::now().to_rfc3339()}
                }
            }),
        )?;
        Ok(resp.get("updated").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly("decay".into()));
        }
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_update_by_query?{}&conflicts=proceed", self.refresh_param()),
            json!({
                "query": {"bool": {"must_not": [{"term": {"importance": "critical"}}]}},
                "script": {
                    "lang": "painless",
                    "source": "double f = params.factor; String imp = ctx._source.importance; double mult = imp != null && imp.equals('high') ? 0.5 : (imp != null && imp.equals('low') ? 2.0 : 1.0); double ac = ctx._source.access_count == null ? 0 : ctx._source.access_count; if (ac > 5) ac = 5; ctx._source.weight = ctx._source.weight * (1.0 - (1.0 - f) * mult / (1.0 + ac * 0.1));",
                    "params": {"factor": decay_factor as f64}
                }
            }),
        )?;
        Ok(resp.get("updated").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn prune(&self, weight_threshold: f32) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly("prune".into()));
        }
        let resp = self.post(
            &format!(
                "{IDX_MEMORIES}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({
                "query": {"bool": {
                    "must": [{"range": {"weight": {"lt": weight_threshold as f64}}}],
                    "must_not": [{"terms": {"importance": ["critical", "high"]}}]
                }}
            }),
        )?;
        Ok(resp.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 500,
                "query": {"term": {"topic.keyword": topic}},
                "sort": [{"weight": "desc"}]
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn list_all(&self) -> IcmResult<Vec<Memory>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({"size": 10000, "query": {"match_all": {}}, "sort": [{"weight": "desc"}]}),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({"size": 0, "aggs": {"topics": {"terms": {"field": "topic.keyword", "size": 10000}}}}),
        )?;
        let mut out = bucket_counts(&resp, "topics");
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("consolidate".into()));
        }
        // Not atomic (delete-then-insert); acceptable for a maintenance op.
        self.post(
            &format!(
                "{IDX_MEMORIES}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({"query": {"term": {"topic.keyword": topic}}}),
        )?;
        let consolidated = validate_and_normalize(consolidated)?;
        self.check_dims(&consolidated)?;
        self.store_inner(&consolidated)?;
        Ok(())
    }

    fn count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_MEMORIES}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn count_by_topic(&self, topic: &str) -> IcmResult<usize> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_count"),
            json!({"query": {"term": {"topic.keyword": topic}}}),
        )?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn stats(&self) -> IcmResult<StoreStats> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 0,
                "track_total_hits": true,
                "aggs": {
                    "avg_w": {"avg": {"field": "weight"}},
                    "topics": {"cardinality": {"field": "topic.keyword"}},
                    "oldest": {"min": {"field": "created_at", "format": "date_time"}},
                    "newest": {"max": {"field": "created_at", "format": "date_time"}}
                }
            }),
        )?;
        let total = resp
            .get("hits")
            .and_then(|h| h.get("total"))
            .and_then(|t| t.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let aggs = resp.get("aggregations").cloned().unwrap_or(Value::Null);
        let avg_weight = aggs
            .get("avg_w")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let total_topics = aggs
            .get("topics")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let parse_agg_date = |name: &str| -> Option<DateTime<Utc>> {
            aggs.get(name)
                .and_then(|a| a.get("value_as_string"))
                .and_then(|v| v.as_str())
                .map(parse_dt)
        };
        Ok(StoreStats {
            total_memories: total,
            total_topics,
            avg_weight,
            oldest_memory: parse_agg_date("oldest"),
            newest_memory: parse_agg_date("newest"),
        })
    }

    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 0,
                "track_total_hits": true,
                "query": {"term": {"topic.keyword": topic}},
                "aggs": {
                    "avg_w": {"avg": {"field": "weight"}},
                    "avg_ac": {"avg": {"field": "access_count"}},
                    "oldest": {"min": {"field": "created_at"}},
                    "newest": {"max": {"field": "created_at"}},
                    "last_acc": {"max": {"field": "last_accessed"}},
                    "stale": {"filter": {"bool": {"must": [
                        {"range": {"weight": {"lt": 0.5}}},
                        {"range": {"last_accessed": {"lt": "now-14d"}}}
                    ]}}}
                }
            }),
        )?;
        let entry_count = resp
            .get("hits")
            .and_then(|h| h.get("total"))
            .and_then(|t| t.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        if entry_count == 0 {
            return Err(IcmError::NotFound(format!(
                "no memories in topic '{topic}'"
            )));
        }
        let aggs = resp.get("aggregations").cloned().unwrap_or(Value::Null);
        let avg_weight = aggs
            .get("avg_w")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let avg_access_count = aggs
            .get("avg_ac")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let stale_count = aggs
            .get("stale")
            .and_then(|a| a.get("doc_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        Ok(TopicHealth {
            topic: topic.to_string(),
            entry_count,
            avg_weight,
            avg_access_count,
            oldest: agg_date(&aggs, "oldest"),
            newest: agg_date(&aggs, "newest"),
            last_accessed: agg_date(&aggs, "last_acc"),
            stale_count,
            needs_consolidation: entry_count > 5,
        })
    }
}

/// Read a `min`/`max` date aggregation into a `DateTime<Utc>`.
///
/// Prefers the ISO `value_as_string` OpenSearch returns and falls back to
/// the epoch-millis `value`. Returns `None` when the bucket is empty.
fn agg_date(aggs: &Value, name: &str) -> Option<DateTime<Utc>> {
    let node = aggs.get(name)?;
    if let Some(s) = node.get("value_as_string").and_then(|v| v.as_str()) {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    let ms = node.get("value").and_then(|v| v.as_f64())?;
    if ms <= 0.0 {
        return None;
    }
    Utc.timestamp_millis_opt(ms as i64).single()
}

/// Extract `(key, doc_count)` pairs from a terms aggregation.
fn bucket_counts(resp: &Value, agg: &str) -> Vec<(String, usize)> {
    resp.get("aggregations")
        .and_then(|a| a.get(agg))
        .and_then(|t| t.get("buckets"))
        .and_then(|b| b.as_array())
        .map(|buckets| {
            buckets
                .iter()
                .filter_map(|b| {
                    let key = b.get("key")?.as_str()?.to_string();
                    let count = b.get("doc_count")?.as_u64()? as usize;
                    Some((key, count))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Inherent methods used by the cli/mcp store/recall/hook path
// ---------------------------------------------------------------------------

impl OpenSearchStore {
    pub fn maybe_auto_decay(&self) -> IcmResult<()> {
        if self.readonly {
            return Ok(());
        }
        // Atomic-ish claim via a scripted upsert on a metadata doc: only the
        // caller that flips `changed` to true runs the decay.
        let now_ms = Utc::now().timestamp_millis();
        let resp = self.post(
            &format!("{IDX_METADATA}/_update/last_decay_at?{}&_source=true", self.refresh_param()),
            json!({
                "scripted_upsert": true,
                "upsert": {},
                "script": {
                    "lang": "painless",
                    "source": "if (ctx._source.value == null || params.now - ctx._source.value >= 86400000L) { ctx._source.value = params.now; ctx._source.changed = true; } else { ctx._source.changed = false; }",
                    "params": {"now": now_ms}
                }
            }),
        )?;
        let changed = resp
            .get("get")
            .and_then(|g| g.get("_source"))
            .and_then(|s| s.get("changed"))
            .and_then(|c| c.as_bool())
            .unwrap_or(false);
        if changed {
            self.apply_decay(0.95)?;
        }
        Ok(())
    }

    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        let resp = self.post(
            &format!("{IDX_METADATA}/_update/hook_counter?_source=true"),
            json!({
                "scripted_upsert": true,
                "upsert": {},
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.value = (ctx._source.value == null ? 1 : ctx._source.value + 1);"
                }
            }),
        )?;
        Ok(resp
            .get("get")
            .and_then(|g| g.get("_source"))
            .and_then(|s| s.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as usize)
    }

    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        self.set_metadata_int("hook_counter", 0)
    }

    pub fn enqueue_pending_extraction(
        &self,
        project: &str,
        tool_name: &str,
        raw_output: &str,
    ) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        self.request(
            "PUT",
            &format!("{IDX_PENDING}/_doc/{id}?{}", self.refresh_param()),
            Some(json!({
                "project": project,
                "tool_name": tool_name,
                "raw_output": raw_output,
                "captured_at": Utc::now().to_rfc3339()
            })),
            false,
        )?;
        Ok(id)
    }

    pub fn list_pending_extractions(&self, limit: usize) -> IcmResult<Vec<PendingRow>> {
        let resp = self.post(
            &format!("{IDX_PENDING}/_search"),
            json!({"size": limit, "query": {"match_all": {}}, "sort": [{"captured_at": "asc"}]}),
        )?;
        let rows = resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let id = h.get("_id")?.as_str()?.to_string();
                        let s = h.get("_source")?;
                        Some((
                            id,
                            s.get("project")?.as_str()?.to_string(),
                            s.get("tool_name")?.as_str()?.to_string(),
                            s.get("raw_output")?.as_str()?.to_string(),
                            s.get("captured_at")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(rows)
    }

    pub fn delete_pending_extractions(&self, ids: &[String]) -> IcmResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let resp = self.post(
            &format!(
                "{IDX_PENDING}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({"query": {"ids": {"values": ids}}}),
        )?;
        Ok(resp.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn pending_extraction_count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_PENDING}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn upsert_code_area(
        &self,
        project: &str,
        file_path: &str,
        description: Option<&str>,
        session_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("upsert_code_area".into()));
        }
        let ts = Utc::now();
        let now = ts.to_rfc3339();
        let id = ts.timestamp_millis();
        // Deterministic id makes the same (project, file_path) a single row.
        let key = B64.encode(format!("{project}\0{file_path}"));
        self.request(
            "POST",
            &format!("{IDX_CODE_AREAS}/_update/{key}?{}", self.refresh_param()),
            Some(json!({
                "scripted_upsert": true,
                "upsert": {
                    "id": id,
                    "project": project,
                    "file_path": file_path,
                    "description": description,
                    "session_id": session_id,
                    "tool_name": tool_name,
                    "touch_count": 1,
                    "first_touched_at": now,
                    "last_touched_at": now
                },
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.touch_count = (ctx._source.touch_count == null ? 1 : ctx._source.touch_count + 1); ctx._source.last_touched_at = params.now; if (params.description != null) ctx._source.description = params.description; if (params.session_id != null) ctx._source.session_id = params.session_id; if (params.tool_name != null) ctx._source.tool_name = params.tool_name;",
                    "params": {"now": now, "description": description, "session_id": session_id, "tool_name": tool_name}
                }
            })),
            false,
        )?;
        Ok(())
    }

    pub fn list_code_areas(
        &self,
        project: Option<&str>,
        in_file: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> IcmResult<Vec<CodeArea>> {
        let mut filters: Vec<Value> = Vec::new();
        if let Some(p) = project {
            filters.push(json!({"term": {"project": p}}));
        }
        if let Some(f) = in_file {
            filters.push(json!({"wildcard": {"file_path": {"value": format!("*{f}*")}}}));
        }
        if let Some(s) = since {
            filters.push(json!({"range": {"last_touched_at": {"gte": s.to_rfc3339()}}}));
        }
        let query = if filters.is_empty() {
            json!({"match_all": {}})
        } else {
            json!({"bool": {"filter": filters}})
        };
        let resp = self.post(
            &format!("{IDX_CODE_AREAS}/_search"),
            json!({"size": limit, "query": query, "sort": [{"last_touched_at": "desc"}]}),
        )?;
        let rows = resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let s = h.get("_source")?;
                        Some(CodeArea {
                            id: s.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            project: s.get("project")?.as_str()?.to_string(),
                            file_path: s.get("file_path")?.as_str()?.to_string(),
                            description: s
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            session_id: s
                                .get("session_id")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            tool_name: s
                                .get("tool_name")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            touch_count: s.get("touch_count").and_then(|v| v.as_i64()).unwrap_or(1),
                            first_touched_at: s
                                .get("first_touched_at")
                                .and_then(|v| v.as_str())
                                .and_then(|x| DateTime::parse_from_rfc3339(x).ok())
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(Utc::now),
                            last_touched_at: s
                                .get("last_touched_at")
                                .and_then(|v| v.as_str())
                                .and_then(|x| DateTime::parse_from_rfc3339(x).ok())
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(Utc::now),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(rows)
    }

    pub fn code_area_count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_CODE_AREAS}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn record_hook_event(&self, ev: &HookEventInsert) -> IcmResult<i64> {
        let now = Utc::now();
        let id = now.timestamp_millis();
        let doc_id = ulid::Ulid::new().to_string();
        self.request(
            "PUT",
            &format!("{IDX_HOOKS}/_doc/{doc_id}"),
            Some(json!({
                "id": id,
                "ts": now.to_rfc3339(),
                "event": ev.event,
                "project": ev.project,
                "session_id": ev.session_id,
                "tool_name": ev.tool_name,
                "duration_ms": ev.duration_ms,
                "exit_code": ev.exit_code,
                "payload_size": ev.payload_size,
                "note": ev.note
            })),
            false,
        )?;
        Ok(id)
    }

    pub fn hook_events_recent(
        &self,
        limit: usize,
        event_filter: Option<&str>,
    ) -> IcmResult<Vec<HookEvent>> {
        let query = match event_filter {
            Some(e) => json!({"term": {"event": e}}),
            None => json!({"match_all": {}}),
        };
        let resp = self.post(
            &format!("{IDX_HOOKS}/_search"),
            json!({"size": limit, "query": query, "sort": [{"ts": "desc"}]}),
        )?;
        let rows = resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let s = h.get("_source")?;
                        Some(HookEvent {
                            id: s.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            ts: parse_dt(s.get("ts").and_then(|v| v.as_str()).unwrap_or("")),
                            event: s
                                .get("event")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            project: s.get("project").and_then(|v| v.as_str()).map(String::from),
                            session_id: s
                                .get("session_id")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            tool_name: s
                                .get("tool_name")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            duration_ms: s.get("duration_ms").and_then(|v| v.as_i64()),
                            exit_code: s.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(0)
                                as i32,
                            payload_size: s.get("payload_size").and_then(|v| v.as_i64()),
                            note: s.get("note").and_then(|v| v.as_str()).map(String::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(rows)
    }

    pub fn hook_stats(&self, since_rfc3339: &str) -> IcmResult<Vec<HookStatsRow>> {
        let resp = self.post(
            &format!("{IDX_HOOKS}/_search"),
            json!({
                "size": 0,
                "query": {"range": {"ts": {"gte": since_rfc3339}}},
                "aggs": {"events": {
                    "terms": {"field": "event", "size": 1000},
                    "aggs": {
                        "errs": {"filter": {"bool": {"must_not": [{"term": {"exit_code": 0}}]}}},
                        "avg_dur": {"avg": {"field": "duration_ms"}},
                        "pct": {"percentiles": {"field": "duration_ms", "percents": [50, 99]}}
                    }
                }}
            }),
        )?;
        let buckets = resp
            .get("aggregations")
            .and_then(|a| a.get("events"))
            .and_then(|e| e.get("buckets"))
            .and_then(|b| b.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for b in &buckets {
            let pct = b.get("pct").and_then(|p| p.get("values"));
            let p = |k: &str| {
                pct.and_then(|v| v.get(k))
                    .and_then(|v| v.as_f64())
                    .filter(|f| f.is_finite())
                    .unwrap_or(0.0) as i64
            };
            out.push(HookStatsRow {
                event: b
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                count: b.get("doc_count").and_then(|v| v.as_i64()).unwrap_or(0),
                error_count: b
                    .get("errs")
                    .and_then(|e| e.get("doc_count"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                avg_duration_ms: b
                    .get("avg_dur")
                    .and_then(|a| a.get("value"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                p50_duration_ms: p("50.0"),
                p99_duration_ms: p("99.0"),
            });
        }
        out.sort_by(|a, b| a.event.cmp(&b.event));
        Ok(out)
    }

    pub fn prune_hook_events(&self, cutoff_rfc3339: &str) -> IcmResult<usize> {
        let resp = self.post(
            &format!(
                "{IDX_HOOKS}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({"query": {"range": {"ts": {"lt": cutoff_rfc3339}}}}),
        )?;
        Ok(resp.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn hook_event_count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_HOOKS}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    /// Auto-consolidation is not yet implemented on this backend; it is a
    /// no-op (returns `false`) so the normal store path keeps working.
    pub fn auto_consolidate(&self, _topic: &str, _threshold: usize) -> IcmResult<bool> {
        Ok(false)
    }

    /// See [`Self::auto_consolidate`].
    pub fn auto_consolidate_with_embedder(
        &self,
        _topic: &str,
        _threshold: usize,
        _embedder: Option<&dyn Embedder>,
    ) -> IcmResult<bool> {
        Ok(false)
    }

    pub fn get_many(&self, ids: &[&str]) -> IcmResult<HashMap<String, Memory>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let resp = self.post(&format!("{IDX_MEMORIES}/_mget"), json!({"ids": ids}))?;
        let mut out = HashMap::new();
        if let Some(docs) = resp.get("docs").and_then(|d| d.as_array()) {
            for d in docs {
                if d.get("found").and_then(|f| f.as_bool()).unwrap_or(false) {
                    if let (Some(id), Some(src)) =
                        (d.get("_id").and_then(|v| v.as_str()), d.get("_source"))
                    {
                        out.insert(id.to_string(), Self::source_to_memory(id, src));
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn get_by_topic_prefix(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 500,
                "query": {"prefix": {"topic.keyword": topic}},
                "sort": [{"weight": "desc"}]
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    pub fn list_topics_with_prefix(&self, prefix: Option<&str>) -> IcmResult<Vec<(String, usize)>> {
        let mut topics = self.list_topics()?;
        if let Some(p) = prefix {
            topics.retain(|(t, _)| t.starts_with(p));
        }
        Ok(topics)
    }

    /// Expand a result set with graph neighbours (related ids), applying a
    /// hop discount. Pure logic over [`Self::get_many`]; identical to the
    /// other backends.
    pub fn expand_with_neighbors(
        &self,
        initial: &[(Memory, f32)],
        max_neighbors: usize,
        hop_discount: f32,
        max_total: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        if max_neighbors == 0 || initial.is_empty() {
            let mut out = initial.to_vec();
            out.truncate(max_total);
            return Ok(out);
        }
        let initial_ids: HashSet<String> = initial.iter().map(|(m, _)| m.id.clone()).collect();
        let mut candidates: Vec<(String, f32)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (m, score) in initial {
            for rid in &m.related_ids {
                if !initial_ids.contains(rid) && seen.insert(rid.clone()) {
                    candidates.push((rid.clone(), *score * hop_discount));
                    if candidates.len() >= max_neighbors {
                        break;
                    }
                }
            }
            if candidates.len() >= max_neighbors {
                break;
            }
        }

        let neighbor_ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
        let fetched = self.get_many(&neighbor_ids)?;

        let mut combined: Vec<(Memory, f32)> = initial.to_vec();
        for (id, score) in candidates {
            if let Some(m) = fetched.get(&id) {
                combined.push((m.clone(), score));
            }
        }
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(max_total);
        Ok(combined)
    }

    /// Pattern mining is not implemented on this backend yet.
    pub fn detect_patterns(
        &self,
        _topic: &str,
        _min_cluster_size: usize,
    ) -> IcmResult<Vec<PatternCluster>> {
        Err(IcmError::Unsupported("detect_patterns".into()))
    }

    /// See [`Self::detect_patterns`].
    pub fn extract_pattern_as_concept(
        &self,
        _cluster: &PatternCluster,
        _memoir_id: &str,
    ) -> IcmResult<String> {
        Err(IcmError::Unsupported("extract_pattern_as_concept".into()))
    }
}

// ---------------------------------------------------------------------------
// Subsystems not yet ported to this backend. They stay fully available on
// the default SQLite backend; here they fail cleanly with `Unsupported`.
// ---------------------------------------------------------------------------

fn unsupported<T>(op: &str) -> IcmResult<T> {
    Err(IcmError::Unsupported(format!(
        "{op} (use the default SQLite backend)"
    )))
}

impl MemoirStore for OpenSearchStore {
    fn create_memoir(&self, _memoir: Memoir) -> IcmResult<String> {
        unsupported("memoir.create_memoir")
    }
    fn get_memoir(&self, _id: &str) -> IcmResult<Option<Memoir>> {
        unsupported("memoir.get_memoir")
    }
    fn get_memoir_by_name(&self, _name: &str) -> IcmResult<Option<Memoir>> {
        unsupported("memoir.get_memoir_by_name")
    }
    fn update_memoir(&self, _memoir: &Memoir) -> IcmResult<()> {
        unsupported("memoir.update_memoir")
    }
    fn delete_memoir(&self, _id: &str) -> IcmResult<()> {
        unsupported("memoir.delete_memoir")
    }
    fn list_memoirs(&self) -> IcmResult<Vec<Memoir>> {
        unsupported("memoir.list_memoirs")
    }
    fn add_concept(&self, _concept: Concept) -> IcmResult<String> {
        unsupported("memoir.add_concept")
    }
    fn get_concept(&self, _id: &str) -> IcmResult<Option<Concept>> {
        unsupported("memoir.get_concept")
    }
    fn get_concept_by_name(&self, _memoir_id: &str, _name: &str) -> IcmResult<Option<Concept>> {
        unsupported("memoir.get_concept_by_name")
    }
    fn update_concept(&self, _concept: &Concept) -> IcmResult<()> {
        unsupported("memoir.update_concept")
    }
    fn delete_concept(&self, _id: &str) -> IcmResult<()> {
        unsupported("memoir.delete_concept")
    }
    fn list_concepts(&self, _memoir_id: &str) -> IcmResult<Vec<Concept>> {
        unsupported("memoir.list_concepts")
    }
    fn search_concepts_fts(
        &self,
        _memoir_id: &str,
        _query: &str,
        _limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        unsupported("memoir.search_concepts_fts")
    }
    fn search_concepts_by_label(
        &self,
        _memoir_id: &str,
        _label: &Label,
        _limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        unsupported("memoir.search_concepts_by_label")
    }
    fn search_all_concepts_fts(&self, _query: &str, _limit: usize) -> IcmResult<Vec<Concept>> {
        unsupported("memoir.search_all_concepts_fts")
    }
    fn refine_concept(
        &self,
        _id: &str,
        _new_definition: &str,
        _new_source_ids: &[String],
    ) -> IcmResult<()> {
        unsupported("memoir.refine_concept")
    }
    fn add_link(&self, _link: ConceptLink) -> IcmResult<String> {
        unsupported("memoir.add_link")
    }
    fn get_links_from(&self, _concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        unsupported("memoir.get_links_from")
    }
    fn get_links_to(&self, _concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        unsupported("memoir.get_links_to")
    }
    fn delete_link(&self, _id: &str) -> IcmResult<()> {
        unsupported("memoir.delete_link")
    }
    fn get_neighbors(
        &self,
        _concept_id: &str,
        _relation: Option<Relation>,
    ) -> IcmResult<Vec<Concept>> {
        unsupported("memoir.get_neighbors")
    }
    fn get_neighborhood(
        &self,
        _concept_id: &str,
        _depth: usize,
    ) -> IcmResult<(Vec<Concept>, Vec<ConceptLink>)> {
        unsupported("memoir.get_neighborhood")
    }
    fn get_links_for_memoir(&self, _memoir_id: &str) -> IcmResult<Vec<ConceptLink>> {
        unsupported("memoir.get_links_for_memoir")
    }
    fn memoir_stats(&self, _memoir_id: &str) -> IcmResult<MemoirStats> {
        unsupported("memoir.memoir_stats")
    }
    fn batch_memoir_concept_counts(&self) -> IcmResult<HashMap<String, usize>> {
        unsupported("memoir.batch_memoir_concept_counts")
    }
}

impl FeedbackStore for OpenSearchStore {
    fn store_feedback(&self, _feedback: Feedback) -> IcmResult<String> {
        unsupported("feedback.store_feedback")
    }
    fn search_feedback(
        &self,
        _query: &str,
        _topic: Option<&str>,
        _limit: usize,
    ) -> IcmResult<Vec<Feedback>> {
        unsupported("feedback.search_feedback")
    }
    fn list_feedback(&self, _topic: Option<&str>, _limit: usize) -> IcmResult<Vec<Feedback>> {
        unsupported("feedback.list_feedback")
    }
    fn increment_applied(&self, _id: &str) -> IcmResult<()> {
        unsupported("feedback.increment_applied")
    }
    fn delete_feedback(&self, _id: &str) -> IcmResult<()> {
        unsupported("feedback.delete_feedback")
    }
    fn feedback_stats(&self) -> IcmResult<FeedbackStats> {
        unsupported("feedback.feedback_stats")
    }
}

impl FactsStore for OpenSearchStore {
    fn set_fact(
        &self,
        _entity: &str,
        _key: &str,
        _value: &str,
        _source: &str,
    ) -> IcmResult<String> {
        unsupported("facts.set_fact")
    }
    fn get_fact(&self, _entity: &str, _key: &str) -> IcmResult<Option<Fact>> {
        unsupported("facts.get_fact")
    }
    fn list_facts(&self, _entity: &str, _key_prefix: Option<&str>) -> IcmResult<Vec<Fact>> {
        unsupported("facts.list_facts")
    }
    fn history(&self, _entity: &str, _key: &str) -> IcmResult<Vec<Fact>> {
        unsupported("facts.history")
    }
    fn forget_fact(&self, _entity: &str, _key: &str) -> IcmResult<usize> {
        unsupported("facts.forget_fact")
    }
    fn facts_stats(&self) -> IcmResult<FactsStats> {
        unsupported("facts.facts_stats")
    }
}

impl TranscriptStore for OpenSearchStore {
    fn create_session(
        &self,
        _agent: &str,
        _project: Option<&str>,
        _metadata: Option<&str>,
    ) -> IcmResult<String> {
        unsupported("transcript.create_session")
    }
    fn ensure_session(
        &self,
        _id: &str,
        _agent: &str,
        _project: Option<&str>,
        _metadata: Option<&str>,
    ) -> IcmResult<String> {
        unsupported("transcript.ensure_session")
    }
    fn get_session(&self, _id: &str) -> IcmResult<Option<Session>> {
        unsupported("transcript.get_session")
    }
    fn list_sessions(&self, _project: Option<&str>, _limit: usize) -> IcmResult<Vec<Session>> {
        unsupported("transcript.list_sessions")
    }
    fn record_message(
        &self,
        _session_id: &str,
        _role: Role,
        _content: &str,
        _tool_name: Option<&str>,
        _tokens: Option<i64>,
        _metadata: Option<&str>,
    ) -> IcmResult<String> {
        unsupported("transcript.record_message")
    }
    fn list_session_messages(
        &self,
        _session_id: &str,
        _limit: usize,
        _offset: usize,
    ) -> IcmResult<Vec<Message>> {
        unsupported("transcript.list_session_messages")
    }
    fn search_transcripts(
        &self,
        _query: &str,
        _session_id: Option<&str>,
        _project: Option<&str>,
        _limit: usize,
    ) -> IcmResult<Vec<TranscriptHit>> {
        unsupported("transcript.search_transcripts")
    }
    fn forget_session(&self, _id: &str) -> IcmResult<()> {
        unsupported("transcript.forget_session")
    }
    fn transcript_stats(&self) -> IcmResult<TranscriptStats> {
        unsupported("transcript.transcript_stats")
    }
}
