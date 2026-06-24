//! PostgreSQL storage backend (issue #301, opt-in via `--features postgres`).
//!
//! A node-local SQLite file cannot be shared between several ICM
//! processes or Kubernetes replicas. This backend runs the same memory
//! model over a network-accessible PostgreSQL database so every instance
//! reads and writes one shared store. PostgreSQL serialises concurrent
//! writers, so N replicas can `icm store` into the same memory safely.
//!
//! Design notes:
//!
//! - **Blocking client.** The store traits are synchronous
//!   (`fn store(&self, ...) -> IcmResult<...>`), so we use the blocking
//!   `postgres` crate. No async runtime, no sync-over-async bridge — the
//!   client maps one-to-one onto the trait surface.
//! - **`pgvector` for embeddings.** Memory embeddings live in a
//!   `vector(N)` column; KNN search uses the `<=>` cosine-distance
//!   operator. Similarity is reported as `1 - distance` to match the
//!   SQLite backend.
//! - **PostgreSQL full-text search** replaces SQLite FTS5: a generated
//!   `tsvector` column (config `simple`, no stemming, to mirror FTS5's
//!   unicode61 tokenizer) with a GIN index, queried via
//!   `websearch_to_tsquery` so arbitrary user input is operator-safe.
//! - **Connection string** comes from `ICM_POSTGRES_URL` (or
//!   `DATABASE_URL` as a fallback). The `&Path` arguments that the CLI
//!   passes for the SQLite file are ignored.
//!
//! Scope of this first cut: the full [`MemoryStore`] surface (the core
//! shared-memory use case behind #301) plus the ancillary tables used by
//! the normal store/recall/hook path (hook telemetry, the extraction
//! queue, code areas, the key/value metadata). The heavier subsystems
//! (memoir graph, transcripts, structured facts, feedback, pattern
//! mining) return [`IcmError::Unsupported`] on this backend for now;
//! they remain fully available on the default SQLite backend.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use postgres::types::ToSql;
use postgres::{Client, GenericClient, NoTls};

use icm_core::{
    Concept, ConceptLink, Embedder, Fact, FactsStats, FactsStore, Feedback, FeedbackStats,
    FeedbackStore, IcmError, IcmResult, Importance, Label, Memoir, MemoirStats, MemoirStore,
    Memory, MemorySource, MemoryStore, Message, PatternCluster, Relation, Role, Session,
    StoreStats, TopicHealth, TranscriptHit, TranscriptStats, TranscriptStore,
};

// Shared public row types live in `crate::common` (issue #301) so every
// backend can be compiled into one binary without colliding definitions.
pub use crate::common::{CodeArea, HookEvent, HookEventInsert, HookStatsRow, PendingRow};

// ---------------------------------------------------------------------------
// Helpers (mirrored from the SQLite backend so behaviour matches)
// ---------------------------------------------------------------------------

fn pg_err(e: postgres::Error) -> IcmError {
    IcmError::Database(e.to_string())
}

fn lock_err() -> IcmError {
    IcmError::Database("postgres client mutex poisoned".into())
}

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
        Importance::Critical => 4,
        Importance::High => 3,
        Importance::Medium => 2,
        Importance::Low => 1,
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
/// Identical normalization to the SQLite backend so dedup hashes match.
fn summary_hash(topic: &str, summary: &str) -> String {
    use sha2::{Digest, Sha256};
    let topic_n = topic.trim().to_lowercase();
    let summary_n: String = summary
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

const MAX_SUMMARY_BYTES: usize = 64 * 1024;
const MAX_TOPIC_BYTES: usize = 256;

/// Validate and normalize a `Memory` before insertion. Mirrors the
/// SQLite backend's `validate_and_normalize`.
fn validate_and_normalize(mut memory: Memory) -> IcmResult<Memory> {
    memory.topic = memory.topic.trim().to_string();

    if memory.topic.is_empty() {
        return Err(IcmError::InvalidInput("topic cannot be empty".into()));
    }
    if memory.summary.trim().is_empty() {
        return Err(IcmError::InvalidInput("summary cannot be empty".into()));
    }
    if memory.topic.contains('\0') {
        return Err(IcmError::InvalidInput(
            "topic must not contain NUL bytes".into(),
        ));
    }
    if memory.summary.contains('\0') {
        return Err(IcmError::InvalidInput(
            "summary must not contain NUL bytes".into(),
        ));
    }
    if memory.topic.contains(['\n', '\r', '\t']) {
        return Err(IcmError::InvalidInput(
            "topic must not contain newline / CR / tab characters".into(),
        ));
    }
    if memory.topic.len() > MAX_TOPIC_BYTES {
        return Err(IcmError::InvalidInput(format!(
            "topic exceeds {MAX_TOPIC_BYTES} bytes"
        )));
    }
    if memory.summary.len() > MAX_SUMMARY_BYTES {
        return Err(IcmError::InvalidInput(format!(
            "summary exceeds {MAX_SUMMARY_BYTES} bytes"
        )));
    }
    Ok(memory)
}

const SELECT_COLS: &str = "id, created_at, updated_at, last_accessed, access_count, weight, \
                           topic, summary, raw_excerpt, keywords, \
                           importance, source_type, source_data, related_ids, embedding";

/// Map a `memories` row (selected via [`SELECT_COLS`]) to a [`Memory`].
fn row_to_memory(row: &postgres::Row) -> Memory {
    let keywords_json: Option<String> = row.get(9);
    let keywords: Vec<String> = keywords_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let importance_str: String = row.get(10);
    let importance = importance_str.parse().unwrap_or(Importance::Medium);

    let source_type_str: String = row.get(11);
    let source_data_str: Option<String> = row.get(12);
    let source = parse_source(&source_type_str, source_data_str);

    let related_json: Option<String> = row.get(13);
    let related_ids: Vec<String> = related_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let embedding: Option<Vec<f32>> = row
        .get::<_, Option<pgvector::Vector>>(14)
        .map(|v| v.as_slice().to_vec());

    let access_count: i32 = row.get(4);

    Memory {
        id: row.get(0),
        created_at: row.get(1),
        updated_at: row.get(2),
        last_accessed: row.get(3),
        access_count: access_count.max(0) as u32,
        weight: row.get(5),
        topic: row.get(6),
        summary: row.get(7),
        raw_excerpt: row.get(8),
        keywords,
        importance,
        source,
        related_ids,
        embedding,
        scope: icm_core::Scope::User,
    }
}

/// Insert a memory, or merge metadata into an existing duplicate.
///
/// Dedup contract identical to the SQLite backend: a collision on
/// `(LOWER(topic), summary_hash)` is ignored and the existing row's id is
/// returned, after merging the caller's importance (take max), keywords
/// (union), and `raw_excerpt` (prefer new) into it.
fn insert_or_merge_memory<C: GenericClient>(c: &mut C, memory: &Memory) -> IcmResult<String> {
    let keywords_json = serde_json::to_string(&memory.keywords)?;
    let related_json = serde_json::to_string(&memory.related_ids)?;
    let st = source_type(&memory.source);
    let sd = source_data(&memory.source);
    let hash = summary_hash(&memory.topic, &memory.summary);
    let importance = memory.importance.to_string();
    let access = memory.access_count as i32;
    let emb: Option<pgvector::Vector> = memory
        .embedding
        .as_ref()
        .map(|e| pgvector::Vector::from(e.clone()));

    let inserted = c
        .query_opt(
            "INSERT INTO memories
             (id, created_at, updated_at, last_accessed, access_count, weight,
              topic, summary, raw_excerpt, keywords, importance,
              source_type, source_data, related_ids, summary_hash, embedding)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
             ON CONFLICT (LOWER(topic), summary_hash) WHERE summary_hash IS NOT NULL
             DO NOTHING
             RETURNING id",
            &[
                &memory.id,
                &memory.created_at,
                &memory.updated_at,
                &memory.last_accessed,
                &access,
                &memory.weight,
                &memory.topic,
                &memory.summary,
                &memory.raw_excerpt,
                &keywords_json,
                &importance,
                &st,
                &sd,
                &related_json,
                &hash,
                &emb,
            ],
        )
        .map_err(pg_err)?;

    if let Some(row) = inserted {
        return Ok(row.get::<_, String>(0));
    }

    // Dedup hit: merge metadata into the existing row (mirrors SQLite).
    let existing = c
        .query_one(
            "SELECT id, importance, keywords, raw_excerpt FROM memories
             WHERE LOWER(topic) = LOWER($1) AND summary_hash = $2",
            &[&memory.topic, &hash],
        )
        .map_err(pg_err)?;

    let existing_id: String = existing.get(0);
    let existing_importance_str: String = existing.get(1);
    let existing_keywords_json: Option<String> = existing.get(2);
    let existing_raw: Option<String> = existing.get(3);

    let existing_importance: Importance = existing_importance_str
        .parse()
        .unwrap_or(Importance::Medium);
    let merged_importance = max_importance(existing_importance, memory.importance);

    let existing_keywords: Vec<String> = existing_keywords_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut merged_keywords = existing_keywords.clone();
    for kw in &memory.keywords {
        if !merged_keywords.contains(kw) {
            merged_keywords.push(kw.clone());
        }
    }

    let merged_raw = memory.raw_excerpt.clone().or_else(|| existing_raw.clone());

    let importance_changed = merged_importance != existing_importance;
    let keywords_changed = merged_keywords != existing_keywords;
    let raw_changed = merged_raw != existing_raw;
    if importance_changed || keywords_changed || raw_changed {
        let merged_keywords_json = serde_json::to_string(&merged_keywords)?;
        c.execute(
            "UPDATE memories
             SET importance = $1, keywords = $2, raw_excerpt = $3, updated_at = $4
             WHERE id = $5",
            &[
                &merged_importance.to_string(),
                &merged_keywords_json,
                &merged_raw,
                &Utc::now(),
                &existing_id,
            ],
        )
        .map_err(pg_err)?;
    }

    Ok(existing_id)
}

// ---------------------------------------------------------------------------
// PostgresStore
// ---------------------------------------------------------------------------

/// PostgreSQL-backed store. See the module docs.
pub struct PostgresStore {
    client: Mutex<Client>,
    embedding_dims: usize,
    readonly: bool,
}

impl PostgresStore {
    fn conn(&self) -> IcmResult<MutexGuard<'_, Client>> {
        self.client.lock().map_err(|_| lock_err())
    }

    /// Resolve the connection string from the environment.
    fn conn_string() -> IcmResult<String> {
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
    fn connect(requested_dims: usize, readonly: bool) -> IcmResult<Self> {
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

    /// Apply decay if more than 24 hours since the last run. Mirrors the
    /// SQLite backend's atomic check-and-claim via `icm_metadata`.
    pub fn maybe_auto_decay(&self) -> IcmResult<()> {
        if self.readonly {
            return Ok(());
        }
        let now = Utc::now();
        let claimed = {
            let mut c = self.conn()?;
            c.execute(
                "INSERT INTO icm_metadata (key, value) VALUES ('last_decay_at', $1)
                 ON CONFLICT (key) DO UPDATE SET value = $1
                 WHERE icm_metadata.value IS NULL
                    OR ($1::timestamptz - icm_metadata.value::timestamptz) >= interval '1 day'",
                &[&now.to_rfc3339()],
            )
            .map_err(pg_err)?
        };
        if claimed > 0 {
            self.apply_decay(0.95)?;
        }
        Ok(())
    }

    /// Atomically increment the hook call counter and return the new value.
    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '1')
                 ON CONFLICT (key) DO UPDATE SET value = ((icm_metadata.value::bigint) + 1)::text
                 RETURNING value::bigint",
                &[],
            )
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    /// Reset the hook call counter to 0.
    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '0')
             ON CONFLICT (key) DO UPDATE SET value = '0'",
            &[],
        )
        .map_err(pg_err)?;
        Ok(())
    }

    // ── Async extraction queue ─────────────────────────────────────────

    /// Enqueue raw tool output for later LLM extraction.
    pub fn enqueue_pending_extraction(
        &self,
        project: &str,
        tool_name: &str,
        raw_output: &str,
    ) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO pending_extractions (id, project, tool_name, raw_output, captured_at)
             VALUES ($1, $2, $3, $4, $5)",
            &[&id, &project, &tool_name, &raw_output, &Utc::now()],
        )
        .map_err(pg_err)?;
        Ok(id)
    }

    /// Pop up to `limit` oldest pending rows (FIFO by capture time).
    pub fn list_pending_extractions(&self, limit: usize) -> IcmResult<Vec<PendingRow>> {
        let mut c = self.conn()?;
        let rows = c
            .query(
                "SELECT id, project, tool_name, raw_output, captured_at
                 FROM pending_extractions
                 ORDER BY captured_at ASC
                 LIMIT $1",
                &[&(limit as i64)],
            )
            .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let captured: DateTime<Utc> = row.get(4);
                (
                    row.get(0),
                    row.get(1),
                    row.get(2),
                    row.get(3),
                    captured.to_rfc3339(),
                )
            })
            .collect())
    }

    /// Delete pending rows by id. Used after a worker has processed them.
    pub fn delete_pending_extractions(&self, ids: &[String]) -> IcmResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let ids_vec: Vec<String> = ids.to_vec();
        let mut c = self.conn()?;
        let n = c
            .execute(
                "DELETE FROM pending_extractions WHERE id = ANY($1)",
                &[&ids_vec],
            )
            .map_err(pg_err)?;
        Ok(n as usize)
    }

    /// Total rows currently waiting in the queue.
    pub fn pending_extraction_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM pending_extractions", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    // ── Code areas (issue #196) ────────────────────────────────────────

    /// Insert or refresh a row for `(project, file_path)`.
    pub fn upsert_code_area(
        &self,
        project: &str,
        file_path: &str,
        description: Option<&str>,
        session_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> IcmResult<()> {
        let now = Utc::now();
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO code_areas
                (project, file_path, description, session_id, tool_name,
                 touch_count, first_touched_at, last_touched_at)
             VALUES ($1, $2, $3, $4, $5, 1, $6, $6)
             ON CONFLICT (project, file_path) DO UPDATE SET
                touch_count = code_areas.touch_count + 1,
                last_touched_at = EXCLUDED.last_touched_at,
                session_id = COALESCE(EXCLUDED.session_id, code_areas.session_id),
                tool_name = COALESCE(EXCLUDED.tool_name, code_areas.tool_name),
                description = COALESCE(EXCLUDED.description, code_areas.description)",
            &[
                &project,
                &file_path,
                &description,
                &session_id,
                &tool_name,
                &now,
            ],
        )
        .map_err(pg_err)?;
        Ok(())
    }

    /// List code areas, optionally filtered, newest-touch first.
    pub fn list_code_areas(
        &self,
        project: Option<&str>,
        in_file: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> IcmResult<Vec<CodeArea>> {
        let mut sql = String::from(
            "SELECT id, project, file_path, description, session_id, tool_name,
                    touch_count, first_touched_at, last_touched_at
             FROM code_areas WHERE TRUE",
        );
        let mut owned: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        if let Some(p) = project {
            owned.push(Box::new(p.to_string()));
            sql.push_str(&format!(" AND project = ${}", owned.len()));
        }
        if let Some(f) = in_file {
            owned.push(Box::new(f.to_string()));
            let exact = owned.len();
            owned.push(Box::new(format!("%/{f}")));
            let suffix = owned.len();
            sql.push_str(&format!(
                " AND (file_path = ${exact} OR file_path LIKE ${suffix})"
            ));
        }
        if let Some(t) = since {
            owned.push(Box::new(t));
            sql.push_str(&format!(" AND last_touched_at >= ${}", owned.len()));
        }
        owned.push(Box::new(limit as i64));
        sql.push_str(&format!(
            " ORDER BY last_touched_at DESC LIMIT ${}",
            owned.len()
        ));

        let params: Vec<&(dyn ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
        let mut c = self.conn()?;
        let rows = c.query(&sql, &params).map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| CodeArea {
                id: row.get(0),
                project: row.get(1),
                file_path: row.get(2),
                description: row.get(3),
                session_id: row.get(4),
                tool_name: row.get(5),
                touch_count: row.get(6),
                first_touched_at: row.get(7),
                last_touched_at: row.get(8),
            })
            .collect())
    }

    /// Total rows in `code_areas`.
    pub fn code_area_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM code_areas", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    // ── Hook telemetry ─────────────────────────────────────────────────

    /// Append one hook telemetry row, returning its id.
    pub fn record_hook_event(&self, ev: &HookEventInsert) -> IcmResult<i64> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "INSERT INTO hook_events
                 (ts, event, project, session_id, tool_name,
                  duration_ms, exit_code, payload_size, note)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 RETURNING id",
                &[
                    &Utc::now(),
                    &ev.event,
                    &ev.project,
                    &ev.session_id,
                    &ev.tool_name,
                    &ev.duration_ms,
                    &ev.exit_code,
                    &ev.payload_size,
                    &ev.note,
                ],
            )
            .map_err(pg_err)?;
        Ok(row.get(0))
    }

    /// Most recent `limit` hook events, newest first; optional event filter.
    pub fn hook_events_recent(
        &self,
        limit: usize,
        event_filter: Option<&str>,
    ) -> IcmResult<Vec<HookEvent>> {
        let mut c = self.conn()?;
        let rows = match event_filter {
            Some(ev) => c.query(
                "SELECT id, ts, event, project, session_id, tool_name,
                        duration_ms, exit_code, payload_size, note
                 FROM hook_events WHERE event = $1 ORDER BY id DESC LIMIT $2",
                &[&ev, &(limit as i64)],
            ),
            None => c.query(
                "SELECT id, ts, event, project, session_id, tool_name,
                        duration_ms, exit_code, payload_size, note
                 FROM hook_events ORDER BY id DESC LIMIT $1",
                &[&(limit as i64)],
            ),
        }
        .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| HookEvent {
                id: row.get(0),
                ts: row.get(1),
                event: row.get(2),
                project: row.get(3),
                session_id: row.get(4),
                tool_name: row.get(5),
                duration_ms: row.get(6),
                exit_code: row.get(7),
                payload_size: row.get(8),
                note: row.get(9),
            })
            .collect())
    }

    /// Per-event aggregate stats since `since_rfc3339`.
    pub fn hook_stats(&self, since_rfc3339: &str) -> IcmResult<Vec<HookStatsRow>> {
        let since = DateTime::parse_from_rfc3339(since_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now() - chrono::Duration::days(7));
        let mut c = self.conn()?;
        let rows = c
            .query(
                "SELECT event,
                        COUNT(*)::bigint,
                        COUNT(*) FILTER (WHERE exit_code <> 0)::bigint,
                        COALESCE(AVG(duration_ms), 0)::float8,
                        COALESCE(percentile_cont(0.5) WITHIN GROUP (ORDER BY duration_ms), 0)::float8,
                        COALESCE(percentile_cont(0.99) WITHIN GROUP (ORDER BY duration_ms), 0)::float8
                 FROM hook_events WHERE ts >= $1
                 GROUP BY event ORDER BY event",
                &[&since],
            )
            .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let p50: f64 = row.get(4);
                let p99: f64 = row.get(5);
                HookStatsRow {
                    event: row.get(0),
                    count: row.get(1),
                    error_count: row.get(2),
                    avg_duration_ms: row.get(3),
                    p50_duration_ms: p50 as i64,
                    p99_duration_ms: p99 as i64,
                }
            })
            .collect())
    }

    /// Delete hook events older than `cutoff_rfc3339`.
    pub fn prune_hook_events(&self, cutoff_rfc3339: &str) -> IcmResult<usize> {
        let cutoff = DateTime::parse_from_rfc3339(cutoff_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| IcmError::InvalidInput(format!("invalid cutoff timestamp: {e}")))?;
        let mut c = self.conn()?;
        let n = c
            .execute("DELETE FROM hook_events WHERE ts < $1", &[&cutoff])
            .map_err(pg_err)?;
        Ok(n as usize)
    }

    /// Total rows in `hook_events`.
    pub fn hook_event_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM hook_events", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    // ── Memory reads used by recall expansion ──────────────────────────

    /// Fetch many memories by id in one round-trip, deduplicated by id.
    pub fn get_many(&self, ids: &[&str]) -> IcmResult<HashMap<String, Memory>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let id_vec: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let mut c = self.conn()?;
        let rows = c
            .query(
                &format!("SELECT {SELECT_COLS} FROM memories WHERE id = ANY($1)"),
                &[&id_vec],
            )
            .map_err(pg_err)?;
        let mut map = HashMap::with_capacity(rows.len());
        for row in &rows {
            let m = row_to_memory(row);
            map.insert(m.id.clone(), m);
        }
        Ok(map)
    }

    /// Expand a scored result set with one hop of related memories.
    /// Backend-agnostic logic mirrored from the SQLite store.
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
        'outer: for (mem, score) in initial {
            for neighbor_id in &mem.related_ids {
                if candidates.len() >= max_neighbors {
                    break 'outer;
                }
                if initial_ids.contains(neighbor_id) || !seen.insert(neighbor_id.clone()) {
                    continue;
                }
                candidates.push((neighbor_id.clone(), *score));
            }
        }

        let mut neighbors: Vec<(Memory, f32)> = Vec::new();
        if !candidates.is_empty() {
            let ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
            let fetched = self.get_many(&ids)?;
            for (id, parent_score) in candidates {
                if let Some(m) = fetched.get(&id) {
                    neighbors.push((m.clone(), parent_score * hop_discount));
                }
            }
        }

        let mut combined: Vec<(Memory, f32)> = initial.to_vec();
        combined.extend(neighbors);
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(max_total);
        Ok(combined)
    }

    /// Memories whose topic starts with `topic` (prefix match).
    pub fn get_by_topic_prefix(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let pattern = format!("{topic}%");
        let mut c = self.conn()?;
        let rows = c
            .query(
                &format!(
                    "SELECT {SELECT_COLS} FROM memories WHERE topic LIKE $1 \
                     ORDER BY weight DESC LIMIT 500"
                ),
                &[&pattern],
            )
            .map_err(pg_err)?;
        Ok(rows.iter().map(row_to_memory).collect())
    }

    /// Distinct topics (optionally prefix-filtered) with their counts.
    pub fn list_topics_with_prefix(&self, prefix: Option<&str>) -> IcmResult<Vec<(String, usize)>> {
        let mut c = self.conn()?;
        let rows = match prefix {
            Some(p) => {
                let pattern = format!("{p}%");
                c.query(
                    "SELECT topic, COUNT(*)::bigint FROM memories WHERE topic LIKE $1 \
                     GROUP BY topic ORDER BY topic",
                    &[&pattern],
                )
            }
            None => c.query(
                "SELECT topic, COUNT(*)::bigint FROM memories GROUP BY topic ORDER BY topic",
                &[],
            ),
        }
        .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let n: i64 = row.get(1);
                (row.get(0), n.max(0) as usize)
            })
            .collect())
    }

    // ── Consolidation / patterns ───────────────────────────────────────

    /// Auto-consolidation is not yet implemented on the PostgreSQL
    /// backend; the call is a no-op (returns "did not consolidate") so the
    /// normal store path is unaffected.
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

    /// Pattern mining is not yet available on the PostgreSQL backend.
    pub fn detect_patterns(
        &self,
        _topic: &str,
        _min_cluster_size: usize,
    ) -> IcmResult<Vec<PatternCluster>> {
        Err(IcmError::Unsupported(
            "detect_patterns (use the default SQLite backend)".into(),
        ))
    }

    /// Pattern mining is not yet available on the PostgreSQL backend.
    pub fn extract_pattern_as_concept(
        &self,
        _cluster: &PatternCluster,
        _memoir_id: &str,
    ) -> IcmResult<String> {
        Err(IcmError::Unsupported(
            "extract_pattern_as_concept (use the default SQLite backend)".into(),
        ))
    }
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
                ON memories (LOWER(topic), summary_hash) WHERE summary_hash IS NOT NULL;

            CREATE TABLE IF NOT EXISTS pending_extractions (
                id TEXT PRIMARY KEY,
                project TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                raw_output TEXT NOT NULL,
                captured_at TIMESTAMPTZ NOT NULL
            );

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

// ---------------------------------------------------------------------------
// MemoryStore
// ---------------------------------------------------------------------------

impl MemoryStore for PostgresStore {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        if self.readonly {
            return Err(IcmError::ReadOnly("store".into()));
        }
        let memory = validate_and_normalize(memory)?;
        self.check_dims(&memory)?;
        let mut c = self.conn()?;
        let mut tx = c.transaction().map_err(pg_err)?;
        let id = insert_or_merge_memory(&mut tx, &memory)?;
        tx.commit().map_err(pg_err)?;
        Ok(id)
    }

    fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
        let mut c = self.conn()?;
        let row = c
            .query_opt(
                &format!("SELECT {SELECT_COLS} FROM memories WHERE id = $1"),
                &[&id],
            )
            .map_err(pg_err)?;
        Ok(row.as_ref().map(row_to_memory))
    }

    fn update(&self, memory: &Memory) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("update".into()));
        }
        self.check_dims(memory)?;
        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);
        let hash = summary_hash(&memory.topic, &memory.summary);
        let importance = memory.importance.to_string();
        let access = memory.access_count as i32;
        let emb: Option<pgvector::Vector> = memory
            .embedding
            .as_ref()
            .map(|e| pgvector::Vector::from(e.clone()));

        let mut c = self.conn()?;
        let changed = c
            .execute(
                "UPDATE memories SET
                    updated_at = $2, last_accessed = $3, access_count = $4, weight = $5,
                    topic = $6, summary = $7, raw_excerpt = $8, keywords = $9,
                    importance = $10, source_type = $11, source_data = $12, related_ids = $13,
                    embedding = $14, summary_hash = $15
                 WHERE id = $1",
                &[
                    &memory.id,
                    &memory.updated_at,
                    &memory.last_accessed,
                    &access,
                    &memory.weight,
                    &memory.topic,
                    &memory.summary,
                    &memory.raw_excerpt,
                    &keywords_json,
                    &importance,
                    &st,
                    &sd,
                    &related_json,
                    &emb,
                    &hash,
                ],
            )
            .map_err(pg_err)?;
        if changed == 0 {
            return Err(IcmError::NotFound(memory.id.clone()));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("delete".into()));
        }
        let mut c = self.conn()?;
        let changed = c
            .execute("DELETE FROM memories WHERE id = $1", &[&id])
            .map_err(pg_err)?;
        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }
        let keywords = &keywords[..keywords.len().min(50)];
        let limit = limit.min(100);

        let mut owned: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        let mut where_parts: Vec<String> = Vec::new();
        for k in keywords {
            owned.push(Box::new(format!("%{k}%")));
            let p = owned.len();
            where_parts.push(format!(
                "(keywords ILIKE ${p} OR summary ILIKE ${p} OR topic ILIKE ${p})"
            ));
        }
        owned.push(Box::new(limit as i64));
        let sql = format!(
            "SELECT {SELECT_COLS} FROM memories WHERE {} ORDER BY weight DESC LIMIT ${}",
            where_parts.join(" OR "),
            owned.len()
        );
        let params: Vec<&(dyn ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
        let mut c = self.conn()?;
        let rows = c.query(&sql, &params).map_err(pg_err)?;
        Ok(rows.iter().map(row_to_memory).collect())
    }

    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>> {
        let limit = limit.min(100);
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut c = self.conn()?;
        let rows = c
            .query(
                &format!(
                    "SELECT {SELECT_COLS} FROM memories \
                     WHERE fts @@ websearch_to_tsquery('simple', $1) \
                     ORDER BY weight DESC LIMIT $2"
                ),
                &[&query, &(limit as i64)],
            )
            .map_err(pg_err)?;
        Ok(rows.iter().map(row_to_memory).collect())
    }

    fn search_by_embedding(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let qv = pgvector::Vector::from(embedding.to_vec());
        let mut c = self.conn()?;
        let rows = c
            .query(
                &format!(
                    "SELECT {SELECT_COLS}, embedding <=> $1 AS distance FROM memories \
                     WHERE embedding IS NOT NULL ORDER BY embedding <=> $1 LIMIT $2"
                ),
                &[&qv, &(limit as i64)],
            )
            .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let distance: f64 = row.get(15);
                (row_to_memory(row), 1.0 - distance as f32)
            })
            .collect())
    }

    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let limit = limit.min(1000);
        let pool_size = limit * 4;

        // 1. FTS candidates (id + rank). Lock released at scope end.
        let fts_pairs: Vec<(String, f64)> = if query.trim().is_empty() {
            Vec::new()
        } else {
            let mut c = self.conn()?;
            let rows = c
                .query(
                    "SELECT id, ts_rank_cd(fts, websearch_to_tsquery('simple', $1))::float8 AS rank \
                     FROM memories \
                     WHERE fts @@ websearch_to_tsquery('simple', $1) \
                     ORDER BY rank DESC LIMIT $2",
                    &[&query, &(pool_size as i64)],
                )
                .map_err(pg_err)?;
            rows.iter().map(|r| (r.get(0), r.get(1))).collect()
        };

        // 2. Vector candidates (full rows + similarity).
        let vec_results = self.search_by_embedding(embedding, pool_size)?;

        // 3. Assemble memory objects and per-source scores.
        let mut all_memories: HashMap<String, Memory> = HashMap::new();
        let mut vec_scores: HashMap<String, f32> = HashMap::new();
        for (mem, sim) in vec_results {
            vec_scores.insert(mem.id.clone(), sim);
            all_memories.insert(mem.id.clone(), mem);
        }

        // Normalize FTS ranks into 0..1 within the pool (higher is better).
        let max_rank = fts_pairs.iter().map(|(_, r)| *r).fold(0.0_f64, f64::max);
        let mut fts_scores: HashMap<String, f32> = HashMap::new();
        let missing: Vec<String> = fts_pairs
            .iter()
            .filter(|(id, _)| !all_memories.contains_key(id))
            .map(|(id, _)| id.clone())
            .collect();
        if !missing.is_empty() {
            let refs: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            let fetched = self.get_many(&refs)?;
            for (id, m) in fetched {
                all_memories.insert(id, m);
            }
        }
        for (id, rank) in fts_pairs {
            let score = if max_rank > 0.0 {
                (rank / max_rank) as f32
            } else {
                0.0
            };
            fts_scores.insert(id, score);
        }

        // 4. Blend: 30% FTS + 70% vector (matches the SQLite backend).
        let mut scored: Vec<(String, f32)> = all_memories
            .keys()
            .map(|id| {
                let fts = fts_scores.get(id).copied().unwrap_or(0.0);
                let vec = vec_scores.get(id).copied().unwrap_or(0.0);
                (id.clone(), 0.3 * fts + 0.7 * vec)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .filter_map(|(id, score)| all_memories.remove(&id).map(|m| (m, score)))
            .collect())
    }

    fn update_access(&self, id: &str) -> IcmResult<()> {
        if self.readonly {
            return Ok(());
        }
        let mut c = self.conn()?;
        let changed = c
            .execute(
                "UPDATE memories SET last_accessed = $1, access_count = access_count + 1 \
                 WHERE id = $2",
                &[&Utc::now(), &id],
            )
            .map_err(pg_err)?;
        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize> {
        if ids.is_empty() || self.readonly {
            return Ok(0);
        }
        let id_vec: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let mut c = self.conn()?;
        let changed = c
            .execute(
                "UPDATE memories SET last_accessed = $1, access_count = access_count + 1 \
                 WHERE id = ANY($2)",
                &[&Utc::now(), &id_vec],
            )
            .map_err(pg_err)?;
        Ok(changed as usize)
    }

    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly("apply_decay".into()));
        }
        // Access-aware decay, capped at 5 accesses (matches SQLite).
        let mut c = self.conn()?;
        let changed = c
            .execute(
                // `$1::float8` is explicit so PostgreSQL doesn't infer the
                // parameter as `numeric`/`real` from a neighbouring operand
                // and reject the `f64` we bind ("error serializing parameter").
                "UPDATE memories SET weight = weight * (
                    1.0 - (1.0 - $1::float8) *
                    CASE importance
                        WHEN 'high' THEN 0.5
                        WHEN 'low' THEN 2.0
                        ELSE 1.0
                    END
                    / (1.0 + LEAST(access_count, 5) * 0.1)
                )
                WHERE importance <> 'critical'",
                &[&(decay_factor as f64)],
            )
            .map_err(pg_err)?;
        Ok(changed as usize)
    }

    fn prune(&self, weight_threshold: f32) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly("prune".into()));
        }
        let mut c = self.conn()?;
        let changed = c
            .execute(
                "DELETE FROM memories \
                 WHERE weight < $1::float8 AND importance NOT IN ('critical', 'high')",
                &[&(weight_threshold as f64)],
            )
            .map_err(pg_err)?;
        Ok(changed as usize)
    }

    fn list_all(&self) -> IcmResult<Vec<Memory>> {
        let mut c = self.conn()?;
        let rows = c
            .query(
                &format!("SELECT {SELECT_COLS} FROM memories ORDER BY weight DESC LIMIT 10000"),
                &[],
            )
            .map_err(pg_err)?;
        Ok(rows.iter().map(row_to_memory).collect())
    }

    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let mut c = self.conn()?;
        let rows = c
            .query(
                &format!(
                    "SELECT {SELECT_COLS} FROM memories WHERE topic = $1 \
                     ORDER BY weight DESC LIMIT 500"
                ),
                &[&topic],
            )
            .map_err(pg_err)?;
        Ok(rows.iter().map(row_to_memory).collect())
    }

    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
        self.list_topics_with_prefix(None)
    }

    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("consolidate_topic".into()));
        }
        let mut c = self.conn()?;
        let mut tx = c.transaction().map_err(pg_err)?;
        tx.execute("DELETE FROM memories WHERE topic = $1", &[&topic])
            .map_err(pg_err)?;
        insert_or_merge_memory(&mut tx, &consolidated)?;
        tx.commit().map_err(pg_err)?;
        Ok(())
    }

    fn count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM memories", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    fn count_by_topic(&self, topic: &str) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM memories WHERE topic = $1", &[&topic])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    fn stats(&self) -> IcmResult<StoreStats> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "SELECT COUNT(*)::bigint, COUNT(DISTINCT topic)::bigint, \
                        COALESCE(AVG(weight), 0.0)::float8, MIN(created_at), MAX(created_at) \
                 FROM memories",
                &[],
            )
            .map_err(pg_err)?;
        let total: i64 = row.get(0);
        let topics: i64 = row.get(1);
        let avg: f64 = row.get(2);
        Ok(StoreStats {
            total_memories: total.max(0) as usize,
            total_topics: topics.max(0) as usize,
            avg_weight: avg as f32,
            oldest_memory: row.get(3),
            newest_memory: row.get(4),
        })
    }

    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "SELECT COUNT(*)::bigint,
                        COALESCE(AVG(weight), 0)::float8,
                        COALESCE(AVG(access_count::float8), 0)::float8,
                        MIN(created_at), MAX(created_at), MAX(last_accessed),
                        COALESCE(SUM(CASE WHEN weight < 0.5
                              AND (now() - last_accessed) > interval '14 days'
                              THEN 1 ELSE 0 END), 0)::bigint
                 FROM memories WHERE topic = $1",
                &[&topic],
            )
            .map_err(pg_err)?;

        let entry_count: i64 = row.get(0);
        if entry_count == 0 {
            return Err(IcmError::NotFound(format!("topic: {topic}")));
        }
        let avg_weight: f64 = row.get(1);
        let avg_access: f64 = row.get(2);
        let stale: i64 = row.get(6);

        Ok(TopicHealth {
            topic: topic.to_string(),
            entry_count: entry_count.max(0) as usize,
            avg_weight: avg_weight as f32,
            avg_access_count: avg_access as f32,
            oldest: row.get(3),
            newest: row.get(4),
            last_accessed: row.get(5),
            needs_consolidation: entry_count > 5,
            stale_count: stale.max(0) as usize,
        })
    }
}

// ---------------------------------------------------------------------------
// Unsupported subsystems on this backend (first cut, issue #301).
//
// These return `IcmError::Unsupported` so the binary keeps working for the
// core shared-memory use case while the heavier subsystems remain on the
// default SQLite backend. A follow-up can port them.
// ---------------------------------------------------------------------------

fn unsupported<T>(op: &str) -> IcmResult<T> {
    Err(IcmError::Unsupported(format!(
        "{op} (use the default SQLite backend)"
    )))
}

impl MemoirStore for PostgresStore {
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

impl FeedbackStore for PostgresStore {
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

impl FactsStore for PostgresStore {
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

impl TranscriptStore for PostgresStore {
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
