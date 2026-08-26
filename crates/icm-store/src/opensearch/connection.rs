//! OpenSearch backend -- split out of the former monolithic opensearch.rs.
//!
//! Connection setup, constructors, and index/schema migration.

use super::*;

impl OpenSearchStore {
    pub(crate) fn conn_url() -> IcmResult<String> {
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

    pub(crate) fn auth_header() -> Option<String> {
        let user = std::env::var("ICM_OPENSEARCH_USER").ok()?;
        let pass = std::env::var("ICM_OPENSEARCH_PASSWORD").unwrap_or_default();
        let token = B64.encode(format!("{user}:{pass}"));
        Some(format!("Basic {token}"))
    }

    /// Perform a request, returning the parsed JSON body. `expected_404`
    /// makes a 404 return `Ok(None)` instead of an error (used by `get`).
    pub(crate) fn request(
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

    pub(crate) fn get_json(&self, path: &str) -> IcmResult<Option<Value>> {
        self.request("GET", path, None, true)
    }

    pub(crate) fn post(&self, path: &str, body: Value) -> IcmResult<Value> {
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

    pub(crate) fn connect(requested_dims: usize, readonly: bool) -> IcmResult<Self> {
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

    pub(crate) fn index_exists(&self, idx: &str) -> IcmResult<bool> {
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

    pub(crate) fn create_index(&self, idx: &str, body: Value) -> IcmResult<()> {
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

    pub(crate) fn init_indices(&self, dims: usize) -> IcmResult<()> {
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
            IDX_CONSOLIDATION_JOBS,
            json!({"mappings": {"properties": {
                "topic": {"type": "keyword"},
                "project": {"type": "keyword"},
                "status": {"type": "keyword"},
                "error": {"type": "text"},
                "created_at": {"type": "date"},
                "completed_at": {"type": "date"}
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

    // metadata kv helpers

    pub(crate) fn get_metadata_int(&self, key: &str) -> IcmResult<Option<i64>> {
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

    pub(crate) fn set_metadata_int(&self, key: &str, value: i64) -> IcmResult<()> {
        let path = format!("{IDX_METADATA}/_doc/{key}?refresh=true");
        self.request("PUT", &path, Some(json!({"value": value})), false)?;
        Ok(())
    }

    pub(crate) fn check_dims(&self, memory: &Memory) -> IcmResult<()> {
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
}
