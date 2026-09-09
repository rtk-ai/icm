use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Mutex, Once};

use chrono::{DateTime, Utc};
use lru::LruCache;
use rusqlite::{Connection, OptionalExtension, ffi::sqlite3_auto_extension, params};
use sha2::{Digest, Sha256};
use zerocopy::IntoBytes;

use icm_core::{
    Concept, ConceptLink, Embedder, Fact, FactsStats, FactsStore, Feedback, FeedbackStats,
    FeedbackStore, IcmError, IcmResult, Importance, Label, Memoir, MemoirStats, MemoirStore,
    Memory, MemorySource, MemoryStore, Message, PatternCluster, Relation, Role, Session,
    StoreStats, TopicHealth, TranscriptHit, TranscriptStats, TranscriptStore,
};

pub use crate::common::{
    CodeArea, ConsolidationJob, HookEvent, HookEventInsert, HookStatsRow, PendingRow,
};
use crate::schema::init_db_with_dims;
pub struct SqliteStore {
    conn: Connection,
    cache: Mutex<LruCache<String, Memory>>,
    /// `true` when opened through [`Self::open_readonly`]. Read-like
    /// methods that would otherwise dirty the DB (auto-decay,
    /// `update_access`) check this and skip silently; mutation methods
    /// (`store`, `update`, `delete`, etc.) check this and return
    /// `IcmError::ReadOnly`. Issue #263.
    readonly: bool,
}
impl SqliteStore {
    pub fn new(path: &Path) -> IcmResult<Self> {
        Self::with_dims(path, icm_core::DEFAULT_EMBEDDING_DIMS)
    }

    /// Open an existing database in read-only mode (issue #263).
    ///
    /// Differences vs [`Self::with_dims`]:
    /// - The parent directory is NOT created.
    /// - The connection is opened with `SQLITE_OPEN_READ_ONLY` — SQLite
    ///   itself refuses any DDL/DML that the application might miss.
    /// - No `PRAGMA journal_mode=WAL` (WAL requires writable access).
    /// - No `init_db_with_dims` (schema migration would mutate the DB).
    ///
    /// Returns an error if the file is absent (caller may want to fall
    /// through to writable mode then). Use [`std::path::Path::exists`]
    /// at the call site if you need a missing-DB fast path.
    pub fn open_readonly(path: &Path) -> IcmResult<Self> {
        ensure_sqlite_vec();
        if !path.exists() {
            return Err(IcmError::NotFound(format!(
                "database not found at {}",
                path.display()
            )));
        }
        let conn = open_readonly_connection(path)?;
        // foreign_keys is a no-op for reads; busy_timeout is still useful
        // when another writer holds the file.
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=30000;")
            .map_err(db_err)?;
        Ok(Self {
            conn,
            cache: Mutex::new(new_cache()),
            readonly: true,
        })
    }

    /// Open an existing database for maintenance — integrity check and
    /// repair (issue #313).
    ///
    /// Writable (so `REINDEX` and FTS `'rebuild'` can run) but, unlike
    /// [`Self::with_dims`], it deliberately does NOT:
    /// - run `init_db_with_dims` — schema migration would fail on, or mutate,
    ///   a corrupt DB before it can even be inspected;
    /// - switch `journal_mode` — a damaged file's on-disk format is left
    ///   exactly as found so recovery reasons about the real state.
    ///
    /// Returns [`IcmError::NotFound`] when the file is absent.
    pub fn open_maintenance(path: &Path) -> IcmResult<Self> {
        ensure_sqlite_vec();
        if !path.exists() {
            return Err(IcmError::NotFound(format!(
                "database not found at {}",
                path.display()
            )));
        }
        let conn = Connection::open(path)
            .map_err(|e| IcmError::Database(format!("cannot open database: {e}")))?;
        conn.execute_batch("PRAGMA busy_timeout=30000;")
            .map_err(db_err)?;
        Ok(Self {
            conn,
            cache: Mutex::new(new_cache()),
            readonly: false,
        })
    }

    /// True when the store was opened read-only (issue #263). Read-like
    /// methods skip side-effect mutations; write methods return
    /// `IcmError::ReadOnly`.
    #[must_use]
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Peek `icm_metadata.embedding_dims` without running any schema
    /// migration. Returns `Ok(None)` when the DB file is absent, the
    /// metadata table doesn't exist (legacy DB), or the row is missing.
    ///
    /// Use this *before* calling [`Self::with_dims`] when running in a
    /// mode that must not trigger a destructive vector recreate — most
    /// notably the `--no-embeddings` path (issue #267): if the caller
    /// has no embedder loaded, `with_dims` would otherwise fall back to
    /// `DEFAULT_EMBEDDING_DIMS`, mismatch the stored value, and silently
    /// DROP `vec_memories` while NULL-ing every `memories.embedding`.
    pub fn read_stored_embedding_dims(path: &Path) -> IcmResult<Option<usize>> {
        if !path.exists() {
            return Ok(None);
        }
        // Open strictly immutable so this helper survives a `chmod -w`
        // sandbox (issue #263 interaction). `SQLITE_OPEN_READ_ONLY`
        // alone is NOT enough — SQLite still tries to create/update
        // the `-shm` / `-wal` companion files for any WAL-mode DB,
        // which fails when the parent directory is non-writable.
        // The `immutable=1` URI flag tells SQLite the file will not
        // change during the connection's lifetime and stops it from
        // touching WAL infrastructure entirely. This is a one-shot probe
        // (not a long-lived connection), so the staleness that #319 fixes
        // for `open_readonly` does not apply here.
        let conn = open_readonly_uri(path, true)?;
        // Probe for the metadata table — legacy DBs predate it.
        let has_table: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master
                 WHERE type = 'table' AND name = 'icm_metadata'",
                [],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if !has_table {
            return Ok(None);
        }
        let row: Option<String> = conn
            .query_row(
                "SELECT value FROM icm_metadata WHERE key = 'embedding_dims'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(row.and_then(|s| s.parse().ok()))
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
        // Schema/PRAGMA setup races with other processes opening the same
        // brand-new DB simultaneously (found via real concurrent testing:
        // 10 processes opening one fresh DB, several hung, others errored,
        // zero succeeded). Both the WAL-mode switch (needs a brief
        // exclusive lock to convert a fresh file — busy_timeout must be
        // set first in the same batch, or this statement itself has no
        // timeout active yet) and init_db_with_dims's schema creation
        // (BEGIN IMMEDIATE-wrapped in schema.rs, but SQLite's FTS5
        // virtual-table module can still surface a transient error on the
        // loser even so) are retried together here: whatever the winner
        // already committed, a fresh attempt's PRAGMA + existence checks
        // correctly see and no-op past it. Jittered, not just linear,
        // backoff: a fixed schedule lets many racing processes retry in
        // near-lockstep and collide again and again.
        let mut last_err = None;
        for attempt in 0..40u32 {
            if attempt > 0 {
                let base_ms = (attempt as u64).min(20) * 15;
                let jitter_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| u64::from(d.subsec_nanos()) % 40)
                    .unwrap_or(0);
                std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter_ms));
            }
            // A short busy_timeout during this retry loop, not the normal
            // 30s: 30s is meant to tolerate *ordinary* write contention
            // during real use (e.g. a hook write racing a consolidate),
            // but stacked with up to 40 outer attempts here it turns into
            // a potentially multi-minute worst case under real multi-
            // process contention (measured: several real `icm` processes
            // hung well past 60s with the 30s inner timeout) — the outer
            // jittered loop is what actually provides the robustness here,
            // so the inner SQLite-level wait only needs to be long enough
            // to smooth over a single competing transaction, not to be a
            // retry mechanism in its own right. Restored to 30s below once
            // the schema is confirmed present.
            let attempt_result = conn
                .execute_batch(
                    "PRAGMA busy_timeout=1000; PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;",
                )
                .map_err(db_err)
                .and_then(|()| init_db_with_dims(&conn, embedding_dims));
            match attempt_result {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    let transient = msg.contains("vtable constructor failed")
                        || msg.contains("already exists")
                        || msg.contains("database is locked")
                        || msg.contains("database is busy");
                    last_err = Some(e);
                    if !transient {
                        break;
                    }
                }
            }
        }
        if let Some(e) = last_err {
            return Err(e);
        }
        conn.execute_batch("PRAGMA busy_timeout=30000;")
            .map_err(db_err)?;

        Ok(Self {
            conn,
            cache: Mutex::new(new_cache()),
            readonly: false,
        })
    }

    pub fn in_memory() -> IcmResult<Self> {
        Self::in_memory_with_dims(icm_core::DEFAULT_EMBEDDING_DIMS)
    }

    /// Open an in-memory store with a specific embedding dimension.
    /// Useful for tests that exercise the dim-migration / dim-drift paths.
    pub fn in_memory_with_dims(embedding_dims: usize) -> IcmResult<Self> {
        ensure_sqlite_vec();
        let conn = Connection::open_in_memory()
            .map_err(|e| IcmError::Database(format!("cannot open in-memory db: {e}")))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=30000;")
            .map_err(db_err)?;
        init_db_with_dims(&conn, embedding_dims)?;
        Ok(Self {
            conn,
            cache: Mutex::new(new_cache()),
            readonly: false,
        })
    }
}

impl SqliteStore {
    /// Insert a memory into the database without transaction management.
    /// Callers are responsible for wrapping this in a transaction.
    ///
    /// Dedup contract: an INSERT that collides with an existing memory on
    /// `(topic, summary_hash)` is silently ignored, and the **existing**
    /// row's id is returned. The caller's `memory.id` is forgotten in
    /// that case. This keeps `store(...)` idempotent: writing the same
    /// fact 100× ends up with one row, not 100.
    fn store_inner(&self, memory: &Memory) -> IcmResult<String> {
        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);
        let emb_blob = memory.embedding.as_deref().map(embedding_to_blob);
        let hash = summary_hash(&memory.topic, &memory.summary);

        let inserted = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO memories (id, created_at, updated_at, last_accessed, access_count, weight,
                 topic, summary, raw_excerpt, keywords,
                 importance, source_type, source_data, related_ids, embedding, summary_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                    hash,
                ],
            )
            .map_err(db_err)?;

        if inserted == 0 {
            // Dedup hit: a row with the same (topic, summary_hash)
            // already exists. Audit #185 H2: the previous behaviour
            // returned the existing id and silently dropped the
            // caller's importance / keywords / raw_excerpt. So
            // running `icm store -t T -c "X" -i medium` then `icm
            // store -t T -c "X" -i critical` left the importance at
            // medium without warning the user.
            //
            // New behaviour: merge the caller's metadata into the
            // existing row before returning the id.
            // - importance: take the max (critical > high > medium >
            //   low). Re-storing with a *higher* priority upgrades.
            //   Re-storing with a *lower* priority is a no-op so a
            //   careless write can't downgrade an already-flagged
            //   critical memory.
            // - keywords: union, preserving existing order then
            //   appending new ones not already present.
            // - raw_excerpt: prefer the new value if non-None,
            //   otherwise keep existing.
            // - updated_at: bumped whenever any field actually changed.
            let (existing_id, existing_importance_str, existing_keywords_json, existing_raw): (
                String,
                String,
                String,
                Option<String>,
            ) = self
                .conn
                .query_row(
                    // Audit finding: `summary_hash` already encodes the topic
                    // (Rust `to_lowercase()`, full Unicode) as part of the
                    // hash input — an additional `LOWER(topic) = LOWER(?)`
                    // comparison here used SQLite's built-in `LOWER()`,
                    // which is ASCII-only and does not fold e.g. 'É' → 'é'.
                    // For an all-caps accented topic like "DÉCISIONS" that
                    // mismatch meant this SELECT could fail to find the row
                    // the `INSERT OR IGNORE` conflict was already about,
                    // even though `summary_hash` alone uniquely identifies
                    // it. `summary_hash` is sufficient on its own.
                    "SELECT id, importance, keywords, raw_excerpt FROM memories
                     WHERE summary_hash = ?1",
                    params![hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(db_err)?;

            let existing_importance: Importance = existing_importance_str
                .parse()
                .unwrap_or(Importance::Medium);
            let merged_importance = max_importance(existing_importance, memory.importance);

            let existing_keywords: Vec<String> =
                serde_json::from_str(&existing_keywords_json).unwrap_or_default();
            let mut merged_keywords = existing_keywords.clone();
            for kw in &memory.keywords {
                if !merged_keywords.contains(kw) {
                    merged_keywords.push(kw.clone());
                }
            }

            let merged_raw = memory.raw_excerpt.clone().or(existing_raw.clone());

            let importance_changed = merged_importance != existing_importance;
            let keywords_changed = merged_keywords != existing_keywords;
            let raw_changed = merged_raw != existing_raw;
            if importance_changed || keywords_changed || raw_changed {
                let merged_keywords_json = serde_json::to_string(&merged_keywords)?;
                self.conn
                    .execute(
                        "UPDATE memories
                         SET importance = ?1, keywords = ?2, raw_excerpt = ?3, updated_at = ?4
                         WHERE id = ?5",
                        params![
                            merged_importance.to_string(),
                            merged_keywords_json,
                            merged_raw,
                            Utc::now().to_rfc3339(),
                            existing_id,
                        ],
                    )
                    .map_err(db_err)?;
                self.cache_invalidate(&existing_id);
            }

            tracing::debug!(
                topic = %memory.topic,
                existing = %existing_id,
                attempted = %memory.id,
                imp_changed = importance_changed,
                kw_changed = keywords_changed,
                raw_changed = raw_changed,
                "store: dedup'd duplicate memory (metadata merged)"
            );
            return Ok(existing_id);
        }

        // Sync to vec_memories for KNN search (only on a fresh insert).
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
// Test helpers (visible to other modules in crate for test use)

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::ensure_sqlite_vec;

    pub fn ensure_vec_init() {
        ensure_sqlite_vec();
    }
}

// Submodules (formerly monolithic `store.rs`, split for reviewability).
mod cache;
mod connection;
mod hooks;
mod rows;

// Shared helpers live in submodules; re-export so `use super::*` in
// sibling impl modules and the tests see them at `store::` paths.
pub(crate) use cache::*;
pub(crate) use connection::*;
pub(crate) use rows::*;

mod facts;
mod feedback;
mod maintenance;
mod memoir;
mod memory;
mod patterns;
#[cfg(test)]
mod tests;
mod transcript;
