//! PostgreSQL backend -- split out of the former monolithic postgres.rs.
//!
//! Connection setup, constructors, and schema migration.

use super::*;

impl PostgresStore {
    pub(crate) fn conn(&self) -> IcmResult<MutexGuard<'_, Client>> {
        self.client.lock().map_err(|_| lock_err())
    }

    /// Resolve the connection string from the environment.
    pub(crate) fn conn_string() -> IcmResult<String> {
        std::env::var("ICM_POSTGRES_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| {
                IcmError::Config(
                    "PostgreSQL backend: set ICM_POSTGRES_URL (or DATABASE_URL) to the \
                     connection string, e.g. postgres://user:pass@host:5432/icm"
                        .into(),
                )
            })
    }

    /// Connect and run the idempotent schema migration.
    ///
    /// The `&Path` the CLI passes for the SQLite file is ignored; the
    /// connection comes from the environment. `requested_dims` is used
    /// only when the database is fresh — an existing database's stored
    /// `embedding_dims` is authoritative so we never try to declare a
    /// `vector(N)` column that disagrees with the live table.
    pub(crate) fn connect(requested_dims: usize, readonly: bool) -> IcmResult<Self> {
        let url = Self::conn_string()?;
        let mut client = Client::connect(&url, NoTls)
            .map_err(|e| IcmError::Database(format!("cannot connect to PostgreSQL: {e}")))?;

        let dims = init_schema(&mut client, requested_dims)?;

        Ok(Self {
            client: Mutex::new(client),
            embedding_dims: dims,
            readonly,
        })
    }

    /// Reject an embedding whose length disagrees with the column's
    /// declared dimension, with a clearer message than the raw PostgreSQL
    /// "expected N dimensions, not M" error.
    pub(crate) fn check_dims(&self, memory: &Memory) -> IcmResult<()> {
        if let Some(emb) = memory.embedding.as_ref()
            && emb.len() != self.embedding_dims
        {
            return Err(IcmError::InvalidInput(format!(
                "embedding has {} dimensions, but this store uses {}",
                emb.len(),
                self.embedding_dims
            )));
        }
        Ok(())
    }

    /// Open or create a store with the default embedding dimension.
    pub fn new(_path: &Path) -> IcmResult<Self> {
        Self::connect(icm_core::DEFAULT_EMBEDDING_DIMS, false)
    }

    /// Open or create a store with a specific embedding dimension.
    pub fn with_dims(_path: &Path, embedding_dims: usize) -> IcmResult<Self> {
        Self::connect(embedding_dims, false)
    }

    /// Open the store in read-only mode (issue #263). The connection is
    /// the same; write methods refuse, read-like side effects are skipped.
    pub fn open_readonly(_path: &Path) -> IcmResult<Self> {
        Self::connect(icm_core::DEFAULT_EMBEDDING_DIMS, true)
    }

    /// PostgreSQL has no in-memory mode; connect to the configured
    /// database. Provided for API parity with the SQLite backend.
    pub fn in_memory() -> IcmResult<Self> {
        Self::connect(icm_core::DEFAULT_EMBEDDING_DIMS, false)
    }

    /// See [`Self::in_memory`].
    pub fn in_memory_with_dims(embedding_dims: usize) -> IcmResult<Self> {
        Self::connect(embedding_dims, false)
    }

    /// PostgreSQL stores `embedding_dims` in `icm_metadata`, but unlike
    /// SQLite it never destructively recreates the vector column, so the
    /// pre-open peek the SQLite backend needs is unnecessary here. Always
    /// returns `Ok(None)` so callers fall through to the normal open path.
    pub fn read_stored_embedding_dims(_path: &Path) -> IcmResult<Option<usize>> {
        Ok(None)
    }

    #[must_use]
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// No-op on PostgreSQL (the SQLite backend uses this to load the
    /// `sqlite-vec` extension; `pgvector` lives server-side).
    pub fn ensure_vec_init() {}
}

/// Idempotent schema creation. Returns the embedding dimension the table
/// is actually using (the stored value wins over `requested_dims` on an
/// existing database).
fn init_schema(client: &mut Client, requested_dims: usize) -> IcmResult<usize> {
    if !(64..=4096).contains(&requested_dims) {
        return Err(IcmError::Config(format!(
            "embedding_dims must be between 64 and 4096, got {requested_dims}"
        )));
    }

    client
        .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
        .map_err(|e| {
            IcmError::Database(format!(
                "cannot enable the pgvector extension (need it for embeddings): {e}"
            ))
        })?;

    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS icm_metadata (
                key TEXT PRIMARY KEY,
                value TEXT
            )",
        )
        .map_err(pg_err)?;

    // The stored dimension is authoritative on an existing database.
    let stored: Option<i64> = client
        .query_opt(
            "SELECT value::bigint FROM icm_metadata WHERE key = 'embedding_dims'",
            &[],
        )
        .map_err(pg_err)?
        .map(|row| row.get(0));
    let dims = stored.map(|d| d as usize).unwrap_or(requested_dims);

    // Migration: the unique index used to be `(LOWER(topic), summary_hash)`.
    // `summary_hash` already encodes the topic via Rust's Unicode-correct
    // `to_lowercase()`, while Postgres's `LOWER()` behavior depends on the
    // cluster's locale (ASCII-only under `C`/POSIX, common in minimal
    // Docker/CI images) — the composite key could let two rows with an
    // identical `summary_hash` coexist whenever their topic's SQL-LOWER()
    // forms differed under that locale (audit finding, same class already
    // fixed for SQLite). `CREATE INDEX IF NOT EXISTS` would silently keep
    // the old definition on an already-migrated DB (same index name), so
    // drop it first if it still has the old column list.
    let old_index_def: Option<String> = client
        .query_opt(
            "SELECT indexdef FROM pg_indexes WHERE indexname = 'idx_memories_topic_hash'",
            &[],
        )
        .map_err(pg_err)?
        .map(|row| row.get(0));
    if let Some(def) = old_index_def
        && def.to_lowercase().contains("lower(topic")
    {
        client
            .batch_execute("DROP INDEX idx_memories_topic_hash;")
            .map_err(pg_err)?;
    }

    client
        .batch_execute(&format!(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                created_at TIMESTAMPTZ NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL,
                last_accessed TIMESTAMPTZ NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                weight REAL NOT NULL DEFAULT 1.0,
                topic TEXT NOT NULL,
                summary TEXT NOT NULL,
                raw_excerpt TEXT,
                keywords TEXT,
                importance TEXT NOT NULL,
                source_type TEXT NOT NULL,
                source_data TEXT,
                related_ids TEXT,
                summary_hash TEXT,
                embedding vector({dims}),
                fts tsvector GENERATED ALWAYS AS (
                    to_tsvector('simple',
                        coalesce(topic, '') || ' ' ||
                        coalesce(summary, '') || ' ' ||
                        coalesce(keywords, ''))
                ) STORED
            );

            CREATE INDEX IF NOT EXISTS idx_memories_topic ON memories(topic);
            CREATE INDEX IF NOT EXISTS idx_memories_weight ON memories(weight);
            CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
            CREATE INDEX IF NOT EXISTS idx_memories_fts ON memories USING GIN (fts);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_topic_hash
                ON memories (summary_hash) WHERE summary_hash IS NOT NULL;

            CREATE TABLE IF NOT EXISTS pending_extractions (
                id TEXT PRIMARY KEY,
                project TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                raw_output TEXT NOT NULL,
                captured_at TIMESTAMPTZ NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pending_consolidations (
                id TEXT PRIMARY KEY,
                topic TEXT NOT NULL,
                project TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                error TEXT,
                created_at TIMESTAMPTZ NOT NULL,
                completed_at TIMESTAMPTZ
            );
            CREATE INDEX IF NOT EXISTS idx_pending_consolidations_status
                ON pending_consolidations(status, created_at);

            CREATE TABLE IF NOT EXISTS code_areas (
                id BIGSERIAL PRIMARY KEY,
                project TEXT NOT NULL,
                file_path TEXT NOT NULL,
                description TEXT,
                session_id TEXT,
                tool_name TEXT,
                touch_count BIGINT NOT NULL DEFAULT 1,
                first_touched_at TIMESTAMPTZ NOT NULL,
                last_touched_at TIMESTAMPTZ NOT NULL,
                UNIQUE (project, file_path)
            );

            CREATE TABLE IF NOT EXISTS hook_events (
                id BIGSERIAL PRIMARY KEY,
                ts TIMESTAMPTZ NOT NULL,
                event TEXT NOT NULL,
                project TEXT,
                session_id TEXT,
                tool_name TEXT,
                duration_ms BIGINT,
                exit_code INTEGER NOT NULL DEFAULT 0,
                payload_size BIGINT,
                note TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_hook_events_ts ON hook_events(ts);
            CREATE INDEX IF NOT EXISTS idx_hook_events_event ON hook_events(event);"
        ))
        .map_err(pg_err)?;

    // A vector index needs a concrete dimension; create it after the
    // table exists. HNSW is available in pgvector >= 0.5 (the images we
    // target ship a newer version).
    client
        .batch_execute(
            "CREATE INDEX IF NOT EXISTS idx_memories_embedding
                ON memories USING hnsw (embedding vector_cosine_ops)",
        )
        .map_err(pg_err)?;

    client
        .execute(
            "INSERT INTO icm_metadata (key, value) VALUES ('embedding_dims', $1)
             ON CONFLICT (key) DO NOTHING",
            &[&dims.to_string()],
        )
        .map_err(pg_err)?;

    Ok(dims)
}
