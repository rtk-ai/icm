use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Mutex, Once};

use chrono::{DateTime, Utc};
use lru::LruCache;
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection};
use zerocopy::IntoBytes;

use icm_core::{
    Concept, ConceptLink, Embedder, Feedback, FeedbackStats, FeedbackStore, IcmError, IcmResult,
    Importance, Label, Memoir, MemoirStats, MemoirStore, Memory, MemorySource, MemoryStore,
    Message, PatternCluster, Relation, Role, Session, StoreStats, TopicHealth, TranscriptHit,
    TranscriptStats, TranscriptStore,
};

use crate::schema::{init_db, init_db_with_dims};

/// Convert rusqlite::Error to IcmError::Database
pub(crate) fn db_err(e: rusqlite::Error) -> IcmError {
    IcmError::Database(e.to_string())
}

/// Collect mapped rows into a Vec, converting rusqlite errors.
fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> IcmResult<Vec<T>> {
    rows.collect::<Result<Vec<T>, _>>().map_err(db_err)
}

static SQLITE_VEC_INIT: Once = Once::new();

fn ensure_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// In-process LRU cache size for hot memories. Each entry is one
/// fully-hydrated `Memory` (incl. optional 384×f32 embedding ≈ 1.5KB),
/// so 256 entries cap RAM at ~400KB worst case. Helps long-running
/// processes (`icm serve`, TUI) where the same memories are read
/// repeatedly; zero benefit in one-shot CLI invocations beyond the
/// single recall flow.
const MEMORY_CACHE_CAP: usize = 256;

pub struct SqliteStore {
    conn: Connection,
    cache: Mutex<LruCache<String, Memory>>,
}

impl SqliteStore {
    pub fn new(path: &Path) -> IcmResult<Self> {
        Self::with_dims(path, icm_core::DEFAULT_EMBEDDING_DIMS)
    }

    /// Open or create a store with a specific embedding dimension.
    pub fn with_dims(path: &Path, embedding_dims: usize) -> IcmResult<Self> {
        ensure_sqlite_vec();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| IcmError::Database(format!("cannot create db directory: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| IcmError::Database(format!("cannot open database: {e}")))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )
        .map_err(db_err)?;
        init_db_with_dims(&conn, embedding_dims)?;
        Ok(Self {
            conn,
            cache: Mutex::new(new_cache()),
        })
    }

    /// Apply decay if more than 24 hours since last decay.
    /// Called automatically on recall to avoid manual `icm decay` cron.
    pub fn maybe_auto_decay(&self) -> IcmResult<()> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        // Atomic check-and-update: only one caller wins the race.
        // First, try to claim the decay slot by inserting or conditionally updating.
        let changed = self
            .conn
            .execute(
                "INSERT INTO icm_metadata (key, value) VALUES ('last_decay_at', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1
                 WHERE value IS NULL OR julianday(?1) - julianday(value) >= 1.0",
                params![now_str],
            )
            .map_err(db_err)?;

        if changed > 0 {
            self.apply_decay(0.95)?;
        }

        Ok(())
    }

    /// Atomically increment the hook call counter and return the new value.
    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        let count: usize = self
            .conn
            .query_row(
                "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '1')
                 ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)
                 RETURNING CAST(value AS INTEGER)",
                [],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(count)
    }

    /// Reset the hook call counter to 0.
    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        self.conn
            .execute(
                "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '0')
                 ON CONFLICT(key) DO UPDATE SET value = '0'",
                [],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn in_memory() -> IcmResult<Self> {
        ensure_sqlite_vec();
        let conn = Connection::open_in_memory()
            .map_err(|e| IcmError::Database(format!("cannot open in-memory db: {e}")))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")
            .map_err(db_err)?;
        init_db(&conn)?;
        Ok(Self {
            conn,
            cache: Mutex::new(new_cache()),
        })
    }

    fn cache_get(&self, id: &str) -> Option<Memory> {
        self.cache.lock().ok().and_then(|mut c| c.get(id).cloned())
    }

    fn cache_put(&self, m: &Memory) {
        if let Ok(mut c) = self.cache.lock() {
            c.put(m.id.clone(), m.clone());
        }
    }

    fn cache_invalidate(&self, id: &str) {
        if let Ok(mut c) = self.cache.lock() {
            c.pop(id);
        }
    }

    fn cache_invalidate_many(&self, ids: &[&str]) {
        if let Ok(mut c) = self.cache.lock() {
            for id in ids {
                c.pop(*id);
            }
        }
    }

    fn cache_clear(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }
}

fn new_cache() -> LruCache<String, Memory> {
    let cap = NonZeroUsize::new(MEMORY_CACHE_CAP)
        .expect("MEMORY_CACHE_CAP must be non-zero — see store.rs");
    LruCache::new(cap)
}

// ---------------------------------------------------------------------------
// Memory helpers
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

fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.as_bytes().to_vec()
}

fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    if !blob.len().is_multiple_of(4) {
        tracing::warn!(
            blob_size = blob.len(),
            "embedding blob size not divisible by 4, truncating"
        );
    }
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn row_to_memory(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    // Column order: id(0), created_at(1), updated_at(2), last_accessed(3),
    //   access_count(4), weight(5), topic(6), summary(7), raw_excerpt(8),
    //   keywords(9), importance(10), source_type(11), source_data(12),
    //   related_ids(13), embedding(14)
    let keywords_json: String = row.get::<_, Option<String>>(9)?.unwrap_or_default();
    let keywords: Vec<String> = serde_json::from_str(&keywords_json).unwrap_or_default();

    let importance_str: String = row.get(10)?;
    let importance = importance_str.parse().unwrap_or(Importance::Medium);

    let source_type_str: String = row.get(11)?;
    let source_data_str: Option<String> = row.get(12)?;
    let source = parse_source(&source_type_str, source_data_str);

    let related_json: String = row.get::<_, Option<String>>(13)?.unwrap_or_default();
    let related_ids: Vec<String> = serde_json::from_str(&related_json).unwrap_or_default();

    let embedding: Option<Vec<f32>> = row
        .get::<_, Option<Vec<u8>>>(14)?
        .map(|b| blob_to_embedding(&b));

    let created_at_str: String = row.get(1)?;
    let updated_at_str: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
    let last_accessed_str: String = row.get(3)?;

    let created_at = parse_dt(&created_at_str);

    Ok(Memory {
        id: row.get(0)?,
        created_at,
        updated_at: if updated_at_str.is_empty() {
            created_at
        } else {
            parse_dt(&updated_at_str)
        },
        last_accessed: parse_dt(&last_accessed_str),
        access_count: row.get::<_, u32>(4)?,
        weight: row.get(5)?,
        topic: row.get(6)?,
        summary: row.get(7)?,
        raw_excerpt: row.get(8)?,
        keywords,
        importance,
        source,
        related_ids,
        embedding,
        scope: icm_core::Scope::User, // default for existing local memories
    })
}

const SELECT_COLS: &str = "id, created_at, updated_at, last_accessed, access_count, weight, \
                           topic, summary, raw_excerpt, keywords, \
                           importance, source_type, source_data, related_ids, embedding";

/// Sanitize a query string for FTS5 MATCH.
///
/// FTS5 treats characters like `-`, `*`, `"`, `:`, `^`, `+`, `~` as operators.
/// A query like `"sqlite-vec"` makes FTS5 interpret `-` as NOT and `vec` as a
/// column name, causing "no such column: vec".
///
/// This function strips special chars and wraps each token in double quotes.
fn sanitize_fts_query(query: &str) -> String {
    // Limit input length to prevent abuse (UTF-8 safe truncation)
    let query = if query.len() > 10_000 {
        let mut end = 10_000;
        while end > 0 && !query.is_char_boundary(end) {
            end -= 1;
        }
        &query[..end]
    } else {
        query
    };

    // Replace FTS5 operator chars with spaces, then quote each resulting token.
    // FTS5 tokenizer (unicode61) splits on `-` too, so we must keep tokens separate.
    let cleaned: String = query
        .chars()
        .map(|c| {
            if matches!(
                c,
                '-' | '*' | '"' | '(' | ')' | '{' | '}' | ':' | '^' | '+' | '~' | '\\'
            ) {
                ' '
            } else {
                c
            }
        })
        .collect();

    let tokens: Vec<String> = cleaned
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .take(100) // Limit token count to prevent excessive query complexity
        .map(|w| {
            // Strip any remaining quotes from tokens before wrapping in quotes
            let stripped = w.replace('"', "");
            format!("\"{stripped}\"")
        })
        .collect();
    tokens.join(" ")
}

// ---------------------------------------------------------------------------
// MemoryStore impl
// ---------------------------------------------------------------------------

impl SqliteStore {
    /// Insert a memory into the database without transaction management.
    /// Callers are responsible for wrapping this in a transaction.
    fn store_inner(&self, memory: &Memory) -> IcmResult<String> {
        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);
        let emb_blob = memory.embedding.as_deref().map(embedding_to_blob);

        self.conn
            .execute(
                "INSERT INTO memories (id, created_at, updated_at, last_accessed, access_count, weight,
                 topic, summary, raw_excerpt, keywords,
                 importance, source_type, source_data, related_ids, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    memory.id,
                    memory.created_at.to_rfc3339(),
                    memory.updated_at.to_rfc3339(),
                    memory.last_accessed.to_rfc3339(),
                    memory.access_count,
                    memory.weight,
                    memory.topic,
                    memory.summary,
                    memory.raw_excerpt,
                    keywords_json,
                    memory.importance.to_string(),
                    st,
                    sd,
                    related_json,
                    emb_blob,
                ],
            )
            .map_err(db_err)?;

        // Sync to vec_memories for KNN search
        if let Some(ref blob) = emb_blob {
            self.conn
                .execute(
                    "INSERT INTO vec_memories (memory_id, embedding) VALUES (?1, ?2)",
                    params![memory.id, blob],
                )
                .map_err(db_err)?;
        }

        Ok(memory.id.clone())
    }
}

impl MemoryStore for SqliteStore {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        match self.store_inner(&memory) {
            Ok(id) => {
                self.conn.execute_batch("COMMIT;").map_err(db_err)?;
                Ok(id)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
        if let Some(m) = self.cache_get(id) {
            return Ok(Some(m));
        }

        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {SELECT_COLS} FROM memories WHERE id = ?1"))
            .map_err(db_err)?;

        let result = stmt
            .query_row(params![id], row_to_memory)
            .optional()
            .map_err(db_err)?;

        if let Some(ref m) = result {
            self.cache_put(m);
        }
        Ok(result)
    }

    fn update(&self, memory: &Memory) -> IcmResult<()> {
        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);
        let emb_blob = memory.embedding.as_deref().map(embedding_to_blob);

        let changed = self
            .conn
            .execute(
                "UPDATE memories SET
                 updated_at = ?2, last_accessed = ?3, access_count = ?4, weight = ?5,
                 topic = ?6, summary = ?7, raw_excerpt = ?8, keywords = ?9,
                 importance = ?10, source_type = ?11, source_data = ?12, related_ids = ?13,
                 embedding = ?14
                 WHERE id = ?1",
                params![
                    memory.id,
                    memory.updated_at.to_rfc3339(),
                    memory.last_accessed.to_rfc3339(),
                    memory.access_count,
                    memory.weight,
                    memory.topic,
                    memory.summary,
                    memory.raw_excerpt,
                    keywords_json,
                    memory.importance.to_string(),
                    st,
                    sd,
                    related_json,
                    emb_blob,
                ],
            )
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(memory.id.clone()));
        }

        // Sync vec_memories: always delete old, re-insert if embedding exists
        let _ = self.conn.execute(
            "DELETE FROM vec_memories WHERE memory_id = ?1",
            params![memory.id],
        );
        if let Some(ref blob) = emb_blob {
            self.conn
                .execute(
                    "INSERT INTO vec_memories (memory_id, embedding) VALUES (?1, ?2)",
                    params![memory.id, blob],
                )
                .map_err(db_err)?;
        }

        self.cache_invalidate(&memory.id);
        Ok(())
    }

    fn delete(&self, id: &str) -> IcmResult<()> {
        let _ = self
            .conn
            .execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id]);

        let changed = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        self.cache_invalidate(id);
        Ok(())
    }

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }

        // Cap keywords to avoid massive SQL generation
        let keywords = &keywords[..keywords.len().min(50)];
        let limit = limit.min(100);

        let where_parts: Vec<String> = (0..keywords.len())
            .map(|i| {
                let p = i + 1;
                format!("(keywords LIKE ?{p} OR summary LIKE ?{p} OR topic LIKE ?{p})")
            })
            .collect();
        let where_clause = where_parts.join(" OR ");

        let query = format!(
            "SELECT {SELECT_COLS} FROM memories WHERE {where_clause} ORDER BY weight DESC LIMIT ?{}",
            keywords.len() + 1
        );

        let mut stmt = self.conn.prepare(&query).map_err(db_err)?;

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = keywords
            .iter()
            .map(|k| Box::new(format!("%{k}%")) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        param_values.push(Box::new(limit as i64));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_ref.as_slice(), row_to_memory)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>> {
        let limit = limit.min(100);
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT {SELECT_COLS} FROM memories
             WHERE id IN (
                 SELECT id FROM memories_fts WHERE memories_fts MATCH ?1
             )
             ORDER BY weight DESC
             LIMIT ?2"
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params![sanitized, limit as i64], row_to_memory)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn search_by_embedding(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let query_blob = embedding_to_blob(embedding);

        // KNN query on vec0 virtual table (requires LIMIT in the query itself)
        let mut knn_stmt = self
            .conn
            .prepare(
                "SELECT memory_id, distance
                 FROM vec_memories
                 WHERE embedding MATCH ?1
                 ORDER BY distance
                 LIMIT ?2",
            )
            .map_err(db_err)?;

        let knn_rows: Vec<(String, f32)> = knn_stmt
            .query_map(params![query_blob, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
            })
            .map_err(db_err)?
            .filter_map(|r| r.ok())
            .collect();

        if knn_rows.is_empty() {
            return Ok(Vec::new());
        }

        // Batch fetch all memories in one query
        let placeholders: Vec<String> = (1..=knn_rows.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT {SELECT_COLS} FROM memories WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let ids: Vec<&str> = knn_rows.iter().map(|(id, _)| id.as_str()).collect();
        let params: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(&*params, row_to_memory).map_err(db_err)?;

        let mut memory_map: std::collections::HashMap<String, Memory> = HashMap::new();
        for row in rows.flatten() {
            memory_map.insert(row.id.clone(), row);
        }

        // Reassemble in KNN order with similarity scores
        let results: Vec<(Memory, f32)> = knn_rows
            .into_iter()
            .filter_map(|(id, distance)| memory_map.remove(&id).map(|mem| (mem, 1.0 - distance)))
            .collect();

        Ok(results)
    }

    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let limit = limit.min(1000);
        let pool_size = limit * 4;
        let sanitized = sanitize_fts_query(query);

        // 1. Get FTS results with rank scores
        let fts_sql =
            "SELECT m.id, m.created_at, m.updated_at, m.last_accessed, m.access_count, m.weight, \
                    m.topic, m.summary, m.raw_excerpt, m.keywords, \
                    m.importance, m.source_type, m.source_data, m.related_ids, m.embedding, \
                    fts.rank \
             FROM memories_fts fts \
             JOIN memories m ON m.id = fts.id \
             WHERE memories_fts MATCH ?1 \
             ORDER BY fts.rank \
             LIMIT ?2";

        let mut fts_scores: HashMap<String, f32> = HashMap::with_capacity(pool_size);
        let mut all_memories: HashMap<String, Memory> = HashMap::with_capacity(pool_size);

        if !sanitized.is_empty() {
            if let Ok(mut stmt) = self.conn.prepare(fts_sql) {
                if let Ok(rows) = stmt.query_map(params![sanitized, pool_size as i64], |row| {
                    let memory = row_to_memory(row)?;
                    let rank: f32 = row.get(15)?;
                    Ok((memory, rank))
                }) {
                    for row in rows.flatten() {
                        let (memory, rank) = row;
                        // Normalize FTS rank (lower is better, typically negative)
                        // Convert to 0..1 score where higher is better
                        let score = 1.0 / (1.0 + rank.abs());
                        fts_scores.insert(memory.id.clone(), score);
                        all_memories.insert(memory.id.clone(), memory);
                    }
                }
            }
        } // sanitized.is_empty()

        // 2. Get vector results
        let vec_results = self.search_by_embedding(embedding, pool_size)?;
        let mut vec_scores: HashMap<String, f32> = HashMap::with_capacity(pool_size);
        for (memory, similarity) in vec_results {
            vec_scores.insert(memory.id.clone(), similarity);
            all_memories.entry(memory.id.clone()).or_insert(memory);
        }

        // 3. Combine scores: 30% FTS + 70% vector
        let keys: Vec<String> = all_memories.keys().cloned().collect();
        let mut scored: Vec<(String, f32)> = Vec::with_capacity(keys.len());
        for id in keys {
            let fts_score = fts_scores.get(&id).copied().unwrap_or(0.0);
            let vec_score = vec_scores.get(&id).copied().unwrap_or(0.0);
            let combined = 0.3 * fts_score + 0.7 * vec_score;
            scored.push((id, combined));
        }

        // Sort by combined score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let results: Vec<(Memory, f32)> = scored
            .into_iter()
            .filter_map(|(id, score)| all_memories.remove(&id).map(|mem| (mem, score)))
            .collect();

        Ok(results)
    }

    fn update_access(&self, id: &str) -> IcmResult<()> {
        let now = Utc::now().to_rfc3339();
        let changed = self
            .conn
            .execute(
                "UPDATE memories SET last_accessed = ?1, access_count = access_count + 1 WHERE id = ?2",
                params![now, id],
            )
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        self.cache_invalidate(id);
        Ok(())
    }

    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let now = Utc::now().to_rfc3339();
        let placeholders: Vec<String> = (2..=ids.len() + 1).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE memories SET last_accessed = ?1, access_count = access_count + 1 WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(ids.len() + 1);
        params_vec.push(Box::new(now));
        for id in ids {
            params_vec.push(Box::new(id.to_string()));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let changed = self.conn.execute(&sql, refs.as_slice()).map_err(db_err)?;
        self.cache_invalidate_many(ids);
        Ok(changed)
    }

    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize> {
        // Access-aware decay: frequently accessed memories decay slower.
        // decay = base_rate * importance_multiplier / (1 + access_count * 0.1)
        // critical: never decays
        // high: 0.5x decay (half speed)
        // medium: 1.0x decay (normal)
        // low: 2.0x decay (double speed)
        let changed = self
            .conn
            .execute(
                "UPDATE memories SET weight = weight * (
                    1.0 - (1.0 - ?1) *
                    CASE importance
                        WHEN 'high' THEN 0.5
                        WHEN 'low' THEN 2.0
                        ELSE 1.0
                    END
                    / (1.0 + access_count * 0.1)
                )
                WHERE importance != 'critical'",
                params![decay_factor],
            )
            .map_err(db_err)?;

        // Decay touches every non-critical row's weight; can't selectively
        // invalidate without re-reading rows, so just nuke the cache.
        self.cache_clear();
        Ok(changed)
    }

    fn prune(&self, weight_threshold: f32) -> IcmResult<usize> {
        // Never prune critical or high importance memories
        let _ = self.conn.execute(
            "DELETE FROM vec_memories WHERE memory_id IN (
                SELECT id FROM memories WHERE weight < ?1 AND importance NOT IN ('critical', 'high')
            )",
            params![weight_threshold],
        );

        let changed = self
            .conn
            .execute(
                "DELETE FROM memories WHERE weight < ?1 AND importance NOT IN ('critical', 'high')",
                params![weight_threshold],
            )
            .map_err(db_err)?;

        if changed > 0 {
            self.cache_clear();
        }
        Ok(changed)
    }

    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM memories WHERE topic = ?1 ORDER BY weight DESC LIMIT 500"
            ))
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![topic], row_to_memory)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn list_all(&self) -> IcmResult<Vec<Memory>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM memories ORDER BY weight DESC LIMIT 10000"
            ))
            .map_err(db_err)?;

        let rows = stmt.query_map([], row_to_memory).map_err(db_err)?;
        collect_rows(rows)
    }

    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT topic, COUNT(*) FROM memories GROUP BY topic ORDER BY topic")
            .map_err(db_err)?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        // Clean vec_memories for entries about to be deleted
        if let Err(e) = self.conn.execute(
            "DELETE FROM vec_memories WHERE memory_id IN (
                SELECT id FROM memories WHERE topic = ?1
            )",
            params![topic],
        ) {
            tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after vec_memories delete failed");
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(IcmError::Database(e.to_string()));
        }

        if let Err(e) = self
            .conn
            .execute("DELETE FROM memories WHERE topic = ?1", params![topic])
        {
            tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after memories delete failed");
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(IcmError::Database(e.to_string()));
        }

        if let Err(e) = self.store_inner(&consolidated) {
            tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after store failed");
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(e);
        }

        // Rebuild FTS index to eliminate any ghost entries from the external
        // content table.  This guarantees search results stay consistent after
        // bulk deletes (fixes #44).
        if let Err(e) = self
            .conn
            .execute_batch("INSERT INTO memories_fts(memories_fts) VALUES('rebuild');")
        {
            tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after FTS rebuild failed");
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(IcmError::Database(e.to_string()));
        }

        self.conn.execute_batch("COMMIT;").map_err(db_err)?;
        // Bulk delete + re-insert touches arbitrarily many cached entries.
        self.cache_clear();
        Ok(())
    }

    fn count(&self) -> IcmResult<usize> {
        self.conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn count_by_topic(&self, topic: &str) -> IcmResult<usize> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE topic = ?1",
                params![topic],
                |row| row.get::<_, usize>(0),
            )
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth> {
        let row = self
            .conn
            .query_row(
                "SELECT
                    COUNT(*),
                    AVG(weight),
                    AVG(CAST(access_count AS REAL)),
                    MIN(created_at),
                    MAX(created_at),
                    MAX(last_accessed),
                    SUM(CASE WHEN weight < 0.5
                         AND julianday('now') - julianday(last_accessed) > 14
                         THEN 1 ELSE 0 END)
                 FROM memories WHERE topic = ?1",
                params![topic],
                |row| {
                    Ok((
                        row.get::<_, usize>(0)?,
                        row.get::<_, f32>(1)?,
                        row.get::<_, f32>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, usize>(6)?,
                    ))
                },
            )
            .map_err(db_err)?;

        let (
            entry_count,
            avg_weight,
            avg_access,
            oldest_str,
            newest_str,
            last_accessed_str,
            stale_count,
        ) = row;

        if entry_count == 0 {
            return Err(IcmError::NotFound(format!("topic: {topic}")));
        }

        let parse_dt = |s: &str| -> Option<DateTime<Utc>> {
            match DateTime::parse_from_rfc3339(s) {
                Ok(d) => Some(d.with_timezone(&Utc)),
                Err(e) => {
                    tracing::warn!("invalid timestamp '{}': {}", s, e);
                    None
                }
            }
        };

        Ok(TopicHealth {
            topic: topic.to_string(),
            entry_count,
            avg_weight,
            avg_access_count: avg_access,
            oldest: oldest_str.as_deref().and_then(parse_dt),
            newest: newest_str.as_deref().and_then(parse_dt),
            last_accessed: last_accessed_str.as_deref().and_then(parse_dt),
            needs_consolidation: entry_count > 5,
            stale_count,
        })
    }

    fn stats(&self) -> IcmResult<StoreStats> {
        let (total_memories, total_topics, avg_weight, oldest_str, newest_str): (
            usize,
            usize,
            f32,
            Option<String>,
            Option<String>,
        ) = self
            .conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT topic), COALESCE(AVG(weight), 0.0), \
                 MIN(created_at), MAX(created_at) FROM memories",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(db_err)?;

        let oldest_memory = oldest_str
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc));
        let newest_memory = newest_str
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc));

        Ok(StoreStats {
            total_memories,
            total_topics,
            avg_weight,
            oldest_memory,
            newest_memory,
        })
    }
}

// ---------------------------------------------------------------------------
// Memoir / Concept helpers
// ---------------------------------------------------------------------------

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_memoir(row: &rusqlite::Row) -> rusqlite::Result<Memoir> {
    Ok(Memoir {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        created_at: parse_dt(&row.get::<_, String>(3)?),
        updated_at: parse_dt(&row.get::<_, String>(4)?),
        consolidation_threshold: row.get::<_, u32>(5)?,
    })
}

const MEMOIR_COLS: &str = "id, name, description, created_at, updated_at, consolidation_threshold";

fn row_to_concept(row: &rusqlite::Row) -> rusqlite::Result<Concept> {
    let labels_json: String = row.get(4)?;
    let labels: Vec<Label> = serde_json::from_str(&labels_json).unwrap_or_default();

    let source_ids_json: String = row.get(9)?;
    let source_memory_ids: Vec<String> = serde_json::from_str(&source_ids_json).unwrap_or_default();

    Ok(Concept {
        id: row.get(0)?,
        memoir_id: row.get(1)?,
        name: row.get(2)?,
        definition: row.get(3)?,
        labels,
        confidence: row.get(5)?,
        revision: row.get::<_, u32>(6)?,
        created_at: parse_dt(&row.get::<_, String>(7)?),
        updated_at: parse_dt(&row.get::<_, String>(8)?),
        source_memory_ids,
    })
}

const CONCEPT_COLS: &str = "id, memoir_id, name, definition, labels, confidence, \
                            revision, created_at, updated_at, source_memory_ids";

fn row_to_link(row: &rusqlite::Row) -> rusqlite::Result<ConceptLink> {
    let relation_str: String = row.get(3)?;
    let relation: Relation = relation_str.parse().unwrap_or(Relation::RelatedTo);

    Ok(ConceptLink {
        id: row.get(0)?,
        source_id: row.get(1)?,
        target_id: row.get(2)?,
        relation,
        weight: row.get(4)?,
        created_at: parse_dt(&row.get::<_, String>(5)?),
    })
}

const LINK_COLS: &str = "id, source_id, target_id, relation, weight, created_at";

// ---------------------------------------------------------------------------
// MemoirStore impl
// ---------------------------------------------------------------------------

use rusqlite::OptionalExtension;

impl MemoirStore for SqliteStore {
    // --- Memoir CRUD ---

    fn create_memoir(&self, memoir: Memoir) -> IcmResult<String> {
        self.conn
            .execute(
                "INSERT INTO memoirs (id, name, description, created_at, updated_at, consolidation_threshold)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    memoir.id,
                    memoir.name,
                    memoir.description,
                    memoir.created_at.to_rfc3339(),
                    memoir.updated_at.to_rfc3339(),
                    memoir.consolidation_threshold,
                ],
            )
            .map_err(db_err)?;
        Ok(memoir.id)
    }

    fn get_memoir(&self, id: &str) -> IcmResult<Option<Memoir>> {
        self.conn
            .prepare(&format!("SELECT {MEMOIR_COLS} FROM memoirs WHERE id = ?1"))
            .map_err(db_err)?
            .query_row(params![id], row_to_memoir)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn get_memoir_by_name(&self, name: &str) -> IcmResult<Option<Memoir>> {
        self.conn
            .prepare(&format!(
                "SELECT {MEMOIR_COLS} FROM memoirs WHERE name = ?1"
            ))
            .map_err(db_err)?
            .query_row(params![name], row_to_memoir)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn update_memoir(&self, memoir: &Memoir) -> IcmResult<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE memoirs SET name = ?2, description = ?3, updated_at = ?4,
                 consolidation_threshold = ?5 WHERE id = ?1",
                params![
                    memoir.id,
                    memoir.name,
                    memoir.description,
                    memoir.updated_at.to_rfc3339(),
                    memoir.consolidation_threshold,
                ],
            )
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(memoir.id.clone()));
        }
        Ok(())
    }

    fn delete_memoir(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM memoirs WHERE id = ?1", params![id])
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn list_memoirs(&self) -> IcmResult<Vec<Memoir>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {MEMOIR_COLS} FROM memoirs ORDER BY name LIMIT 500"
            ))
            .map_err(db_err)?;

        let rows = stmt.query_map([], row_to_memoir).map_err(db_err)?;

        collect_rows(rows)
    }

    // --- Concept CRUD ---

    fn add_concept(&self, concept: Concept) -> IcmResult<String> {
        let labels_json = serde_json::to_string(&concept.labels)?;
        let source_ids_json = serde_json::to_string(&concept.source_memory_ids)?;

        self.conn
            .execute(
                "INSERT INTO concepts (id, memoir_id, name, definition, labels, confidence,
                 revision, created_at, updated_at, source_memory_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    concept.id,
                    concept.memoir_id,
                    concept.name,
                    concept.definition,
                    labels_json,
                    concept.confidence,
                    concept.revision,
                    concept.created_at.to_rfc3339(),
                    concept.updated_at.to_rfc3339(),
                    source_ids_json,
                ],
            )
            .map_err(db_err)?;
        Ok(concept.id)
    }

    fn get_concept(&self, id: &str) -> IcmResult<Option<Concept>> {
        self.conn
            .prepare(&format!(
                "SELECT {CONCEPT_COLS} FROM concepts WHERE id = ?1"
            ))
            .map_err(db_err)?
            .query_row(params![id], row_to_concept)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn get_concept_by_name(&self, memoir_id: &str, name: &str) -> IcmResult<Option<Concept>> {
        self.conn
            .prepare(&format!(
                "SELECT {CONCEPT_COLS} FROM concepts WHERE memoir_id = ?1 AND name = ?2"
            ))
            .map_err(db_err)?
            .query_row(params![memoir_id, name], row_to_concept)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn update_concept(&self, concept: &Concept) -> IcmResult<()> {
        let labels_json = serde_json::to_string(&concept.labels)?;
        let source_ids_json = serde_json::to_string(&concept.source_memory_ids)?;

        let changed = self
            .conn
            .execute(
                "UPDATE concepts SET memoir_id = ?2, name = ?3, definition = ?4, labels = ?5,
                 confidence = ?6, revision = ?7, updated_at = ?8, source_memory_ids = ?9
                 WHERE id = ?1",
                params![
                    concept.id,
                    concept.memoir_id,
                    concept.name,
                    concept.definition,
                    labels_json,
                    concept.confidence,
                    concept.revision,
                    concept.updated_at.to_rfc3339(),
                    source_ids_json,
                ],
            )
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(concept.id.clone()));
        }
        Ok(())
    }

    fn delete_concept(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM concepts WHERE id = ?1", params![id])
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    // --- Concept Search ---

    fn list_concepts(&self, memoir_id: &str) -> IcmResult<Vec<Concept>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {CONCEPT_COLS} FROM concepts WHERE memoir_id = ?1 ORDER BY name LIMIT 1000"
            ))
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![memoir_id], row_to_concept)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn search_concepts_fts(
        &self,
        memoir_id: &str,
        query: &str,
        limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT {CONCEPT_COLS} FROM concepts
             WHERE memoir_id = ?1
               AND id IN (SELECT id FROM concepts_fts WHERE concepts_fts MATCH ?2)
             ORDER BY confidence DESC
             LIMIT ?3"
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params![memoir_id, sanitized, limit as i64], row_to_concept)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn search_all_concepts_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Concept>> {
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT {CONCEPT_COLS} FROM concepts
             WHERE id IN (SELECT id FROM concepts_fts WHERE concepts_fts MATCH ?1)
             ORDER BY confidence DESC
             LIMIT ?2"
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params![sanitized, limit as i64], row_to_concept)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn search_concepts_by_label(
        &self,
        memoir_id: &str,
        label: &Label,
        limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        // Search JSON labels column using LIKE with the serialized label pattern
        let pattern = format!(
            "%\"namespace\":\"{}\"%\"value\":\"{}\"%",
            label.namespace, label.value
        );

        let sql = format!(
            "SELECT {CONCEPT_COLS} FROM concepts
             WHERE memoir_id = ?1 AND labels LIKE ?2
             ORDER BY confidence DESC
             LIMIT ?3"
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params![memoir_id, pattern, limit as i64], row_to_concept)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    // --- Refinement ---

    fn refine_concept(
        &self,
        id: &str,
        new_definition: &str,
        new_source_ids: &[String],
    ) -> IcmResult<()> {
        // Get existing concept to merge source IDs
        let concept = self
            .get_concept(id)?
            .ok_or_else(|| IcmError::NotFound(id.to_string()))?;

        let mut merged_sources = concept.source_memory_ids;
        for sid in new_source_ids {
            if !merged_sources.contains(sid) {
                merged_sources.push(sid.clone());
            }
        }
        let source_ids_json = serde_json::to_string(&merged_sources)?;

        let now = Utc::now().to_rfc3339();
        let new_confidence = (concept.confidence + 0.1).min(1.0);

        self.conn
            .execute(
                "UPDATE concepts SET definition = ?2, revision = revision + 1,
                 confidence = ?3, updated_at = ?4, source_memory_ids = ?5
                 WHERE id = ?1",
                params![id, new_definition, new_confidence, now, source_ids_json],
            )
            .map_err(db_err)?;

        Ok(())
    }

    // --- Graph ---

    fn add_link(&self, link: ConceptLink) -> IcmResult<String> {
        // Reject self-links: A→A is meaningless and produces a 1-step
        // cycle. Caller usually catches this earlier but the store is
        // the authoritative invariant gate.
        if link.source_id == link.target_id {
            return Err(IcmError::InvalidInput(format!(
                "self-link rejected: source and target are the same concept ({})",
                link.source_id
            )));
        }
        // Cycle detection: BFS from `target` following outgoing edges.
        // If we can reach `source`, the new edge would close a cycle
        // (source → target → ... → source). Reject before insert.
        //
        // The BFS is bounded by the number of links currently in the
        // memoir, so worst-case it touches every link once. For typical
        // memoirs (<10k links) this is sub-millisecond.
        if self.would_create_cycle(&link.source_id, &link.target_id)? {
            return Err(IcmError::InvalidInput(format!(
                "concept link rejected: {} → {} would create a cycle in the graph",
                link.source_id, link.target_id
            )));
        }
        self.conn
            .execute(
                "INSERT INTO concept_links (id, source_id, target_id, relation, weight, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    link.id,
                    link.source_id,
                    link.target_id,
                    link.relation.to_string(),
                    link.weight,
                    link.created_at.to_rfc3339(),
                ],
            )
            .map_err(db_err)?;
        Ok(link.id)
    }

    fn get_links_from(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {LINK_COLS} FROM concept_links WHERE source_id = ?1"
            ))
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![concept_id], row_to_link)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn get_links_to(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {LINK_COLS} FROM concept_links WHERE target_id = ?1"
            ))
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![concept_id], row_to_link)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn delete_link(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM concept_links WHERE id = ?1", params![id])
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn get_neighbors(
        &self,
        concept_id: &str,
        relation: Option<Relation>,
    ) -> IcmResult<Vec<Concept>> {
        let (sql, p_relation);

        let base = format!(
            "SELECT {CONCEPT_COLS} FROM concepts WHERE id IN (
                SELECT target_id FROM concept_links WHERE source_id = ?1 {{filter}}
                UNION
                SELECT source_id FROM concept_links WHERE target_id = ?1 {{filter}}
            )"
        );

        if let Some(ref r) = relation {
            p_relation = r.to_string();
            let filtered = base.replace("{filter}", "AND relation = ?2");
            sql = filtered;
        } else {
            p_relation = String::new();
            sql = base.replace("{filter}", "");
        };

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let rows = if relation.is_some() {
            stmt.query_map(params![concept_id, p_relation], row_to_concept)
                .map_err(db_err)?
        } else {
            stmt.query_map(params![concept_id], row_to_concept)
                .map_err(db_err)?
        };

        collect_rows(rows)
    }

    fn get_neighborhood(
        &self,
        concept_id: &str,
        depth: usize,
    ) -> IcmResult<(Vec<Concept>, Vec<ConceptLink>)> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let mut concepts = Vec::new();
        let mut links = Vec::new();

        // Seed with the root concept
        if let Some(root) = self.get_concept(concept_id)? {
            visited.insert(root.id.clone());
            queue.push_back((root.id.clone(), 0));
            concepts.push(root);
        } else {
            return Err(IcmError::NotFound(concept_id.to_string()));
        }

        while let Some((current_id, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }

            // Outgoing links
            let outgoing = self.get_links_from(&current_id)?;
            for link in outgoing {
                if !visited.contains(&link.target_id) {
                    if let Some(c) = self.get_concept(&link.target_id)? {
                        visited.insert(c.id.clone());
                        queue.push_back((c.id.clone(), current_depth + 1));
                        concepts.push(c);
                    }
                }
                links.push(link);
            }

            // Incoming links
            let incoming = self.get_links_to(&current_id)?;
            for link in incoming {
                if !visited.contains(&link.source_id) {
                    if let Some(c) = self.get_concept(&link.source_id)? {
                        visited.insert(c.id.clone());
                        queue.push_back((c.id.clone(), current_depth + 1));
                        concepts.push(c);
                    }
                }
                links.push(link);
            }
        }

        Ok((concepts, links))
    }

    // --- Stats ---

    fn memoir_stats(&self, memoir_id: &str) -> IcmResult<MemoirStats> {
        let total_concepts: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concepts WHERE memoir_id = ?1",
                params![memoir_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let total_links: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concept_links
                 WHERE source_id IN (SELECT id FROM concepts WHERE memoir_id = ?1)",
                params![memoir_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let avg_confidence: f32 = if total_concepts > 0 {
            self.conn
                .query_row(
                    "SELECT AVG(confidence) FROM concepts WHERE memoir_id = ?1",
                    params![memoir_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?
        } else {
            0.0
        };

        // Count labels via SQL — avoids loading all concepts into memory
        let mut label_stmt = self
            .conn
            .prepare("SELECT labels FROM concepts WHERE memoir_id = ?1 AND labels != '[]'")
            .map_err(db_err)?;
        let label_rows = label_stmt
            .query_map(params![memoir_id], |row| row.get::<_, String>(0))
            .map_err(db_err)?;
        let mut label_map: HashMap<String, usize> = HashMap::new();
        for row in label_rows {
            let raw = row.map_err(db_err)?;
            if let Ok(labels) = serde_json::from_str::<Vec<Label>>(&raw) {
                for l in labels {
                    *label_map.entry(l.to_string()).or_insert(0) += 1;
                }
            }
        }
        let mut label_counts: Vec<(String, usize)> = label_map.into_iter().collect();
        label_counts.sort_by_key(|b| std::cmp::Reverse(b.1));

        Ok(MemoirStats {
            total_concepts,
            total_links,
            avg_confidence,
            label_counts,
        })
    }

    fn get_links_for_memoir(&self, memoir_id: &str) -> IcmResult<Vec<ConceptLink>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {LINK_COLS} FROM concept_links
                 WHERE source_id IN (SELECT id FROM concepts WHERE memoir_id = ?1)
                 LIMIT 5000"
            ))
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![memoir_id], row_to_link)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    fn batch_memoir_concept_counts(&self) -> IcmResult<HashMap<String, usize>> {
        let mut stmt = self
            .conn
            .prepare("SELECT memoir_id, COUNT(*) FROM concepts GROUP BY memoir_id")
            .map_err(db_err)?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })
            .map_err(db_err)?;

        let mut map = HashMap::new();
        for row in rows {
            let (id, count) = row.map_err(db_err)?;
            map.insert(id, count);
        }
        Ok(map)
    }
}

// ---------------------------------------------------------------------------
// Feedback helpers
// ---------------------------------------------------------------------------

fn row_to_feedback(row: &rusqlite::Row) -> rusqlite::Result<Feedback> {
    Ok(Feedback {
        id: row.get(0)?,
        topic: row.get(1)?,
        context: row.get(2)?,
        predicted: row.get(3)?,
        corrected: row.get(4)?,
        reason: row.get(5)?,
        source: row.get(6)?,
        created_at: parse_dt(&row.get::<_, String>(7)?),
        applied_count: row.get(8)?,
    })
}

const FEEDBACK_COLS: &str =
    "id, topic, context, predicted, corrected, reason, source, created_at, applied_count";

// ---------------------------------------------------------------------------
// FeedbackStore impl
// ---------------------------------------------------------------------------

impl FeedbackStore for SqliteStore {
    fn store_feedback(&self, feedback: Feedback) -> IcmResult<String> {
        self.conn
            .execute(
                "INSERT INTO feedback (id, topic, context, predicted, corrected, reason, source, created_at, applied_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    feedback.id,
                    feedback.topic,
                    feedback.context,
                    feedback.predicted,
                    feedback.corrected,
                    feedback.reason,
                    feedback.source,
                    feedback.created_at.to_rfc3339(),
                    feedback.applied_count,
                ],
            )
            .map_err(db_err)?;
        Ok(feedback.id)
    }

    fn search_feedback(
        &self,
        query: &str,
        topic: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<Feedback>> {
        let sanitized = sanitize_fts_query(query);

        if sanitized.is_empty() {
            return self.list_feedback(topic, limit);
        }

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(t) = topic {
                (
                    format!(
                        "SELECT {FEEDBACK_COLS} FROM feedback
                     WHERE id IN (SELECT id FROM feedback_fts WHERE feedback_fts MATCH ?1)
                     AND topic = ?2
                     ORDER BY created_at DESC LIMIT ?3"
                    ),
                    vec![
                        Box::new(sanitized) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(t.to_string()),
                        Box::new(limit as i64),
                    ],
                )
            } else {
                (
                    format!(
                        "SELECT {FEEDBACK_COLS} FROM feedback
                     WHERE id IN (SELECT id FROM feedback_fts WHERE feedback_fts MATCH ?1)
                     ORDER BY created_at DESC LIMIT ?2"
                    ),
                    vec![
                        Box::new(sanitized) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(limit as i64),
                    ],
                )
            };

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), row_to_feedback)
            .map_err(db_err)?;
        collect_rows(rows)
    }

    fn list_feedback(&self, topic: Option<&str>, limit: usize) -> IcmResult<Vec<Feedback>> {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(t) =
            topic
        {
            (
                    format!(
                        "SELECT {FEEDBACK_COLS} FROM feedback WHERE topic = ?1 ORDER BY created_at DESC LIMIT ?2"
                    ),
                    vec![
                        Box::new(t.to_string()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(limit as i64),
                    ],
                )
        } else {
            (
                format!("SELECT {FEEDBACK_COLS} FROM feedback ORDER BY created_at DESC LIMIT ?1"),
                vec![Box::new(limit as i64) as Box<dyn rusqlite::types::ToSql>],
            )
        };

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), row_to_feedback)
            .map_err(db_err)?;
        collect_rows(rows)
    }

    fn increment_applied(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE feedback SET applied_count = applied_count + 1 WHERE id = ?1",
                params![id],
            )
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn delete_feedback(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM feedback WHERE id = ?1", params![id])
            .map_err(db_err)?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn feedback_stats(&self) -> IcmResult<FeedbackStats> {
        let total: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM feedback", [], |row| row.get(0))
            .map_err(db_err)?;

        let mut stmt = self
            .conn
            .prepare("SELECT topic, COUNT(*) as cnt FROM feedback GROUP BY topic ORDER BY cnt DESC")
            .map_err(db_err)?;

        let by_topic: Vec<(String, usize)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_err)?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, applied_count FROM feedback WHERE applied_count > 0 ORDER BY applied_count DESC LIMIT 10",
            )
            .map_err(db_err)?;

        let most_applied: Vec<(String, u32)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_err)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(FeedbackStats {
            total,
            by_topic,
            most_applied,
        })
    }
}

// ---------------------------------------------------------------------------
// Transcripts (verbatim sessions + messages)
// ---------------------------------------------------------------------------

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let started_at: String = row.get("started_at")?;
    let updated_at: String = row.get("updated_at")?;
    Ok(Session {
        id: row.get("id")?,
        agent: row.get("agent")?,
        project: row.get("project")?,
        started_at: parse_ts(&started_at),
        updated_at: parse_ts(&updated_at),
        metadata: row.get("metadata")?,
    })
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let role_str: String = row.get("role")?;
    let ts: String = row.get("ts")?;
    let role = Role::parse(&role_str).unwrap_or(Role::Tool);
    Ok(Message {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        role,
        content: row.get("content")?,
        tool_name: row.get("tool_name")?,
        tokens: row.get("tokens")?,
        ts: parse_ts(&ts),
        metadata: row.get("metadata")?,
    })
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

impl TranscriptStore for SqliteStore {
    fn create_session(
        &self,
        agent: &str,
        project: Option<&str>,
        metadata: Option<&str>,
    ) -> IcmResult<String> {
        let session = Session::new(
            agent.to_string(),
            project.map(|s| s.to_string()),
            metadata.map(|s| s.to_string()),
        );
        let conn = &self.conn;
        conn.execute(
            "INSERT INTO sessions (id, agent, project, started_at, updated_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session.id,
                session.agent,
                session.project,
                session.started_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                session.metadata,
            ],
        )
        .map_err(db_err)?;
        Ok(session.id)
    }

    fn get_session(&self, id: &str) -> IcmResult<Option<Session>> {
        let conn = &self.conn;
        let row = conn
            .query_row(
                "SELECT id, agent, project, started_at, updated_at, metadata
                 FROM sessions WHERE id = ?1",
                params![id],
                row_to_session,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(db_err(other)),
            })?;
        Ok(row)
    }

    fn list_sessions(&self, project: Option<&str>, limit: usize) -> IcmResult<Vec<Session>> {
        let conn = &self.conn;
        match project {
            Some(p) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, agent, project, started_at, updated_at, metadata
                         FROM sessions WHERE project = ?1
                         ORDER BY updated_at DESC LIMIT ?2",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![p, limit as i64], row_to_session)
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                Ok(rows)
            }
            None => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, agent, project, started_at, updated_at, metadata
                         FROM sessions ORDER BY updated_at DESC LIMIT ?1",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![limit as i64], row_to_session)
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                Ok(rows)
            }
        }
    }

    fn record_message(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        tool_name: Option<&str>,
        tokens: Option<i64>,
        metadata: Option<&str>,
    ) -> IcmResult<String> {
        let msg = Message::new(
            session_id.to_string(),
            role,
            content.to_string(),
            tool_name.map(|s| s.to_string()),
            tokens,
            metadata.map(|s| s.to_string()),
        );
        let conn = &self.conn;

        // Ensure the session exists — referential integrity check is friendlier
        // than raw FK failure.
        let session_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sessions WHERE id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if !session_exists {
            return Err(IcmError::NotFound(format!(
                "session {} does not exist",
                session_id
            )));
        }

        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, tool_name, tokens, ts, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                msg.id,
                msg.session_id,
                msg.role.as_str(),
                msg.content,
                msg.tool_name,
                msg.tokens,
                msg.ts.to_rfc3339(),
                msg.metadata,
            ],
        )
        .map_err(db_err)?;
        conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![msg.ts.to_rfc3339(), session_id],
        )
        .map_err(db_err)?;
        Ok(msg.id)
    }

    fn list_session_messages(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> IcmResult<Vec<Message>> {
        let conn = &self.conn;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, role, content, tool_name, tokens, ts, metadata
                 FROM messages WHERE session_id = ?1
                 ORDER BY ts ASC LIMIT ?2 OFFSET ?3",
            )
            .map_err(db_err)?;
        let rows: Vec<Message> = stmt
            .query_map(
                params![session_id, limit as i64, offset as i64],
                row_to_message,
            )
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    fn search_transcripts(
        &self,
        query: &str,
        session_id: Option<&str>,
        project: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<TranscriptHit>> {
        let conn = &self.conn;
        // Build dynamic WHERE filters. FTS MATCH comes first for index usage.
        let mut sql = String::from(
            "SELECT m.id, m.session_id, m.role, m.content, m.tool_name, m.tokens, m.ts, m.metadata,
                    s.id AS s_id, s.agent AS s_agent, s.project AS s_project,
                    s.started_at AS s_started_at, s.updated_at AS s_updated_at,
                    s.metadata AS s_metadata,
                    bm25(messages_fts) AS score
             FROM messages_fts
             JOIN messages m ON m.rowid = messages_fts.rowid
             JOIN sessions s ON s.id = m.session_id
             WHERE messages_fts MATCH ?1",
        );
        // Param numbering: ?1 = query (always). Session if present is ?2.
        // Project is ?2 if no session, else ?3.
        if session_id.is_some() {
            sql.push_str(" AND m.session_id = ?2");
        }
        if project.is_some() {
            if session_id.is_some() {
                sql.push_str(" AND s.project = ?3");
            } else {
                sql.push_str(" AND s.project = ?2");
            }
        }
        sql.push_str(" ORDER BY score ASC LIMIT ?");
        sql.push_str(match (session_id.is_some(), project.is_some()) {
            (true, true) => "4",
            (true, false) | (false, true) => "3",
            (false, false) => "2",
        });

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let limit_i = limit as i64;
        let rows: Vec<TranscriptHit> = match (session_id, project) {
            (Some(sid), Some(p)) => stmt
                .query_map(params![query, sid, p, limit_i], |row| {
                    let msg = Message {
                        id: row.get("id")?,
                        session_id: row.get("session_id")?,
                        role: Role::parse(&row.get::<_, String>("role")?).unwrap_or(Role::Tool),
                        content: row.get("content")?,
                        tool_name: row.get("tool_name")?,
                        tokens: row.get("tokens")?,
                        ts: parse_ts(&row.get::<_, String>("ts")?),
                        metadata: row.get("metadata")?,
                    };
                    let sess = Session {
                        id: row.get("s_id")?,
                        agent: row.get("s_agent")?,
                        project: row.get("s_project")?,
                        started_at: parse_ts(&row.get::<_, String>("s_started_at")?),
                        updated_at: parse_ts(&row.get::<_, String>("s_updated_at")?),
                        metadata: row.get("s_metadata")?,
                    };
                    let raw_score: f64 = row.get("score")?;
                    Ok(TranscriptHit {
                        message: msg,
                        session: sess,
                        score: -raw_score, // FTS5 bm25 returns negative for better rank
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
            (Some(sid), None) => stmt
                .query_map(params![query, sid, limit_i], |row| {
                    let msg = Message {
                        id: row.get("id")?,
                        session_id: row.get("session_id")?,
                        role: Role::parse(&row.get::<_, String>("role")?).unwrap_or(Role::Tool),
                        content: row.get("content")?,
                        tool_name: row.get("tool_name")?,
                        tokens: row.get("tokens")?,
                        ts: parse_ts(&row.get::<_, String>("ts")?),
                        metadata: row.get("metadata")?,
                    };
                    let sess = Session {
                        id: row.get("s_id")?,
                        agent: row.get("s_agent")?,
                        project: row.get("s_project")?,
                        started_at: parse_ts(&row.get::<_, String>("s_started_at")?),
                        updated_at: parse_ts(&row.get::<_, String>("s_updated_at")?),
                        metadata: row.get("s_metadata")?,
                    };
                    let raw_score: f64 = row.get("score")?;
                    Ok(TranscriptHit {
                        message: msg,
                        session: sess,
                        score: -raw_score,
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
            (None, Some(p)) => stmt
                .query_map(params![query, p, limit_i], |row| {
                    let msg = Message {
                        id: row.get("id")?,
                        session_id: row.get("session_id")?,
                        role: Role::parse(&row.get::<_, String>("role")?).unwrap_or(Role::Tool),
                        content: row.get("content")?,
                        tool_name: row.get("tool_name")?,
                        tokens: row.get("tokens")?,
                        ts: parse_ts(&row.get::<_, String>("ts")?),
                        metadata: row.get("metadata")?,
                    };
                    let sess = Session {
                        id: row.get("s_id")?,
                        agent: row.get("s_agent")?,
                        project: row.get("s_project")?,
                        started_at: parse_ts(&row.get::<_, String>("s_started_at")?),
                        updated_at: parse_ts(&row.get::<_, String>("s_updated_at")?),
                        metadata: row.get("s_metadata")?,
                    };
                    let raw_score: f64 = row.get("score")?;
                    Ok(TranscriptHit {
                        message: msg,
                        session: sess,
                        score: -raw_score,
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
            (None, None) => stmt
                .query_map(params![query, limit_i], |row| {
                    let msg = Message {
                        id: row.get("id")?,
                        session_id: row.get("session_id")?,
                        role: Role::parse(&row.get::<_, String>("role")?).unwrap_or(Role::Tool),
                        content: row.get("content")?,
                        tool_name: row.get("tool_name")?,
                        tokens: row.get("tokens")?,
                        ts: parse_ts(&row.get::<_, String>("ts")?),
                        metadata: row.get("metadata")?,
                    };
                    let sess = Session {
                        id: row.get("s_id")?,
                        agent: row.get("s_agent")?,
                        project: row.get("s_project")?,
                        started_at: parse_ts(&row.get::<_, String>("s_started_at")?),
                        updated_at: parse_ts(&row.get::<_, String>("s_updated_at")?),
                        metadata: row.get("s_metadata")?,
                    };
                    let raw_score: f64 = row.get("score")?;
                    Ok(TranscriptHit {
                        message: msg,
                        session: sess,
                        score: -raw_score,
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?,
        };
        Ok(rows)
    }

    fn forget_session(&self, id: &str) -> IcmResult<()> {
        let conn = &self.conn;
        // Explicit delete of messages (in case FK cascade isn't enabled on older DBs).
        conn.execute("DELETE FROM messages WHERE session_id = ?1", params![id])
            .map_err(db_err)?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    fn transcript_stats(&self) -> IcmResult<TranscriptStats> {
        let conn = &self.conn;

        let total_sessions: usize = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .map_err(db_err)?;
        let total_messages: usize = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .map_err(db_err)?;
        let total_bytes: u64 = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(content)), 0) FROM messages",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(db_err)? as u64;

        let mut stmt_role = conn
            .prepare("SELECT role, COUNT(*) FROM messages GROUP BY role ORDER BY 2 DESC")
            .map_err(db_err)?;
        let by_role: Vec<(String, usize)> = stmt_role
            .query_map([], |r: &rusqlite::Row<'_>| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;

        let mut stmt_agent = conn
            .prepare("SELECT agent, COUNT(*) FROM sessions GROUP BY agent ORDER BY 2 DESC")
            .map_err(db_err)?;
        let by_agent: Vec<(String, usize)> = stmt_agent
            .query_map([], |r: &rusqlite::Row<'_>| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;

        let mut stmt_top = conn
            .prepare(
                "SELECT session_id, COUNT(*) FROM messages
                 GROUP BY session_id ORDER BY 2 DESC LIMIT 10",
            )
            .map_err(db_err)?;
        let top_sessions: Vec<(String, usize)> = stmt_top
            .query_map([], |r: &rusqlite::Row<'_>| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;

        let (oldest, newest): (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = conn
            .query_row(
                "SELECT MIN(ts), MAX(ts) FROM messages",
                [],
                |r: &rusqlite::Row<'_>| {
                    let o: Option<String> = r.get(0)?;
                    let n: Option<String> = r.get(1)?;
                    Ok((o.as_deref().map(parse_ts), n.as_deref().map(parse_ts)))
                },
            )
            .unwrap_or((None, None));

        Ok(TranscriptStats {
            total_sessions,
            total_messages,
            total_bytes,
            by_role,
            by_agent,
            top_sessions,
            oldest,
            newest,
        })
    }
}

// ---------------------------------------------------------------------------
// Auto-consolidation, prefix queries, and pattern detection
// ---------------------------------------------------------------------------

impl SqliteStore {
    /// Would inserting an edge `source → target` close a cycle in the
    /// concept graph? BFS from `target` along outgoing edges; if we
    /// reach `source`, the new edge would form a cycle.
    ///
    /// Returns `Ok(false)` for the empty-graph case and bounds the
    /// search to a depth-limit equal to twice the link count to avoid
    /// pathological loops on already-corrupt graphs.
    fn would_create_cycle(&self, source: &str, target: &str) -> IcmResult<bool> {
        if source == target {
            return Ok(true);
        }
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(target.to_string());
        visited.insert(target.to_string());
        // Soft cap on traversal — guards against malformed pre-existing
        // cycles (which shouldn't exist, but the user could have edited
        // the DB by hand).
        let cap = 10_000;
        let mut steps = 0;
        while let Some(current) = queue.pop_front() {
            steps += 1;
            if steps > cap {
                tracing::warn!(
                    "would_create_cycle: BFS hit {cap}-step cap while checking {source} → {target}"
                );
                break;
            }
            for next_link in self.get_links_from(&current)? {
                if next_link.target_id == source {
                    return Ok(true);
                }
                if visited.insert(next_link.target_id.clone()) {
                    queue.push_back(next_link.target_id);
                }
            }
        }
        Ok(false)
    }

    /// Automatically consolidate a topic if it exceeds the threshold.
    ///
    /// Keeps the top 3 summaries (by weight), merges all unique keywords,
    /// and replaces all memories with a single consolidated memory.
    /// Returns `true` if consolidation was performed.
    ///
    /// Backwards-compatible no-embedder variant. Prefer
    /// [`auto_consolidate_with_embedder`] for new code so the
    /// consolidated memory keeps a fresh embedding instead of being
    /// silently un-recallable via vector search.
    pub fn auto_consolidate(&self, topic: &str, threshold: usize) -> IcmResult<bool> {
        self.auto_consolidate_with_embedder(topic, threshold, None)
    }

    /// Same as [`auto_consolidate`] but also embeds the consolidated
    /// memory when an embedder is available.
    ///
    /// Audit finding M2/AC2: the no-embedder variant produced a
    /// consolidated memory with `embedding = None`, leaving it
    /// invisible to hybrid / vector search until a manual `icm embed`
    /// rebuilt it. With this variant the embedder is invoked inline so
    /// the consolidated memory is recall-ready as soon as the topic is
    /// rolled up.
    pub fn auto_consolidate_with_embedder(
        &self,
        topic: &str,
        threshold: usize,
        embedder: Option<&dyn Embedder>,
    ) -> IcmResult<bool> {
        let count = self.count_by_topic(topic)?;
        if count < threshold {
            return Ok(false);
        }

        let mut memories = self.get_by_topic(topic)?;
        if memories.is_empty() {
            return Ok(false);
        }

        // Sort by weight DESC (get_by_topic already does this, but be explicit)
        memories.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take the top 3 summaries for the consolidated summary
        let top_summaries: Vec<&str> = memories
            .iter()
            .take(3)
            .map(|m| m.summary.as_str())
            .collect();
        let consolidated_summary = top_summaries.join(" | ");

        // Merge all unique keywords
        let mut all_keywords: Vec<String> = Vec::new();
        let mut seen_keywords: HashSet<String> = HashSet::new();
        for mem in &memories {
            for kw in &mem.keywords {
                let lower = kw.to_lowercase();
                if seen_keywords.insert(lower) {
                    all_keywords.push(kw.clone());
                }
            }
        }

        let original_count = memories.len();

        // Build the consolidated memory
        let mut consolidated = Memory::new(topic.into(), consolidated_summary, Importance::High);
        consolidated.keywords = all_keywords;
        consolidated.raw_excerpt =
            Some(format!("auto-consolidated from {original_count} memories"));
        consolidated.weight = 1.0;

        // Embed the consolidated content if an embedder is available so
        // hybrid recall picks it up immediately. Errors are logged and
        // swallowed — a partial consolidation (no embedding) is still
        // better than blocking the whole rollup on an embedder hiccup.
        if let Some(emb) = embedder {
            match emb.embed(&consolidated.embed_text()) {
                Ok(vec) => consolidated.embedding = Some(vec),
                Err(e) => {
                    tracing::warn!(
                        "auto-consolidate: embedding failed for topic '{topic}': {e}; \
                         consolidated memory will lack vector representation"
                    );
                }
            }
        }

        // Replace all memories in the topic with the consolidated one
        self.consolidate_topic(topic, consolidated)?;

        Ok(true)
    }

    /// Graph expansion: given a list of `(Memory, score)` results from a
    /// primary search, follow each memory's `related_ids` one hop and fetch
    /// the neighbors that are not already in the result set.
    ///
    /// Each neighbor is scored as `parent_score * hop_discount` (default
    /// 0.5) so it ranks below its direct-match parent but above unrelated
    /// low-score results. Returns the combined, deduped, score-descending
    /// list capped at `max_total` (pass `usize::MAX` for no cap).
    ///
    /// This is the core of the graph-aware recall feature: it lets the
    /// recall path surface memories that are semantically or causally
    /// linked to the query's direct matches, even if they don't match the
    /// query text themselves.
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

        // Phase 1 — collect candidate neighbour ids in score-priority order
        // up to max_neighbors. We track parent scores here so we can apply
        // the hop discount once memories come back from the batched fetch.
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

        // Phase 2 — single batched SELECT instead of N round-trips.
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

        // Merge initial + neighbors, then sort descending by score.
        let mut combined: Vec<(Memory, f32)> = initial.to_vec();
        combined.extend(neighbors);
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(max_total);
        Ok(combined)
    }

    /// Fetch many memories by id in one round-trip, deduplicated by id.
    ///
    /// Cache-aware: cached entries are served from memory, misses are
    /// batched into a single `IN (?,?,…)` query. Missing ids are
    /// silently dropped — callers expecting a strict mapping should
    /// diff their input vs the returned map.
    pub fn get_many(&self, ids: &[&str]) -> IcmResult<HashMap<String, Memory>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        // Dedup so the IN clause gets at most one slot per id.
        let mut unique: Vec<&str> = Vec::with_capacity(ids.len());
        let mut seen: HashSet<&str> = HashSet::new();
        for id in ids {
            if seen.insert(*id) {
                unique.push(*id);
            }
        }

        // Phase 1 — pull cache hits aside, leaving only true misses for SQL.
        let mut out: HashMap<String, Memory> = HashMap::with_capacity(unique.len());
        let mut misses: Vec<&str> = Vec::with_capacity(unique.len());
        if let Ok(mut cache) = self.cache.lock() {
            for id in &unique {
                if let Some(m) = cache.get(*id) {
                    out.insert((*id).to_string(), m.clone());
                } else {
                    misses.push(*id);
                }
            }
        } else {
            misses.extend_from_slice(&unique);
        }

        if misses.is_empty() {
            return Ok(out);
        }

        // Phase 2 — single batched SELECT for the misses.
        let placeholders: Vec<String> = (1..=misses.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT {SELECT_COLS} FROM memories WHERE id IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let params_vec: Vec<&dyn rusqlite::types::ToSql> = misses
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt
            .query_map(params_vec.as_slice(), row_to_memory)
            .map_err(db_err)?;

        let mut fetched: Vec<Memory> = Vec::new();
        for row in rows {
            fetched.push(row.map_err(db_err)?);
        }

        if let Ok(mut cache) = self.cache.lock() {
            for m in &fetched {
                cache.put(m.id.clone(), m.clone());
            }
        }
        for m in fetched {
            out.insert(m.id.clone(), m);
        }
        Ok(out)
    }

    /// Get memories by topic prefix (e.g., "wshm" matches "wshm:owner/repo").
    ///
    /// If `topic` ends with `*`, uses LIKE matching. Otherwise exact match.
    pub fn get_by_topic_prefix(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        if let Some(prefix) = topic.strip_suffix('*') {
            let pattern = format!("{prefix}%");
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT {SELECT_COLS} FROM memories WHERE topic LIKE ?1 ORDER BY weight DESC LIMIT 500"
                ))
                .map_err(db_err)?;

            let rows = stmt
                .query_map(params![pattern], row_to_memory)
                .map_err(db_err)?;

            collect_rows(rows)
        } else {
            self.get_by_topic(topic)
        }
    }

    /// List topics, optionally filtered by a prefix.
    pub fn list_topics_with_prefix(&self, prefix: Option<&str>) -> IcmResult<Vec<(String, usize)>> {
        match prefix {
            Some(p) => {
                let pattern = format!("{p}%");
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT topic, COUNT(*) FROM memories WHERE topic LIKE ?1 GROUP BY topic ORDER BY topic",
                    )
                    .map_err(db_err)?;

                let rows = stmt
                    .query_map(params![pattern], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
                    })
                    .map_err(db_err)?;

                let mut results = Vec::new();
                for row in rows {
                    results.push(row.map_err(db_err)?);
                }
                Ok(results)
            }
            None => self.list_topics(),
        }
    }

    /// Detect recurring patterns in a topic by computing Jaccard similarity on keywords.
    ///
    /// Groups memories with keyword similarity > 0.5 into clusters,
    /// and returns clusters of size >= `min_cluster_size`.
    pub fn detect_patterns(
        &self,
        topic: &str,
        min_cluster_size: usize,
    ) -> IcmResult<Vec<PatternCluster>> {
        let memories = self.get_by_topic(topic)?;
        if memories.len() < min_cluster_size {
            return Ok(Vec::new());
        }

        // Build keyword sets for each memory
        let keyword_sets: Vec<HashSet<String>> = memories
            .iter()
            .map(|m| m.keywords.iter().map(|k| k.to_lowercase()).collect())
            .collect();

        // Union-Find-style clustering via adjacency
        let n = memories.len();
        let mut parent: Vec<usize> = (0..n).collect();

        fn find(parent: &mut [usize], i: usize) -> usize {
            let mut i = i;
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }

        fn union(parent: &mut [usize], a: usize, b: usize) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra != rb {
                parent[ra] = rb;
            }
        }

        // Compute Jaccard similarity for each pair, union if > 0.5
        for i in 0..n {
            for j in (i + 1)..n {
                if keyword_sets[i].is_empty() && keyword_sets[j].is_empty() {
                    continue;
                }
                let intersection = keyword_sets[i].intersection(&keyword_sets[j]).count();
                let union_size = keyword_sets[i].union(&keyword_sets[j]).count();
                if union_size > 0 {
                    let jaccard = intersection as f32 / union_size as f32;
                    if jaccard > 0.5 {
                        union(&mut parent, i, j);
                    }
                }
            }
        }

        // Group by cluster root
        let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let root = find(&mut parent, i);
            clusters.entry(root).or_default().push(i);
        }

        // Build PatternCluster for each group meeting the minimum size
        let mut result: Vec<PatternCluster> = Vec::new();
        for indices in clusters.values() {
            if indices.len() < min_cluster_size {
                continue;
            }

            // Representative = the highest-weight memory in the cluster
            let best_idx = match indices.iter().max_by(|&&a, &&b| {
                memories[a]
                    .weight
                    .partial_cmp(&memories[b].weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                Some(&idx) => idx,
                None => continue, // empty cluster, skip
            };

            // Collect all unique keywords from the cluster
            let mut all_kw: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for &idx in indices {
                for kw in &memories[idx].keywords {
                    let lower = kw.to_lowercase();
                    if seen.insert(lower) {
                        all_kw.push(kw.clone());
                    }
                }
            }

            result.push(PatternCluster {
                representative_summary: memories[best_idx].summary.clone(),
                memory_ids: indices.iter().map(|&i| memories[i].id.clone()).collect(),
                keywords: all_kw,
                count: indices.len(),
            });
        }

        // Sort by cluster size descending
        result.sort_by_key(|b| std::cmp::Reverse(b.count));

        Ok(result)
    }

    /// Extract a pattern cluster as a concept in a memoir.
    ///
    /// Creates a Concept with:
    /// - name derived from common keywords
    /// - definition = combined summary of the cluster
    /// - source_memory_ids = memory IDs in the cluster
    /// - confidence = 0.5 + (count * 0.05) capped at 0.9
    /// - labels = common keywords as labels
    pub fn extract_pattern_as_concept(
        &self,
        cluster: &PatternCluster,
        memoir_id: &str,
    ) -> IcmResult<String> {
        // Derive concept name from top keywords
        let concept_name = if cluster.keywords.is_empty() {
            format!("pattern-{}", &cluster.memory_ids[0][..8])
        } else {
            cluster
                .keywords
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("-")
        };

        // Build definition from cluster representative + count
        let definition = format!(
            "{} (pattern detected across {} memories)",
            cluster.representative_summary, cluster.count
        );

        let mut concept = Concept::new(memoir_id.into(), concept_name, definition);
        concept.source_memory_ids = cluster.memory_ids.clone();
        concept.confidence = (0.5 + cluster.count as f32 * 0.05).min(0.9);
        concept.labels = cluster
            .keywords
            .iter()
            .take(5)
            .map(|kw| Label::new("pattern", kw.as_str()))
            .collect();

        self.add_concept(concept)
    }
}

// ---------------------------------------------------------------------------
// Test helpers (visible to other modules in crate for test use)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::ensure_sqlite_vec;

    pub fn ensure_vec_init() {
        ensure_sqlite_vec();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use icm_core::Importance;

    fn test_store() -> SqliteStore {
        SqliteStore::in_memory().unwrap()
    }

    fn make_memory(topic: &str, summary: &str) -> Memory {
        Memory::new(topic.into(), summary.into(), Importance::Medium)
    }

    fn make_memoir(name: &str) -> Memoir {
        Memoir::new(name.into(), format!("Description for {name}"))
    }

    fn make_concept(memoir_id: &str, name: &str, definition: &str) -> Concept {
        Concept::new(memoir_id.into(), name.into(), definition.into())
    }

    // === MemoryStore tests ===

    #[test]
    fn test_store_and_get() {
        let store = test_store();
        let mem = make_memory("test", "hello world");
        let id = mem.id.clone();

        store.store(mem).unwrap();
        let retrieved = store.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.summary, "hello world");
        assert_eq!(retrieved.topic, "test");
    }

    #[test]
    fn test_get_not_found() {
        let store = test_store();
        let result = store.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_update() {
        let store = test_store();
        let mut mem = make_memory("test", "original");
        let id = mem.id.clone();
        store.store(mem.clone()).unwrap();

        mem.summary = "updated".into();
        store.update(&mem).unwrap();

        let retrieved = store.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.summary, "updated");
    }

    #[test]
    fn test_delete() {
        let store = test_store();
        let mem = make_memory("test", "to delete");
        let id = mem.id.clone();
        store.store(mem).unwrap();

        store.delete(&id).unwrap();
        assert!(store.get(&id).unwrap().is_none());
    }

    #[test]
    fn test_delete_not_found() {
        let store = test_store();
        let result = store.delete("nonexistent");
        assert!(matches!(result, Err(IcmError::NotFound(_))));
    }

    #[test]
    fn test_search_fts() {
        let store = test_store();
        store
            .store(make_memory(
                "rust",
                "Rust is a systems programming language",
            ))
            .unwrap();
        store
            .store(make_memory("python", "Python is great for scripting"))
            .unwrap();

        let results = store.search_fts("rust programming", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic, "rust");
    }

    #[test]
    fn test_search_by_keywords() {
        let store = test_store();
        let mut mem = make_memory("test", "database optimization tips");
        mem.keywords = vec!["database".into(), "optimization".into()];
        store.store(mem).unwrap();

        let results = store.search_by_keywords(&["database"], 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_list_topics() {
        let store = test_store();
        store.store(make_memory("alpha", "first")).unwrap();
        store.store(make_memory("alpha", "second")).unwrap();
        store.store(make_memory("beta", "third")).unwrap();

        let topics = store.list_topics().unwrap();
        assert_eq!(topics.len(), 2);
        assert!(topics.contains(&("alpha".into(), 2)));
        assert!(topics.contains(&("beta".into(), 1)));
    }

    #[test]
    fn test_apply_decay() {
        let store = test_store();
        store.store(make_memory("test", "decayable")).unwrap();

        let mut critical = make_memory("test", "critical memory");
        critical.importance = Importance::Critical;
        store.store(critical).unwrap();

        let affected = store.apply_decay(0.9).unwrap();
        assert_eq!(affected, 1); // Only the non-critical one
    }

    #[test]
    fn test_prune() {
        let store = test_store();
        let mut low = make_memory("test", "low weight");
        low.weight = 0.05;
        store.store(low).unwrap();

        store.store(make_memory("test", "normal weight")).unwrap();

        let pruned = store.prune(0.1).unwrap();
        assert_eq!(pruned, 1);
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn test_stats() {
        let store = test_store();
        store.store(make_memory("a", "first")).unwrap();
        store.store(make_memory("b", "second")).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.total_memories, 2);
        assert_eq!(stats.total_topics, 2);
        assert!(stats.avg_weight > 0.0);
        assert!(stats.oldest_memory.is_some());
        assert!(stats.newest_memory.is_some());
    }

    #[test]
    fn test_update_access() {
        let store = test_store();
        let mem = make_memory("test", "access test");
        let id = mem.id.clone();
        store.store(mem).unwrap();

        store.update_access(&id).unwrap();
        let retrieved = store.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.access_count, 1);
    }

    #[test]
    fn test_consolidate_topic() {
        let store = test_store();
        store.store(make_memory("topic-a", "entry 1")).unwrap();
        store.store(make_memory("topic-a", "entry 2")).unwrap();
        store.store(make_memory("topic-b", "other")).unwrap();

        let consolidated = make_memory("topic-a", "consolidated summary");
        store.consolidate_topic("topic-a", consolidated).unwrap();

        let memories = store.get_by_topic("topic-a").unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].summary, "consolidated summary");

        // topic-b should be untouched
        assert_eq!(store.get_by_topic("topic-b").unwrap().len(), 1);
    }

    /// Reproduces issue #44: after consolidation, recall should only return the
    /// consolidated memory — not stale fragments from the originals.
    #[test]
    fn test_consolidate_no_stale_fts_results() {
        let store = test_store();

        // Step 1: store 3 related memories on the same topic
        store
            .store(make_memory(
                "errors-resolved",
                "fix: null pointer in parser",
            ))
            .unwrap();
        store
            .store(make_memory(
                "errors-resolved",
                "fix: timeout in HTTP client",
            ))
            .unwrap();
        store
            .store(make_memory(
                "errors-resolved",
                "fix: race condition in cache",
            ))
            .unwrap();

        // Verify FTS finds them before consolidation
        let before = store.search_fts("fix", 10).unwrap();
        assert_eq!(before.len(), 3);

        // Step 2: consolidate
        let consolidated = make_memory(
            "errors-resolved",
            "All errors resolved: parser, HTTP, cache",
        );
        store
            .consolidate_topic("errors-resolved", consolidated)
            .unwrap();

        // Step 3: recall — should only return the consolidated memory
        let after = store.search_fts("fix", 10).unwrap();
        assert!(
            after.len() <= 1,
            "expected at most 1 result after consolidation, got {}",
            after.len()
        );

        // The consolidated memory should be findable
        let consolidated_results = store.search_fts("errors resolved parser", 10).unwrap();
        assert_eq!(consolidated_results.len(), 1);
        assert!(consolidated_results[0]
            .summary
            .contains("All errors resolved"));

        // Verify topic has exactly 1 memory
        let topic_mems = store.get_by_topic("errors-resolved").unwrap();
        assert_eq!(topic_mems.len(), 1);
    }

    // === MemoirStore tests ===

    #[test]
    fn test_memoir_crud() {
        let store = test_store();
        let m = make_memoir("my-project");
        let id = store.create_memoir(m).unwrap();

        let retrieved = store.get_memoir(&id).unwrap().unwrap();
        assert_eq!(retrieved.name, "my-project");

        let by_name = store.get_memoir_by_name("my-project").unwrap().unwrap();
        assert_eq!(by_name.id, id);

        store.delete_memoir(&id).unwrap();
        assert!(store.get_memoir(&id).unwrap().is_none());
    }

    #[test]
    fn test_memoir_unique_name() {
        let store = test_store();
        store.create_memoir(make_memoir("dup")).unwrap();
        let result = store.create_memoir(make_memoir("dup"));
        assert!(result.is_err());
    }

    #[test]
    fn test_list_memoirs() {
        let store = test_store();
        store.create_memoir(make_memoir("beta")).unwrap();
        store.create_memoir(make_memoir("alpha")).unwrap();

        let list = store.list_memoirs().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha"); // sorted by name
        assert_eq!(list[1].name, "beta");
    }

    #[test]
    fn test_concept_crud() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();

        let mut c = make_concept(&m_id, "event-sourcing", "Events stored in SQLite");
        c.labels = vec![Label::new("domain", "arch"), Label::new("type", "decision")];
        let c_id = store.add_concept(c).unwrap();

        let retrieved = store.get_concept(&c_id).unwrap().unwrap();
        assert_eq!(retrieved.name, "event-sourcing");
        assert_eq!(retrieved.labels.len(), 2);

        let by_name = store
            .get_concept_by_name(&m_id, "event-sourcing")
            .unwrap()
            .unwrap();
        assert_eq!(by_name.id, c_id);

        store.delete_concept(&c_id).unwrap();
        assert!(store.get_concept(&c_id).unwrap().is_none());
    }

    #[test]
    fn test_concept_unique_within_memoir() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();

        store
            .add_concept(make_concept(&m_id, "dup", "first"))
            .unwrap();
        let result = store.add_concept(make_concept(&m_id, "dup", "second"));
        assert!(result.is_err());
    }

    #[test]
    fn test_concept_same_name_different_memoirs() {
        let store = test_store();
        let m1 = store.create_memoir(make_memoir("proj1")).unwrap();
        let m2 = store.create_memoir(make_memoir("proj2")).unwrap();

        store
            .add_concept(make_concept(&m1, "sqlite", "def1"))
            .unwrap();
        store
            .add_concept(make_concept(&m2, "sqlite", "def2"))
            .unwrap();

        let c1 = store.get_concept_by_name(&m1, "sqlite").unwrap().unwrap();
        let c2 = store.get_concept_by_name(&m2, "sqlite").unwrap().unwrap();
        assert_ne!(c1.id, c2.id);
    }

    #[test]
    fn test_refine_concept() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let c_id = store
            .add_concept(make_concept(&m_id, "es", "Events v1"))
            .unwrap();

        let orig = store.get_concept(&c_id).unwrap().unwrap();
        assert_eq!(orig.revision, 1);
        let orig_confidence = orig.confidence;

        store
            .refine_concept(&c_id, "Events v2 with snapshots", &["mem-1".into()])
            .unwrap();

        let refined = store.get_concept(&c_id).unwrap().unwrap();
        assert_eq!(refined.revision, 2);
        assert_eq!(refined.definition, "Events v2 with snapshots");
        assert!(refined.confidence > orig_confidence);
        assert!(refined.source_memory_ids.contains(&"mem-1".into()));
    }

    #[test]
    fn test_concept_links() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let c1_id = store
            .add_concept(make_concept(&m_id, "event-sourcing", "ES pattern"))
            .unwrap();
        let c2_id = store
            .add_concept(make_concept(&m_id, "sqlite", "SQLite storage"))
            .unwrap();

        let link = ConceptLink::new(c1_id.clone(), c2_id.clone(), Relation::DependsOn);
        let link_id = store.add_link(link).unwrap();

        let from = store.get_links_from(&c1_id).unwrap();
        assert_eq!(from.len(), 1);
        assert_eq!(from[0].target_id, c2_id);
        assert_eq!(from[0].relation, Relation::DependsOn);

        let to = store.get_links_to(&c2_id).unwrap();
        assert_eq!(to.len(), 1);
        assert_eq!(to[0].source_id, c1_id);

        store.delete_link(&link_id).unwrap();
        assert!(store.get_links_from(&c1_id).unwrap().is_empty());
    }

    #[test]
    fn test_self_link_rejected() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let c_id = store
            .add_concept(make_concept(&m_id, "concept", "def"))
            .unwrap();

        let link = ConceptLink::new(c_id.clone(), c_id, Relation::RelatedTo);
        let result = store.add_link(link);
        assert!(result.is_err());
    }

    #[test]
    fn test_transitive_cycle_rejected() {
        // Audit M11/CYC1: A → B → C → A used to be silently accepted,
        // corrupting BFS in `get_neighborhood`. Now the third edge
        // (closing the cycle) is rejected with `InvalidInput`.
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let a = store.add_concept(make_concept(&m_id, "A", "a")).unwrap();
        let b = store.add_concept(make_concept(&m_id, "B", "b")).unwrap();
        let c = store.add_concept(make_concept(&m_id, "C", "c")).unwrap();

        // A → B: ok
        store
            .add_link(ConceptLink::new(a.clone(), b.clone(), Relation::DependsOn))
            .unwrap();
        // B → C: ok
        store
            .add_link(ConceptLink::new(b.clone(), c.clone(), Relation::Refines))
            .unwrap();
        // C → A: would close the cycle — reject
        let cycle_attempt = store.add_link(ConceptLink::new(c, a, Relation::RelatedTo));
        assert!(
            cycle_attempt.is_err(),
            "C → A should be rejected as a cycle"
        );
        let err_msg = cycle_attempt.unwrap_err().to_string();
        assert!(
            err_msg.contains("cycle"),
            "error message should mention cycle: {err_msg}"
        );
    }

    #[test]
    fn test_dag_links_still_allowed() {
        // Sanity: rejecting cycles must not break legitimate DAG links.
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let a = store.add_concept(make_concept(&m_id, "A", "a")).unwrap();
        let b = store.add_concept(make_concept(&m_id, "B", "b")).unwrap();
        let c = store.add_concept(make_concept(&m_id, "C", "c")).unwrap();

        // A → B, A → C, B → C — three edges in a DAG, all should pass.
        store
            .add_link(ConceptLink::new(a.clone(), b.clone(), Relation::DependsOn))
            .unwrap();
        store
            .add_link(ConceptLink::new(a, c.clone(), Relation::DependsOn))
            .unwrap();
        store
            .add_link(ConceptLink::new(b, c, Relation::Refines))
            .unwrap();
    }

    #[test]
    fn test_get_neighbors() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let c1 = store
            .add_concept(make_concept(&m_id, "a", "node a"))
            .unwrap();
        let c2 = store
            .add_concept(make_concept(&m_id, "b", "node b"))
            .unwrap();
        let c3 = store
            .add_concept(make_concept(&m_id, "c", "node c"))
            .unwrap();

        store
            .add_link(ConceptLink::new(
                c1.clone(),
                c2.clone(),
                Relation::DependsOn,
            ))
            .unwrap();
        store
            .add_link(ConceptLink::new(c3.clone(), c1.clone(), Relation::PartOf))
            .unwrap();

        let neighbors = store.get_neighbors(&c1, None).unwrap();
        assert_eq!(neighbors.len(), 2);

        let dep_neighbors = store.get_neighbors(&c1, Some(Relation::DependsOn)).unwrap();
        assert_eq!(dep_neighbors.len(), 1);
        assert_eq!(dep_neighbors[0].name, "b");
    }

    #[test]
    fn test_get_neighborhood_bfs() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let c1 = store
            .add_concept(make_concept(&m_id, "a", "node a"))
            .unwrap();
        let c2 = store
            .add_concept(make_concept(&m_id, "b", "node b"))
            .unwrap();
        let c3 = store
            .add_concept(make_concept(&m_id, "c", "node c"))
            .unwrap();
        let c4 = store
            .add_concept(make_concept(&m_id, "d", "node d"))
            .unwrap();

        // a -> b -> c -> d
        store
            .add_link(ConceptLink::new(
                c1.clone(),
                c2.clone(),
                Relation::DependsOn,
            ))
            .unwrap();
        store
            .add_link(ConceptLink::new(
                c2.clone(),
                c3.clone(),
                Relation::DependsOn,
            ))
            .unwrap();
        store
            .add_link(ConceptLink::new(c3, c4, Relation::DependsOn))
            .unwrap();

        // depth=1 should get a + b
        let (concepts, links) = store.get_neighborhood(&c1, 1).unwrap();
        assert_eq!(concepts.len(), 2);
        assert!(!links.is_empty());

        // depth=2 should get a + b + c
        let (concepts, _) = store.get_neighborhood(&c1, 2).unwrap();
        assert_eq!(concepts.len(), 3);

        // depth=3 should get all 4
        let (concepts, _) = store.get_neighborhood(&c1, 3).unwrap();
        assert_eq!(concepts.len(), 4);
    }

    #[test]
    fn test_cascade_delete_memoir() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();
        let c1 = store.add_concept(make_concept(&m_id, "a", "def")).unwrap();
        let c2 = store.add_concept(make_concept(&m_id, "b", "def")).unwrap();
        store
            .add_link(ConceptLink::new(c1, c2, Relation::RelatedTo))
            .unwrap();

        store.delete_memoir(&m_id).unwrap();

        // Concepts and links should be gone
        let concepts = store.list_concepts(&m_id).unwrap();
        assert!(concepts.is_empty());
    }

    #[test]
    fn test_memoir_stats() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();

        let mut c = make_concept(&m_id, "es", "event sourcing");
        c.labels = vec![Label::new("domain", "arch")];
        let c1 = store.add_concept(c).unwrap();

        let mut c = make_concept(&m_id, "sqlite", "sqlite storage");
        c.labels = vec![Label::new("domain", "arch"), Label::new("type", "tech")];
        let c2 = store.add_concept(c).unwrap();

        store
            .add_link(ConceptLink::new(c1, c2, Relation::DependsOn))
            .unwrap();

        let stats = store.memoir_stats(&m_id).unwrap();
        assert_eq!(stats.total_concepts, 2);
        assert_eq!(stats.total_links, 1);
        assert!(stats.avg_confidence > 0.0);
        assert!(!stats.label_counts.is_empty());
    }

    #[test]
    fn test_search_concepts_fts() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();

        store
            .add_concept(make_concept(
                &m_id,
                "event-sourcing",
                "Store domain events in append-only log",
            ))
            .unwrap();
        store
            .add_concept(make_concept(
                &m_id,
                "cqrs",
                "Command Query Responsibility Segregation",
            ))
            .unwrap();

        let results = store.search_concepts_fts(&m_id, "events", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "event-sourcing");
    }

    #[test]
    fn test_search_concepts_by_label() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();

        let mut c1 = make_concept(&m_id, "es", "event sourcing");
        c1.labels = vec![Label::new("domain", "arch")];
        store.add_concept(c1).unwrap();

        let mut c2 = make_concept(&m_id, "sqlite", "storage");
        c2.labels = vec![Label::new("domain", "tech")];
        store.add_concept(c2).unwrap();

        let results = store
            .search_concepts_by_label(&m_id, &Label::new("domain", "arch"), 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "es");
    }

    // === Vector search tests ===

    #[test]
    fn test_store_with_embedding() {
        let store = test_store();
        let mut mem = make_memory("test", "vector enabled");
        mem.embedding = Some(vec![0.1; 384]);
        let id = store.store(mem).unwrap();

        let retrieved = store.get(&id).unwrap().unwrap();
        assert!(retrieved.embedding.is_some());
        assert_eq!(retrieved.embedding.as_ref().unwrap().len(), 384);
    }

    #[test]
    fn test_store_without_embedding() {
        let store = test_store();
        let mem = make_memory("test", "no vector");
        let id = store.store(mem).unwrap();

        let retrieved = store.get(&id).unwrap().unwrap();
        assert!(retrieved.embedding.is_none());
    }

    #[test]
    fn test_search_by_embedding() {
        let store = test_store();

        // Store 3 memories with different embeddings
        let mut m1 = make_memory("rust", "Rust systems programming");
        m1.embedding = Some(vec![
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ]);
        store.store(m1).unwrap();

        let mut m2 = make_memory("python", "Python scripting");
        // Very different embedding
        let mut emb2 = vec![0.0; 384];
        emb2[1] = 1.0;
        m2.embedding = Some(emb2);
        store.store(m2).unwrap();

        // Store one without embedding
        store.store(make_memory("go", "Go programming")).unwrap();

        // Search with a query vector close to m1
        let mut query = vec![0.0; 384];
        query[0] = 0.9;
        let results = store.search_by_embedding(&query, 5).unwrap();

        assert!(!results.is_empty());
        // First result should be closest to query
        assert_eq!(results[0].0.topic, "rust");
    }

    #[test]
    fn test_delete_cleans_vec_table() {
        let store = test_store();
        let mut mem = make_memory("test", "to delete with vec");
        mem.embedding = Some(vec![0.5; 384]);
        let id = store.store(mem).unwrap();

        store.delete(&id).unwrap();

        // Verify vec_memories is also cleaned
        let query = vec![0.5; 384];
        let results = store.search_by_embedding(&query, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_hybrid() {
        let store = test_store();

        // Store memory with both text and embedding
        let mut mem = make_memory("rust", "Rust is great for systems programming");
        mem.embedding = Some(vec![0.8; 384]);
        store.store(mem).unwrap();

        let mut mem2 = make_memory("python", "Python is great for scripting");
        let mut emb2 = vec![0.0; 384];
        emb2[1] = 1.0;
        mem2.embedding = Some(emb2);
        store.store(mem2).unwrap();

        // Hybrid search with both text match and close embedding
        let query_emb = vec![0.7; 384]; // close to m1's embedding
        let results = store
            .search_hybrid("rust programming", &query_emb, 5)
            .unwrap();

        assert!(!results.is_empty());
        // Rust should rank first (matches both FTS and vector)
        assert_eq!(results[0].0.topic, "rust");
        // Score should be > 0
        assert!(results[0].1 > 0.0);
    }

    #[test]
    fn test_sanitize_fts_query() {
        // Normal words get quoted
        assert_eq!(sanitize_fts_query("hello world"), "\"hello\" \"world\"");

        // Special chars become spaces, splitting into separate tokens
        assert_eq!(sanitize_fts_query("sqlite-vec"), "\"sqlite\" \"vec\"");
        assert_eq!(sanitize_fts_query("foo*bar"), "\"foo\" \"bar\"");
        assert_eq!(sanitize_fts_query("col:value"), "\"col\" \"value\"");

        // Empty/whitespace returns empty
        assert_eq!(sanitize_fts_query(""), "");
        assert_eq!(sanitize_fts_query("  "), "");
        assert_eq!(sanitize_fts_query("---"), "");

        // Mixed content
        assert_eq!(
            sanitize_fts_query("no-such column:vec"),
            "\"no\" \"such\" \"column\" \"vec\""
        );
    }

    #[test]
    fn test_search_fts_special_chars() {
        let store = test_store();
        store
            .store(make_memory(
                "tools",
                "sqlite-vec is a vector search extension",
            ))
            .unwrap();

        // This query used to crash with "no such column: vec"
        let results = store.search_fts("sqlite-vec", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic, "tools");

        // Pure special chars should return empty, not error
        let results = store.search_fts("---", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_concepts_fts_special_chars() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("proj")).unwrap();

        store
            .add_concept(make_concept(
                &m_id,
                "sqlite-vec",
                "Vector search extension for SQLite",
            ))
            .unwrap();

        // Should not crash with special chars in query
        let results = store.search_concepts_fts(&m_id, "sqlite-vec", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "sqlite-vec");

        // Pure special chars should return empty
        let results = store.search_concepts_fts(&m_id, "***", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_sql_injection_in_topic() {
        let store = test_store();
        let mem = make_memory("'; DROP TABLE memories; --", "should be safe");
        store.store(mem.clone()).unwrap();

        let retrieved = store.get(&mem.id).unwrap().unwrap();
        assert_eq!(retrieved.topic, "'; DROP TABLE memories; --");
        assert_eq!(store.count().unwrap(), 1);
        let topics = store.list_topics().unwrap();
        assert_eq!(topics.len(), 1);
    }

    #[test]
    fn test_sql_injection_in_summary() {
        let store = test_store();
        let mem = make_memory("test", "value'); DELETE FROM memories WHERE ('1'='1");
        store.store(mem).unwrap();
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn test_sql_injection_in_fts_query() {
        let store = test_store();
        store
            .store(make_memory("test", "normal content here"))
            .unwrap();

        // FTS5 injection attempts
        let results = store.search_fts("') OR 1=1 --", 10).unwrap();
        assert!(results.is_empty() || results.len() <= 1);

        let results = store.search_fts("NEAR(a b)", 10).unwrap();
        let _ = results;
    }

    #[test]
    fn test_sql_injection_in_keywords() {
        let store = test_store();
        let mut mem = make_memory("test", "keyword injection");
        mem.keywords = vec!["normal".into(), "'; DROP TABLE memories; --".into()];
        store.store(mem).unwrap();
        assert_eq!(store.count().unwrap(), 1);

        let results = store
            .search_by_keywords(&["'; DROP TABLE memories; --"], 10)
            .unwrap();
        let _ = results;
    }

    #[test]
    fn test_null_bytes_in_content() {
        let store = test_store();
        let mem = make_memory("test", "before\0after");
        store.store(mem.clone()).unwrap();
        let retrieved = store.get(&mem.id).unwrap().unwrap();
        assert!(retrieved.summary.contains("before"));
    }

    #[test]
    fn test_unicode_boundary_content() {
        let store = test_store();
        let unicode_topic = "\u{1F600}\u{1F4A9}\u{0000}";
        let mem = make_memory(unicode_topic, "emoji topic");
        store.store(mem.clone()).unwrap();
        let retrieved = store.get(&mem.id).unwrap().unwrap();
        assert!(retrieved.topic.starts_with('\u{1F600}'));
    }

    #[test]
    fn test_very_long_summary() {
        let store = test_store();
        let long_summary = "a".repeat(100_000);
        let mem = make_memory("test", &long_summary);
        store.store(mem.clone()).unwrap();
        let retrieved = store.get(&mem.id).unwrap().unwrap();
        assert_eq!(retrieved.summary.len(), 100_000);
    }

    #[test]
    fn test_empty_strings() {
        let store = test_store();
        let mem = make_memory("", "");
        store.store(mem.clone()).unwrap();
        let retrieved = store.get(&mem.id).unwrap().unwrap();
        assert_eq!(retrieved.topic, "");
        assert_eq!(retrieved.summary, "");
    }

    #[test]
    fn test_bulk_insert_100() {
        let store = test_store();
        for i in 0..100 {
            store
                .store(make_memory("bulk", &format!("memory number {i}")))
                .unwrap();
        }
        assert_eq!(store.count().unwrap(), 100);
        let by_topic = store.get_by_topic("bulk").unwrap();
        assert_eq!(by_topic.len(), 100);
    }

    #[test]
    fn test_fts_search_many_entries() {
        let store = test_store();
        for i in 0..50 {
            store
                .store(make_memory(
                    "lang",
                    &format!("programming language number {i}"),
                ))
                .unwrap();
        }
        store
            .store(make_memory(
                "unique",
                "Rust is a memory-safe systems language",
            ))
            .unwrap();

        let results = store.search_fts("memory-safe systems", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].topic, "unique");
    }

    #[test]
    fn test_decay_bulk() {
        let store = test_store();
        for i in 0..50 {
            let mut mem = make_memory("decay", &format!("entry {i}"));
            if i % 5 == 0 {
                mem.importance = Importance::Critical;
            }
            store.store(mem).unwrap();
        }
        // 10 critical, 40 non-critical
        let affected = store.apply_decay(0.9).unwrap();
        assert_eq!(affected, 40);
    }

    #[test]
    fn test_prune_leaves_important() {
        let store = test_store();
        for i in 0..20 {
            let mut mem = make_memory("prune", &format!("entry {i}"));
            mem.weight = if i < 10 { 0.01 } else { 0.5 };
            store.store(mem).unwrap();
        }
        let pruned = store.prune(0.1).unwrap();
        assert_eq!(pruned, 10);
        assert_eq!(store.count().unwrap(), 10);
    }

    #[test]
    fn test_many_topics_listing() {
        let store = test_store();
        for i in 0..30 {
            store
                .store(make_memory(&format!("topic-{i}"), &format!("content {i}")))
                .unwrap();
        }
        let topics = store.list_topics().unwrap();
        assert_eq!(topics.len(), 30);
    }

    #[test]
    fn test_consolidate_large_topic() {
        let store = test_store();
        for i in 0..25 {
            store
                .store(make_memory("big-topic", &format!("detail {i}")))
                .unwrap();
        }
        let consolidated = make_memory("big-topic", "consolidated summary of 25 entries");
        store.consolidate_topic("big-topic", consolidated).unwrap();
        let remaining = store.get_by_topic("big-topic").unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].summary.contains("consolidated"));
    }

    #[test]
    fn test_get_by_topic_returns_sorted_by_weight() {
        let store = test_store();
        let mut low = make_memory("ux", "low weight");
        low.weight = 0.3;
        store.store(low).unwrap();

        let mut high = make_memory("ux", "high weight");
        high.weight = 0.9;
        store.store(high).unwrap();

        let results = store.get_by_topic("ux").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].weight >= results[1].weight);
    }

    #[test]
    fn test_update_access_increments_correctly() {
        let store = test_store();
        let mem = make_memory("ux", "access counter");
        let id = mem.id.clone();
        store.store(mem).unwrap();

        for _ in 0..5 {
            store.update_access(&id).unwrap();
        }
        let retrieved = store.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.access_count, 5);
    }

    #[test]
    fn test_stats_on_empty_store() {
        let store = test_store();
        let stats = store.stats().unwrap();
        assert_eq!(stats.total_memories, 0);
        assert_eq!(stats.total_topics, 0);
        assert_eq!(stats.avg_weight, 0.0);
        assert!(stats.oldest_memory.is_none());
        assert!(stats.newest_memory.is_none());
    }

    #[test]
    fn test_double_delete_returns_not_found() {
        let store = test_store();
        let mem = make_memory("ux", "delete twice");
        let id = mem.id.clone();
        store.store(mem).unwrap();

        store.delete(&id).unwrap();
        let result = store.delete(&id);
        assert!(matches!(result, Err(IcmError::NotFound(_))));
    }

    #[test]
    fn test_update_syncs_embedding() {
        let store = test_store();
        let mut mem = make_memory("test", "before update");
        let id = mem.id.clone();
        store.store(mem.clone()).unwrap();

        // Initially no embedding
        assert!(store.get(&id).unwrap().unwrap().embedding.is_none());

        // Update with embedding
        mem.embedding = Some(vec![0.3; 384]);
        store.update(&mem).unwrap();

        let retrieved = store.get(&id).unwrap().unwrap();
        assert!(retrieved.embedding.is_some());

        // Should be findable via vector search
        let results = store.search_by_embedding(&vec![0.3; 384], 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, id);
    }

    #[test]
    fn perf_store_1000() {
        let store = test_store();
        let start = std::time::Instant::now();
        for i in 0..1000 {
            store
                .store(make_memory("perf", &format!("memory number {i}")))
                .unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 2000,
            "1000 stores took {}ms (max 2000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_store_with_embeddings_1000() {
        let store = test_store();
        let start = std::time::Instant::now();
        for i in 0..1000 {
            let mut mem = make_memory("perf", &format!("embedded memory {i}"));
            mem.embedding = Some(vec![0.1; 384]);
            store.store(mem).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 3000,
            "1000 stores+embedding took {}ms (max 3000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_fts_search_100() {
        let store = test_store();
        for i in 0..500 {
            store
                .store(make_memory(
                    "lang",
                    &format!("programming language {i} with features"),
                ))
                .unwrap();
        }
        let start = std::time::Instant::now();
        for _ in 0..100 {
            store
                .search_fts("programming language features", 10)
                .unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 1000,
            "100 FTS searches took {}ms (max 1000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_vector_search_100() {
        let store = test_store();
        for i in 0..500 {
            let mut mem = make_memory("vec", &format!("vector memory {i}"));
            let mut emb = vec![0.0; 384];
            emb[i % 384] = 1.0;
            mem.embedding = Some(emb);
            store.store(mem).unwrap();
        }
        let query = vec![0.5; 384];
        let start = std::time::Instant::now();
        for _ in 0..100 {
            store.search_by_embedding(&query, 10).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 5000,
            "100 vector searches took {}ms (max 5000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_hybrid_search_100() {
        let store = test_store();
        for i in 0..500 {
            let mut mem = make_memory("hybrid", &format!("hybrid searchable memory {i}"));
            mem.embedding = Some(vec![0.1; 384]);
            store.store(mem).unwrap();
        }
        let query_emb = vec![0.1; 384];
        let start = std::time::Instant::now();
        for _ in 0..100 {
            store
                .search_hybrid("hybrid searchable", &query_emb, 10)
                .unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 10000,
            "100 hybrid searches took {}ms (max 10000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_decay_1000() {
        let store = test_store();
        for i in 0..1000 {
            store
                .store(make_memory("decay", &format!("decayable {i}")))
                .unwrap();
        }
        let start = std::time::Instant::now();
        store.apply_decay(0.95).unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 500,
            "decay on 1000 memories took {}ms (max 500ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_get_by_id_1000() {
        let store = test_store();
        let mut ids = Vec::new();
        for i in 0..1000 {
            let mem = make_memory("get", &format!("lookup {i}"));
            let id = mem.id.clone();
            store.store(mem).unwrap();
            ids.push(id);
        }
        let start = std::time::Instant::now();
        for id in &ids {
            store.get(id).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 1000,
            "1000 gets took {}ms (max 1000ms)",
            elapsed.as_millis()
        );
    }

    // === Additional performance tests ===

    #[test]
    fn perf_search_fts_latency_with_1000_entries() {
        let store = test_store();
        for i in 0..1000 {
            store
                .store(make_memory(
                    &format!("topic-{}", i % 50),
                    &format!("detailed description about system component {i} with features and architecture"),
                ))
                .unwrap();
        }
        let start = std::time::Instant::now();
        for _ in 0..50 {
            let results = store
                .search_fts("system component architecture", 10)
                .unwrap();
            assert!(!results.is_empty());
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 2000,
            "50 FTS searches over 1000 entries took {}ms (max 2000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_sequential_store_operations_rapid() {
        let store = test_store();
        let start = std::time::Instant::now();
        // Simulate concurrent-like rapid sequential operations mixing stores, gets, searches
        for i in 0..500 {
            let mem = make_memory("rapid", &format!("rapid entry {i}"));
            let id = mem.id.clone();
            store.store(mem).unwrap();
            // Interleave reads
            if i % 5 == 0 {
                store.get(&id).unwrap();
            }
            // Interleave searches
            if i % 20 == 0 {
                store.search_fts("rapid entry", 5).unwrap();
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 3000,
            "500 mixed store/get/search ops took {}ms (max 3000ms)",
            elapsed.as_millis()
        );
        assert_eq!(store.count().unwrap(), 500);
    }

    #[test]
    fn perf_memoir_creation_and_concept_linking() {
        let store = test_store();
        let start = std::time::Instant::now();

        // Create 10 memoirs, each with 10 concepts and links between them
        for m in 0..10 {
            let m_id = store
                .create_memoir(make_memoir(&format!("perf-memoir-{m}")))
                .unwrap();
            let mut concept_ids = Vec::new();
            for c in 0..10 {
                let c_id = store
                    .add_concept(make_concept(
                        &m_id,
                        &format!("concept-{m}-{c}"),
                        &format!("Definition for concept {c} in memoir {m}"),
                    ))
                    .unwrap();
                concept_ids.push(c_id);
            }
            // Link each concept to the next one (chain)
            for w in concept_ids.windows(2) {
                store
                    .add_link(ConceptLink::new(
                        w[0].clone(),
                        w[1].clone(),
                        Relation::DependsOn,
                    ))
                    .unwrap();
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 3000,
            "10 memoirs x 10 concepts + links took {}ms (max 3000ms)",
            elapsed.as_millis()
        );

        // Verify structure
        let memoirs = store.list_memoirs().unwrap();
        assert_eq!(memoirs.len(), 10);
    }

    #[test]
    fn perf_neighborhood_bfs_large_graph() {
        let store = test_store();
        let m_id = store.create_memoir(make_memoir("large-graph")).unwrap();

        // Create a large graph: 50 concepts in a chain
        let mut concept_ids = Vec::new();
        for i in 0..50 {
            let c_id = store
                .add_concept(make_concept(
                    &m_id,
                    &format!("node-{i}"),
                    &format!("Graph node number {i}"),
                ))
                .unwrap();
            concept_ids.push(c_id);
        }
        // Chain: 0->1->2->...->49
        for w in concept_ids.windows(2) {
            store
                .add_link(ConceptLink::new(
                    w[0].clone(),
                    w[1].clone(),
                    Relation::DependsOn,
                ))
                .unwrap();
        }
        // Add some cross-links for complexity
        for i in (0..50).step_by(5) {
            if i + 10 < 50 {
                store
                    .add_link(ConceptLink::new(
                        concept_ids[i].clone(),
                        concept_ids[i + 10].clone(),
                        Relation::RelatedTo,
                    ))
                    .unwrap();
            }
        }

        let start = std::time::Instant::now();
        // BFS traversal at various depths
        for depth in 1..=5 {
            let (concepts, links) = store.get_neighborhood(&concept_ids[0], depth).unwrap();
            assert!(!concepts.is_empty());
            assert!(!links.is_empty());
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 2000,
            "BFS traversals (depth 1-5) on 50-node graph took {}ms (max 2000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_embedding_storage_batch() {
        let store = test_store();
        let start = std::time::Instant::now();
        for i in 0..500 {
            let mut mem = make_memory("embed-perf", &format!("embedding batch entry {i}"));
            let mut emb = vec![0.0f32; 384];
            // Vary embeddings so they're not all identical
            emb[i % 384] = 1.0;
            emb[(i * 7) % 384] = 0.5;
            mem.embedding = Some(emb);
            store.store(mem).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 3000,
            "500 stores with embeddings took {}ms (max 3000ms)",
            elapsed.as_millis()
        );

        // Now search
        let query = vec![0.5f32; 384];
        let search_start = std::time::Instant::now();
        for _ in 0..50 {
            let results = store.search_by_embedding(&query, 10).unwrap();
            assert!(!results.is_empty());
        }
        let search_elapsed = search_start.elapsed();
        assert!(
            search_elapsed.as_millis() < 3000,
            "50 vector searches over 500 entries took {}ms (max 3000ms)",
            search_elapsed.as_millis()
        );
    }

    #[test]
    fn perf_keyword_search_with_many_entries() {
        let store = test_store();
        for i in 0..1000 {
            let mut mem = make_memory(
                &format!("kw-topic-{}", i % 20),
                &format!("keyword searchable entry number {i}"),
            );
            mem.keywords = vec![
                format!("keyword-{}", i % 10),
                format!("category-{}", i % 5),
                "common".into(),
            ];
            store.store(mem).unwrap();
        }

        let start = std::time::Instant::now();
        for i in 0..50 {
            let results = store
                .search_by_keywords(&[&format!("keyword-{}", i % 10)], 10)
                .unwrap();
            assert!(!results.is_empty());
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 2000,
            "50 keyword searches over 1000 entries took {}ms (max 2000ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn perf_consolidate_large_topic_timing() {
        let store = test_store();
        for i in 0..100 {
            store
                .store(make_memory(
                    "consolidate-perf",
                    &format!("detail entry {i} with various information"),
                ))
                .unwrap();
        }
        let start = std::time::Instant::now();
        let consolidated = make_memory("consolidate-perf", "All 100 entries consolidated");
        store
            .consolidate_topic("consolidate-perf", consolidated)
            .unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 1000,
            "Consolidating 100 entries took {}ms (max 1000ms)",
            elapsed.as_millis()
        );
        assert_eq!(store.get_by_topic("consolidate-perf").unwrap().len(), 1);
    }

    #[test]
    fn perf_list_topics_many() {
        let store = test_store();
        // Create 200 distinct topics
        for i in 0..200 {
            store
                .store(make_memory(
                    &format!("distinct-topic-{i}"),
                    &format!("content for topic {i}"),
                ))
                .unwrap();
        }
        let start = std::time::Instant::now();
        for _ in 0..50 {
            let topics = store.list_topics().unwrap();
            assert_eq!(topics.len(), 200);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 1000,
            "50 list_topics calls over 200 topics took {}ms (max 1000ms)",
            elapsed.as_millis()
        );
    }

    // === FeedbackStore tests ===

    fn make_feedback(topic: &str, context: &str, predicted: &str, corrected: &str) -> Feedback {
        Feedback::new(
            topic.into(),
            context.into(),
            predicted.into(),
            corrected.into(),
            None,
            "test".into(),
        )
    }

    #[test]
    fn test_feedback_store_and_list() {
        let store = test_store();
        let fb = make_feedback("triage", "issue about crashes", "low", "high");
        let id = fb.id.clone();
        store.store_feedback(fb).unwrap();

        let results = store.list_feedback(None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
        assert_eq!(results[0].topic, "triage");
        assert_eq!(results[0].predicted, "low");
        assert_eq!(results[0].corrected, "high");
    }

    #[test]
    fn test_feedback_list_by_topic() {
        let store = test_store();
        store
            .store_feedback(make_feedback("triage", "ctx1", "a", "b"))
            .unwrap();
        store
            .store_feedback(make_feedback("pr-review", "ctx2", "c", "d"))
            .unwrap();

        let triage = store.list_feedback(Some("triage"), 10).unwrap();
        assert_eq!(triage.len(), 1);
        assert_eq!(triage[0].topic, "triage");

        let all = store.list_feedback(None, 10).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_feedback_search() {
        let store = test_store();
        store
            .store_feedback(make_feedback(
                "triage",
                "user reports memory leak",
                "low priority",
                "high priority",
            ))
            .unwrap();
        store
            .store_feedback(make_feedback(
                "triage",
                "build failure on CI",
                "feature",
                "bug",
            ))
            .unwrap();

        let results = store.search_feedback("memory leak", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].context.contains("memory leak"));
    }

    #[test]
    fn test_feedback_search_with_topic_filter() {
        let store = test_store();
        store
            .store_feedback(make_feedback("triage", "memory issue", "low", "high"))
            .unwrap();
        store
            .store_feedback(make_feedback("pr-review", "memory usage", "ok", "bad"))
            .unwrap();

        let results = store.search_feedback("memory", Some("triage"), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic, "triage");
    }

    #[test]
    fn test_feedback_increment_applied() {
        let store = test_store();
        let fb = make_feedback("triage", "ctx", "a", "b");
        let id = fb.id.clone();
        store.store_feedback(fb).unwrap();

        store.increment_applied(&id).unwrap();
        store.increment_applied(&id).unwrap();

        let results = store.list_feedback(None, 10).unwrap();
        assert_eq!(results[0].applied_count, 2);
    }

    #[test]
    fn test_feedback_increment_applied_not_found() {
        let store = test_store();
        let result = store.increment_applied("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_feedback_delete() {
        let store = test_store();
        let fb = make_feedback("triage", "ctx", "a", "b");
        let id = fb.id.clone();
        store.store_feedback(fb).unwrap();

        store.delete_feedback(&id).unwrap();
        let results = store.list_feedback(None, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_feedback_delete_not_found() {
        let store = test_store();
        let result = store.delete_feedback("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_feedback_stats() {
        let store = test_store();
        store
            .store_feedback(make_feedback("triage", "ctx1", "a", "b"))
            .unwrap();
        store
            .store_feedback(make_feedback("triage", "ctx2", "c", "d"))
            .unwrap();
        store
            .store_feedback(make_feedback("pr-review", "ctx3", "e", "f"))
            .unwrap();

        let fb = make_feedback("triage", "ctx4", "g", "h");
        let id = fb.id.clone();
        store.store_feedback(fb).unwrap();
        store.increment_applied(&id).unwrap();

        let stats = store.feedback_stats().unwrap();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.by_topic.len(), 2);
        assert_eq!(stats.by_topic[0].0, "triage");
        assert_eq!(stats.by_topic[0].1, 3);
        assert_eq!(stats.most_applied.len(), 1);
        assert_eq!(stats.most_applied[0].1, 1);
    }

    // === sanitize_fts_query tests ===

    #[test]
    fn test_sanitize_fts_empty() {
        assert_eq!(sanitize_fts_query(""), "");
        assert_eq!(sanitize_fts_query("   "), "");
    }

    #[test]
    fn test_sanitize_fts_special_chars() {
        // All FTS5 operators should be stripped
        assert_eq!(sanitize_fts_query("hello-world"), "\"hello\" \"world\"");
        assert_eq!(sanitize_fts_query("foo*bar"), "\"foo\" \"bar\"");
        assert_eq!(sanitize_fts_query("a:b"), "\"a\" \"b\"");
        assert_eq!(sanitize_fts_query("(test)"), "\"test\"");
        assert_eq!(sanitize_fts_query("x^y+z~w"), "\"x\" \"y\" \"z\" \"w\"");
    }

    #[test]
    fn test_sanitize_fts_quotes_stripped() {
        // Embedded quotes must be removed before wrapping in quotes
        assert_eq!(sanitize_fts_query("say \"hello\""), "\"say\" \"hello\"");
    }

    #[test]
    fn test_sanitize_fts_unicode() {
        assert_eq!(sanitize_fts_query("café résumé"), "\"café\" \"résumé\"");
        assert_eq!(sanitize_fts_query("日本語テスト"), "\"日本語テスト\"");
    }

    #[test]
    fn test_sanitize_fts_long_input_truncated() {
        let long = "a ".repeat(6000); // 12000 chars
        let result = sanitize_fts_query(&long);
        // Input is truncated to 10_000 chars, then tokens are capped at 100
        let token_count = result.split_whitespace().count();
        assert!(token_count <= 100);
    }

    #[test]
    fn test_sanitize_fts_many_tokens_capped() {
        // 200 tokens should be capped to 100
        let many_tokens: String = (0..200)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let result = sanitize_fts_query(&many_tokens);
        let token_count = result.split_whitespace().count();
        assert_eq!(token_count, 100);
    }

    // === search limit cap tests ===

    #[test]
    fn test_search_fts_limit_capped() {
        let store = test_store();
        // Store a memory so search has something to find
        store.store(make_memory("test", "hello world")).unwrap();

        // Even with a huge limit, it should not error (capped internally)
        let results = store.search_fts("hello", 999_999).unwrap();
        assert!(results.len() <= 100);
    }

    #[test]
    fn test_search_by_keywords_limit_capped() {
        let store = test_store();
        let mut mem = make_memory("test", "keyword search test");
        mem.keywords = vec!["findme".into()];
        store.store(mem).unwrap();

        let results = store.search_by_keywords(&["findme"], 999_999).unwrap();
        assert!(results.len() <= 100);
    }

    // === Additional MemoryStore coverage ===

    #[test]
    fn test_search_fts_empty_query() {
        let store = test_store();
        store.store(make_memory("topic", "hello world")).unwrap();
        let results = store.search_fts("", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_by_keywords_empty() {
        let store = test_store();
        let results = store.search_by_keywords(&[], 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_update_nonexistent_memory() {
        let store = test_store();
        let mut mem = make_memory("t", "s");
        mem.id = "nonexistent-id".to_string();
        let result = store.update(&mem);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_nonexistent_memory() {
        let store = test_store();
        let result = store.delete("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_update_access() {
        let store = test_store();
        let id1 = store.store(make_memory("t", "one")).unwrap();
        let id2 = store.store(make_memory("t", "two")).unwrap();
        store.batch_update_access(&[&id1, &id2]).unwrap();
        let m1 = store.get(&id1).unwrap().unwrap();
        let m2 = store.get(&id2).unwrap().unwrap();
        assert_eq!(m1.access_count, 1);
        assert_eq!(m2.access_count, 1);
    }

    #[test]
    fn test_auto_consolidate_below_threshold() {
        let store = test_store();
        store.store(make_memory("t", "one")).unwrap();
        store.store(make_memory("t", "two")).unwrap();
        // Threshold is 10, so no consolidation
        let result = store.auto_consolidate("t", 10).unwrap();
        assert!(!result);
        assert_eq!(store.count_by_topic("t").unwrap(), 2);
    }

    #[test]
    fn test_auto_consolidate_above_threshold() {
        let store = test_store();
        for i in 0..12 {
            store
                .store(make_memory("bulk", &format!("entry {i}")))
                .unwrap();
        }
        let result = store.auto_consolidate("bulk", 10).unwrap();
        assert!(result);
        assert_eq!(store.count_by_topic("bulk").unwrap(), 1);
    }

    #[test]
    fn test_auto_consolidate_with_embedder_attaches_embedding() {
        // Audit M2/AC2: the embedder-aware variant must produce a
        // consolidated memory that is recall-ready (embedding != None).
        struct StubEmbedder;
        impl icm_core::Embedder for StubEmbedder {
            fn embed(&self, _text: &str) -> IcmResult<Vec<f32>> {
                Ok(vec![0.42; icm_core::DEFAULT_EMBEDDING_DIMS])
            }
            fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
                Ok(texts
                    .iter()
                    .map(|_| vec![0.42; icm_core::DEFAULT_EMBEDDING_DIMS])
                    .collect())
            }
            fn dimensions(&self) -> usize {
                icm_core::DEFAULT_EMBEDDING_DIMS
            }
        }
        let store = test_store();
        for i in 0..11 {
            store
                .store(make_memory("rolled", &format!("fact {i}")))
                .unwrap();
        }
        let stub = StubEmbedder;
        let did = store
            .auto_consolidate_with_embedder("rolled", 10, Some(&stub))
            .unwrap();
        assert!(did);
        let consolidated = store.get_by_topic("rolled").unwrap();
        assert_eq!(consolidated.len(), 1);
        let embedding = consolidated[0]
            .embedding
            .as_ref()
            .expect("consolidated memory must have an embedding");
        assert_eq!(embedding.len(), icm_core::DEFAULT_EMBEDDING_DIMS);
        assert!((embedding[0] - 0.42).abs() < 1e-6);
    }

    #[test]
    fn test_apply_decay_with_aggressive_factor() {
        let store = test_store();
        store.store(make_memory("t", "decayable")).unwrap();
        let affected = store.apply_decay(0.5).unwrap();
        assert!(affected > 0);
        let mems = store.get_by_topic("t").unwrap();
        assert!(mems[0].weight < 1.0);
    }

    #[test]
    fn test_prune_low_weight() {
        let store = test_store();
        store.store(make_memory("t", "will be pruned")).unwrap();
        // Apply aggressive decay
        store.apply_decay(0.01).unwrap();
        let pruned = store.prune(0.5).unwrap();
        assert!(pruned > 0);
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn test_list_topics_multiple() {
        let store = test_store();
        store.store(make_memory("alpha", "a")).unwrap();
        store.store(make_memory("beta", "b")).unwrap();
        store.store(make_memory("alpha", "c")).unwrap();
        let topics = store.list_topics().unwrap();
        assert_eq!(topics.len(), 2);
    }

    #[test]
    fn test_stats_multi_topic() {
        let store = test_store();
        store.store(make_memory("t1", "one")).unwrap();
        store.store(make_memory("t2", "two")).unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.total_memories, 2);
        assert_eq!(stats.total_topics, 2);
    }

    #[test]
    fn test_get_by_topic_prefix() {
        let store = test_store();
        store
            .store(make_memory("project:web", "web stuff"))
            .unwrap();
        store
            .store(make_memory("project:api", "api stuff"))
            .unwrap();
        store.store(make_memory("other", "unrelated")).unwrap();
        let results = store.get_by_topic_prefix("project:*").unwrap();
        assert_eq!(results.len(), 2);
    }

    // ── expand_with_neighbors ────────────────────────────────────────────

    #[test]
    fn test_expand_with_neighbors_brings_hop_1() {
        let store = test_store();
        // Create 3 memories. m1 is a direct query hit; m2 and m3 are
        // related to m1 via related_ids; m3 is unrelated.
        let mut m1 = make_memory("decisions", "primary hit");
        let mut m2 = make_memory("decisions", "related neighbor");
        let m3 = make_memory("unrelated", "far away");

        // Set up the edges before storing.
        m1.related_ids.push(m2.id.clone());
        m2.related_ids.push(m1.id.clone());

        let id1 = store.store(m1.clone()).unwrap();
        let _id2 = store.store(m2.clone()).unwrap();
        let _id3 = store.store(m3.clone()).unwrap();

        let m1_full = store.get(&id1).unwrap().unwrap();
        let initial = vec![(m1_full, 0.9_f32)];

        let expanded = store.expand_with_neighbors(&initial, 5, 0.5, 10).unwrap();

        assert_eq!(expanded.len(), 2, "primary + 1 neighbor");
        assert!(expanded.iter().any(|(m, _)| m.id == m1.id));
        assert!(
            expanded.iter().any(|(m, _)| m.id == m2.id),
            "neighbor should be pulled in"
        );
        assert!(
            expanded.iter().all(|(m, _)| m.id != m3.id),
            "unrelated memory must not be pulled in: {expanded:?}"
        );
    }

    #[test]
    fn test_expand_with_neighbors_dedupes_initial() {
        let store = test_store();
        let mut m1 = make_memory("t", "hit 1");
        let mut m2 = make_memory("t", "hit 2");
        m1.related_ids.push(m2.id.clone());
        m2.related_ids.push(m1.id.clone());

        let id1 = store.store(m1.clone()).unwrap();
        let id2 = store.store(m2.clone()).unwrap();

        // Both already in the initial set — no neighbor to add.
        let m1_full = store.get(&id1).unwrap().unwrap();
        let m2_full = store.get(&id2).unwrap().unwrap();
        let initial = vec![(m1_full, 0.9_f32), (m2_full, 0.85_f32)];

        let expanded = store.expand_with_neighbors(&initial, 5, 0.5, 10).unwrap();
        assert_eq!(expanded.len(), 2, "no duplicates when both already present");
    }

    #[test]
    fn test_expand_with_neighbors_respects_max_neighbors() {
        let store = test_store();
        // m1 has 5 neighbors. Cap max_neighbors at 2.
        let mut m1 = make_memory("t", "hub");
        let n1 = make_memory("t", "neighbor 1");
        let n2 = make_memory("t", "neighbor 2");
        let n3 = make_memory("t", "neighbor 3");
        let n4 = make_memory("t", "neighbor 4");
        let n5 = make_memory("t", "neighbor 5");
        m1.related_ids.extend([
            n1.id.clone(),
            n2.id.clone(),
            n3.id.clone(),
            n4.id.clone(),
            n5.id.clone(),
        ]);

        let id1 = store.store(m1.clone()).unwrap();
        for n in [&n1, &n2, &n3, &n4, &n5] {
            store.store(n.clone()).unwrap();
        }

        let m1_full = store.get(&id1).unwrap().unwrap();
        let initial = vec![(m1_full, 0.9_f32)];

        let expanded = store.expand_with_neighbors(&initial, 2, 0.5, 10).unwrap();
        // 1 primary + 2 neighbors = 3.
        assert_eq!(expanded.len(), 3);
    }

    #[test]
    fn test_expand_with_neighbors_applies_discount() {
        let store = test_store();
        let mut m1 = make_memory("t", "primary");
        let m2 = make_memory("t", "neighbor");
        m1.related_ids.push(m2.id.clone());

        let id1 = store.store(m1.clone()).unwrap();
        store.store(m2.clone()).unwrap();

        let m1_full = store.get(&id1).unwrap().unwrap();
        let initial = vec![(m1_full, 0.9_f32)];

        let expanded = store.expand_with_neighbors(&initial, 5, 0.5, 10).unwrap();

        // Find neighbor score: should be 0.9 * 0.5 = 0.45
        let neighbor_score = expanded
            .iter()
            .find(|(m, _)| m.id == m2.id)
            .map(|(_, s)| *s)
            .unwrap();
        assert!(
            (neighbor_score - 0.45).abs() < 1e-5,
            "neighbor discount wrong: {neighbor_score}"
        );
    }

    #[test]
    fn test_expand_with_neighbors_respects_max_total() {
        let store = test_store();
        // 3 primaries + 3 neighbors, but max_total=4 caps output.
        let mut m1 = make_memory("t", "p1");
        let mut m2 = make_memory("t", "p2");
        let mut m3 = make_memory("t", "p3");
        let n1 = make_memory("t", "n1");
        let n2 = make_memory("t", "n2");
        let n3 = make_memory("t", "n3");
        m1.related_ids.push(n1.id.clone());
        m2.related_ids.push(n2.id.clone());
        m3.related_ids.push(n3.id.clone());

        let id1 = store.store(m1.clone()).unwrap();
        let id2 = store.store(m2.clone()).unwrap();
        let id3 = store.store(m3.clone()).unwrap();
        store.store(n1).unwrap();
        store.store(n2).unwrap();
        store.store(n3).unwrap();

        let initial = vec![
            (store.get(&id1).unwrap().unwrap(), 0.9),
            (store.get(&id2).unwrap().unwrap(), 0.85),
            (store.get(&id3).unwrap().unwrap(), 0.8),
        ];

        let expanded = store.expand_with_neighbors(&initial, 5, 0.5, 4).unwrap();
        assert_eq!(expanded.len(), 4, "must respect max_total cap");
        // Top scorer remains first.
        assert!((expanded[0].1 - 0.9).abs() < 1e-5);
    }

    #[test]
    fn test_expand_with_neighbors_empty_initial_passthrough() {
        let store = test_store();
        let expanded = store.expand_with_neighbors(&[], 5, 0.5, 10).unwrap();
        assert!(expanded.is_empty());
    }

    #[test]
    fn test_expand_with_neighbors_zero_neighbors_disables() {
        let store = test_store();
        let mut m1 = make_memory("t", "primary");
        let m2 = make_memory("t", "would-be neighbor");
        m1.related_ids.push(m2.id.clone());

        let id1 = store.store(m1.clone()).unwrap();
        store.store(m2).unwrap();

        let initial = vec![(store.get(&id1).unwrap().unwrap(), 0.9)];
        let expanded = store.expand_with_neighbors(&initial, 0, 0.5, 10).unwrap();
        assert_eq!(expanded.len(), 1, "max_neighbors=0 disables expansion");
    }

    #[test]
    fn test_expand_with_neighbors_skips_missing_targets() {
        let store = test_store();
        // m1 points to a ghost id that no longer exists (e.g., deleted).
        let mut m1 = make_memory("t", "has ghost link");
        m1.related_ids.push("01GHOSTID".into());
        let id1 = store.store(m1.clone()).unwrap();

        let initial = vec![(store.get(&id1).unwrap().unwrap(), 0.9)];
        let expanded = store.expand_with_neighbors(&initial, 5, 0.5, 10).unwrap();
        assert_eq!(expanded.len(), 1, "ghost link must be silently skipped");
    }

    // ── get_many (batched fetch) ─────────────────────────────────────────

    #[test]
    fn test_get_many_returns_requested_ids() {
        let store = test_store();
        let m1 = make_memory("t", "first");
        let m2 = make_memory("t", "second");
        let m3 = make_memory("t", "third");
        let id1 = store.store(m1.clone()).unwrap();
        let id2 = store.store(m2.clone()).unwrap();
        store.store(m3).unwrap();

        let got = store.get_many(&[id1.as_str(), id2.as_str()]).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains_key(&id1));
        assert!(got.contains_key(&id2));
    }

    #[test]
    fn test_get_many_empty_input_returns_empty() {
        let store = test_store();
        let got = store.get_many(&[]).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn test_get_many_missing_ids_silently_dropped() {
        let store = test_store();
        let m1 = make_memory("t", "real");
        let id1 = store.store(m1).unwrap();

        let got = store.get_many(&[id1.as_str(), "01NONEXISTENT"]).unwrap();
        assert_eq!(got.len(), 1);
        assert!(got.contains_key(&id1));
    }

    #[test]
    fn test_get_many_dedupes_input() {
        let store = test_store();
        let m1 = make_memory("t", "only");
        let id1 = store.store(m1).unwrap();

        // Same id three times — must not blow up the IN clause.
        let got = store
            .get_many(&[id1.as_str(), id1.as_str(), id1.as_str()])
            .unwrap();
        assert_eq!(got.len(), 1);
    }

    // ── LRU cache invalidation ────────────────────────────────────────────

    #[test]
    fn test_cache_serves_after_first_get() {
        let store = test_store();
        let m = make_memory("t", "original");
        let id = store.store(m).unwrap();

        // Warm the cache.
        let first = store.get(&id).unwrap().unwrap();
        assert_eq!(first.summary, "original");

        // Mutate the row out-of-band so a stale cache hit would show.
        store
            .conn
            .execute(
                "UPDATE memories SET summary = 'mutated' WHERE id = ?1",
                params![id],
            )
            .unwrap();

        // Cache is unaware of the raw SQL write, so we should still
        // see "original" — that's the proof the cache is serving reads.
        let cached = store.get(&id).unwrap().unwrap();
        assert_eq!(cached.summary, "original", "cache must serve hot reads");
    }

    #[test]
    fn test_update_invalidates_cache() {
        let store = test_store();
        let m = make_memory("t", "v1");
        let id = store.store(m).unwrap();

        // Warm cache.
        let _ = store.get(&id).unwrap();

        // Proper update through the trait flushes the cache entry.
        let mut updated = store.get(&id).unwrap().unwrap();
        updated.summary = "v2".into();
        store.update(&updated).unwrap();

        let after = store.get(&id).unwrap().unwrap();
        assert_eq!(after.summary, "v2");
    }

    #[test]
    fn test_delete_invalidates_cache() {
        let store = test_store();
        let m = make_memory("t", "doomed");
        let id = store.store(m).unwrap();

        // Warm the cache, then delete.
        let _ = store.get(&id).unwrap();
        store.delete(&id).unwrap();

        let after = store.get(&id).unwrap();
        assert!(after.is_none(), "deleted memory must not survive in cache");
    }

    #[test]
    fn test_apply_decay_clears_cache() {
        let store = test_store();
        let m1 = make_memory("t", "a");
        let m2 = make_memory("t", "b");
        let id1 = store.store(m1).unwrap();
        let id2 = store.store(m2).unwrap();

        // Warm cache for both.
        let before1 = store.get(&id1).unwrap().unwrap().weight;
        let _ = store.get(&id2).unwrap();

        store.apply_decay(0.5).unwrap();

        // After decay, cache must have been wiped, so the next read
        // returns the decayed weight from disk.
        let after1 = store.get(&id1).unwrap().unwrap().weight;
        assert!(
            after1 < before1,
            "post-decay weight should reflect DB, not stale cache (before={before1}, after={after1})"
        );
    }

    #[test]
    fn test_get_many_uses_cache_for_warm_ids() {
        let store = test_store();
        let m = make_memory("t", "warm");
        let id = store.store(m).unwrap();

        // Warm the cache via single get.
        let _ = store.get(&id).unwrap();

        // Out-of-band mutate — cached value should still be served by
        // get_many for this id.
        store
            .conn
            .execute(
                "UPDATE memories SET summary = 'mutated' WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let got = store.get_many(&[id.as_str()]).unwrap();
        assert_eq!(got.get(&id).unwrap().summary, "warm");
    }

    #[test]
    fn test_store_is_atomic() {
        let store = test_store();
        let mut mem = make_memory("atomic", "test atomicity");
        mem.embedding = Some(vec![0.1; 384]);
        let id = mem.id.clone();

        store.store(mem).unwrap();

        // Verify main table has the row
        let retrieved = store.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.summary, "test atomicity");

        // Verify vec_memories also has the row
        let vec_count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM vec_memories WHERE memory_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(vec_count, 1);
    }

    #[test]
    fn test_busy_timeout_pragma() {
        let store = test_store();
        let timeout: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    #[test]
    fn test_fts_sanitize_utf8_safe() {
        // Build a string with multibyte chars near the 10k boundary.
        // Each emoji is 4 bytes. Fill up to just past 10_000 bytes.
        let base = "a".repeat(9_998);
        // Add a 4-byte emoji that straddles the 10_000 boundary
        let input = format!("{base}\u{1F600}\u{1F600}"); // 9998 + 4 + 4 = 10006 bytes
        assert!(input.len() > 10_000);

        // This should not panic (the old code could split a UTF-8 char)
        let result = sanitize_fts_query(&input);
        // The result should be valid UTF-8 (it's a String, so it is by construction)
        assert!(!result.is_empty());
        // The truncated input should not contain partial emoji
        // (9998 + 4 = 10002 > 10000, so the emoji at 9998 is excluded; end = 9998)
        // Result should just be the 'a' tokens
    }

    #[test]
    fn test_forget_topic() {
        let store = test_store();

        // Create 3 memories in topic "ephemeral"
        for i in 0..3 {
            let m = make_memory("ephemeral", &format!("item {i}"));
            store.store(m).unwrap();
        }

        // Verify they exist
        let before = store.get_by_topic("ephemeral").unwrap();
        assert_eq!(before.len(), 3);

        // Delete all memories in the topic
        for m in &before {
            store.delete(&m.id).unwrap();
        }

        // Verify 0 remain
        let after = store.get_by_topic("ephemeral").unwrap();
        assert!(after.is_empty());
    }

    // === TranscriptStore tests ===

    #[test]
    fn test_transcript_create_session_and_record() {
        let store = test_store();
        let sid = store
            .create_session("claude-code", Some("proj"), None)
            .unwrap();
        assert!(!sid.is_empty());

        let mid = store
            .record_message(&sid, Role::User, "hello world", None, None, None)
            .unwrap();
        assert!(!mid.is_empty());

        let msgs = store.list_session_messages(&sid, 10, 0).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello world");
        assert_eq!(msgs[0].role, Role::User);
    }

    #[test]
    fn test_transcript_record_into_missing_session_fails() {
        let store = test_store();
        let err = store
            .record_message("nonexistent", Role::User, "hi", None, None, None)
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("session"));
    }

    #[test]
    fn test_transcript_search_fts5_boolean_and_phrase() {
        let store = test_store();
        let sid = store
            .create_session("cli", Some("db-debate"), None)
            .unwrap();
        store
            .record_message(
                &sid,
                Role::Assistant,
                "Postgres 16 supports JSONB and BRIN indexes natively.",
                None,
                None,
                None,
            )
            .unwrap();
        store
            .record_message(
                &sid,
                Role::Assistant,
                "MySQL lacks BRIN; its JSON type is stored differently.",
                None,
                None,
                None,
            )
            .unwrap();
        store
            .record_message(&sid, Role::User, "Et SQLite ?", None, None, None)
            .unwrap();

        // Boolean OR
        let hits = store
            .search_transcripts("postgres OR mysql", None, None, 10)
            .unwrap();
        assert_eq!(hits.len(), 2);

        // Exact phrase
        let phrase_hits = store
            .search_transcripts("\"BRIN indexes\"", None, None, 10)
            .unwrap();
        assert_eq!(phrase_hits.len(), 1);
        assert!(phrase_hits[0].message.content.contains("Postgres"));
    }

    #[test]
    fn test_transcript_search_scoped_by_session_and_project() {
        let store = test_store();
        let s1 = store.create_session("cli", Some("alpha"), None).unwrap();
        let s2 = store.create_session("cli", Some("beta"), None).unwrap();
        store
            .record_message(&s1, Role::User, "alpha wants postgres", None, None, None)
            .unwrap();
        store
            .record_message(&s2, Role::User, "beta wants postgres", None, None, None)
            .unwrap();

        // Global search returns both
        let all = store
            .search_transcripts("postgres", None, None, 10)
            .unwrap();
        assert_eq!(all.len(), 2);

        // Session filter
        let only_s1 = store
            .search_transcripts("postgres", Some(&s1), None, 10)
            .unwrap();
        assert_eq!(only_s1.len(), 1);
        assert_eq!(only_s1[0].message.session_id, s1);

        // Project filter
        let only_beta = store
            .search_transcripts("postgres", None, Some("beta"), 10)
            .unwrap();
        assert_eq!(only_beta.len(), 1);
        assert_eq!(only_beta[0].session.project.as_deref(), Some("beta"));
    }

    #[test]
    fn test_transcript_stats_breakdown() {
        let store = test_store();
        let s = store.create_session("claude-code", None, None).unwrap();
        store
            .record_message(&s, Role::User, "q", None, None, None)
            .unwrap();
        store
            .record_message(&s, Role::Assistant, "a", None, None, None)
            .unwrap();
        store
            .record_message(&s, Role::Tool, "{}", Some("Bash"), Some(10), None)
            .unwrap();

        let stats = store.transcript_stats().unwrap();
        assert_eq!(stats.total_sessions, 1);
        assert_eq!(stats.total_messages, 3);
        assert!(stats.total_bytes > 0);
        assert_eq!(stats.by_role.len(), 3);
        assert!(stats.by_agent.iter().any(|(a, _)| a == "claude-code"));
        assert_eq!(stats.top_sessions.len(), 1);
        assert_eq!(stats.top_sessions[0].1, 3);
    }

    #[test]
    fn test_transcript_forget_cascade_deletes_messages() {
        let store = test_store();
        let s = store.create_session("cli", None, None).unwrap();
        for i in 0..5 {
            store
                .record_message(&s, Role::User, &format!("msg {i}"), None, None, None)
                .unwrap();
        }

        store.forget_session(&s).unwrap();

        assert!(store.get_session(&s).unwrap().is_none());
        let msgs = store.list_session_messages(&s, 100, 0).unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_transcript_list_sessions_sorted_by_updated() {
        let store = test_store();
        let a = store.create_session("cli", Some("p"), None).unwrap();
        let b = store.create_session("cli", Some("p"), None).unwrap();
        // Bump `a` by recording a message (updates its updated_at)
        store
            .record_message(&a, Role::User, "bump", None, None, None)
            .unwrap();

        let list = store.list_sessions(Some("p"), 10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, a); // most recently updated first
        assert_eq!(list[1].id, b);
    }

    #[test]
    fn test_transcript_messages_chronological() {
        let store = test_store();
        let s = store.create_session("cli", None, None).unwrap();
        let ids: Vec<_> = (0..3)
            .map(|i| {
                store
                    .record_message(&s, Role::User, &format!("{i}"), None, None, None)
                    .unwrap()
            })
            .collect();

        let msgs = store.list_session_messages(&s, 10, 0).unwrap();
        let got: Vec<_> = msgs.iter().map(|m| m.id.clone()).collect();
        assert_eq!(got, ids);
    }
}
