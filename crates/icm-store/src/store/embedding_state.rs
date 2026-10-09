//! SQLite backend — embedding bookkeeping: which model and dimension the
//! stored vectors have, how many memories carry one, the write that sets
//! one vector, and the single explicit operation allowed to discard them.

use super::*;
use icm_core::{EmbeddingState, META_EMBEDDING_MODEL};

/// How long the start-up peek waits for a lock. WAL readers are not blocked
/// by a writer; only WAL recovery or a rollback-journal database can make
/// it wait, and a hook must not hang on that.
const PEEK_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// `Ok(None)` when the queried table does not exist (a database created
/// before that table, or a file with no schema yet).
fn or_missing_table<T>(result: rusqlite::Result<T>) -> IcmResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if is_missing_table(&e) => Ok(None),
        Err(e) => Err(db_err(e)),
    }
}

fn metadata_value(conn: &Connection, key: &str) -> IcmResult<Option<String>> {
    let row = or_missing_table(
        conn.query_row(
            "SELECT value FROM icm_metadata WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional(),
    )?;
    Ok(row.flatten())
}

/// Dimension of the `vec_memories` index, parsed from its own DDL
/// (`embedding float[N]`). Same rule as `schema::vec_table_dims`, which is
/// private to the migration code: the index is authoritative, the metadata
/// row can be missing on legacy databases.
fn index_dims(conn: &Connection) -> IcmResult<Option<usize>> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'vec_memories'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    Ok(sql.and_then(|sql| {
        let after = sql.split("float[").nth(1)?;
        after.split(']').next()?.trim().parse().ok()
    }))
}

/// The embedding state as `conn` sees it.
fn state_of(conn: &Connection) -> IcmResult<EmbeddingState> {
    let dims = match index_dims(conn)? {
        Some(dims) => Some(dims),
        None => metadata_value(conn, "embedding_dims")?.and_then(|s| s.parse().ok()),
    };
    let model = metadata_value(conn, META_EMBEDDING_MODEL)?.filter(|m| !m.trim().is_empty());
    // EXISTS, not COUNT(*): this must stay cheap on a large database.
    let has_vectors = or_missing_table(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memories WHERE embedding IS NOT NULL)",
        [],
        |row| row.get::<_, bool>(0),
    ))?
    .unwrap_or(false);
    Ok(EmbeddingState {
        dims,
        model,
        has_vectors,
    })
}

/// Read-only connection for the start-up peek.
///
/// It must see the WAL: with `immutable=1` SQLite reads the main file only,
/// so everything committed since the last checkpoint is invisible — and a
/// checkpoint only happens every 1000 WAL pages or when the last
/// connection closes. With an MCP server running, a peek through an
/// immutable connection kept answering "old model, old dimension" long
/// after `icm embed --migrate` had reset the index, and never saw a model
/// another process had just recorded.
///
/// The immutable snapshot remains the fallback for a database the process
/// cannot open normally: a read-only directory, where SQLite cannot create
/// the `-shm` sidecar a WAL reader needs (issue #263). A lock timeout is
/// reported as an error instead: a stale answer is worse than none.
fn open_for_peek(path: &Path) -> IcmResult<Connection> {
    if let Ok(conn) = open_readonly_uri(path, false) {
        // Cannot meaningfully fail; the default is no wait at all.
        let _ = conn.busy_timeout(PEEK_BUSY_TIMEOUT);
        // A real read: on a read-only directory the open succeeds and the
        // first read is what fails.
        match conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
            Ok(()) => return Ok(conn),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("database is locked") || msg.contains("database is busy") {
                    return Err(db_err(e));
                }
            }
        }
    }
    open_readonly_uri(path, true)
}

impl SqliteStore {
    /// Peek what the database says about its embeddings, without opening it
    /// for real: no migration, no write to the database.
    ///
    /// Runs on every process start (hooks included) before the store is
    /// opened, so the caller can open at the dimension the index really
    /// has. `Ok(None)` when the file is absent; an existing file with no
    /// schema yields an empty state rather than an error.
    ///
    /// The answer includes commits still in the WAL (see `open_for_peek`),
    /// but another process can still commit between this call and the
    /// open: the decision that matters is taken again on the open store,
    /// from [`Self::embedding_state`].
    pub fn read_embedding_state(path: &Path) -> IcmResult<Option<EmbeddingState>> {
        if !path.exists() {
            return Ok(None);
        }
        state_of(&open_for_peek(path)?).map(Some)
    }

    /// The embedding state of this open store, read on its own connection:
    /// what every other process has committed so far, checkpointed or not.
    pub fn embedding_state(&self) -> IcmResult<EmbeddingState> {
        state_of(&self.conn)
    }

    /// `(memories with a vector, memories)`. Two full counts: meant for
    /// `icm doctor`, not for the start-up path. `Ok(None)` when the file or
    /// the `memories` table is absent.
    pub fn read_embedding_coverage(path: &Path) -> IcmResult<Option<(usize, usize)>> {
        if !path.exists() {
            return Ok(None);
        }
        let conn = open_readonly_connection(path)?;
        or_missing_table(conn.query_row(
            "SELECT COUNT(embedding), COUNT(*) FROM memories",
            [],
            |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
        ))
    }

    /// Record `model` as the origin of the stored vectors, unless a model
    /// is already recorded. Returns whether the row was written.
    ///
    /// Never overwrites: replacing the recorded model is a migration
    /// ([`Self::reset_vector_index_for_model`]). The insert-if-absent is
    /// one statement, so two processes with different configurations
    /// cannot both win.
    ///
    /// Best effort by design. Any command, a read included, calls this the
    /// first time a database is opened by a binary that knows the key; it
    /// waits for the write lock only as long as other read-path
    /// bookkeeping does, and the next start tries again.
    pub fn record_embedding_model(&self, model: &str) -> IcmResult<bool> {
        if self.readonly {
            return Err(IcmError::ReadOnly("record_embedding_model".into()));
        }
        self.with_bookkeeping_timeout(|| {
            self.conn
                .execute(
                    "INSERT INTO icm_metadata (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO NOTHING",
                    params![META_EMBEDDING_MODEL, model],
                )
                .map(|written| written > 0)
                .map_err(db_err)
        })
    }

    /// Set the vector of one memory, and nothing else.
    ///
    /// `icm embed` computes vectors for minutes or hours from a list read
    /// at the start. Writing them back through `update` rewrote the whole
    /// row from that list, so a summary corrected, an importance raised or
    /// an access counted by another session in the meantime was silently
    /// reverted. This touches only `memories.embedding` and the vector
    /// index, in one transaction. `Ok(false)` when the memory no longer
    /// exists; a vector of the wrong dimension is an error and changes
    /// nothing.
    pub fn set_embedding(&self, id: &str, embedding: &[f32]) -> IcmResult<bool> {
        if self.readonly {
            return Err(IcmError::ReadOnly("set_embedding".into()));
        }
        let blob = embedding_to_blob(embedding);
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(db_err)?;
        let found = tx
            .execute(
                "UPDATE memories SET embedding = ?2 WHERE id = ?1",
                params![id, blob],
            )
            .map_err(db_err)?;
        if found == 0 {
            return Ok(false);
        }
        tx.execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id])
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO vec_memories (memory_id, embedding) VALUES (?1, ?2)",
            params![id, blob],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        self.cache_invalidate(id);
        Ok(true)
    }

    /// Discard every stored vector and recreate the vector index at
    /// `new_dims`. Returns how many memories lost their vector.
    ///
    /// With [`Self::reset_vector_index_for_model`], the only place that
    /// destroys embeddings on purpose: opening a database never does. The
    /// index, the BLOBs, `embedding_dims` and the recorded model change in
    /// one transaction, so an interrupted call leaves either the old state
    /// or a coherent empty index — the next open at `new_dims` finds
    /// nothing to migrate. The recorded model is removed: it described the
    /// vectors just discarded.
    pub fn reset_vector_index(&self, new_dims: usize) -> IcmResult<usize> {
        self.reset_index(new_dims, None, "reset_vector_index")
    }

    /// [`Self::reset_vector_index`] for a model migration: the same
    /// transaction also records `model` as the one the index now belongs
    /// to. There is no instant at which the database shows an empty index
    /// without its new model, so a process starting during the re-embedding
    /// either runs that model or stays away from the index (see
    /// `icm_core::embedding_policy::decide`).
    pub fn reset_vector_index_for_model(&self, new_dims: usize, model: &str) -> IcmResult<usize> {
        self.reset_index(new_dims, Some(model), "reset_vector_index_for_model")
    }

    fn reset_index(&self, new_dims: usize, model: Option<&str>, op: &str) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly(op.into()));
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(db_err)?;
        tx.execute_batch("DROP TABLE IF EXISTS vec_memories")
            .map_err(db_err)?;
        let cleared = tx
            .execute(
                "UPDATE memories SET embedding = NULL WHERE embedding IS NOT NULL",
                [],
            )
            .map_err(db_err)?;
        // Validates `new_dims` and writes `embedding_dims`; an invalid value
        // returns here and the transaction rolls back untouched.
        crate::schema::create_vec_table(&tx, new_dims)?;
        match model {
            Some(model) => tx.execute(
                "INSERT INTO icm_metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![META_EMBEDDING_MODEL, model],
            ),
            None => tx.execute(
                "DELETE FROM icm_metadata WHERE key = ?1",
                params![META_EMBEDDING_MODEL],
            ),
        }
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        self.cache_clear();
        Ok(cleared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn memory_with_vector(summary: &str, dims: usize) -> Memory {
        let mut m = Memory::new("embedding-state".into(), summary.into(), Importance::Medium);
        m.embedding = Some(vec![0.25_f32; dims]);
        m
    }

    /// File-backed store at `dims` holding `n` memories with a vector and
    /// one without.
    fn seed(path: &Path, dims: usize, n: usize) {
        let store = SqliteStore::with_dims(path, dims).unwrap();
        for i in 0..n {
            store
                .store(memory_with_vector(&format!("with vector {i}"), dims))
                .unwrap();
        }
        store
            .store(Memory::new(
                "embedding-state".into(),
                "without vector".into(),
                Importance::Medium,
            ))
            .unwrap();
    }

    fn vector_count(store: &SqliteStore) -> (usize, usize) {
        let blobs = store
            .conn
            .query_row("SELECT COUNT(embedding) FROM memories", [], |r| r.get(0))
            .unwrap();
        let indexed = store
            .conn
            .query_row("SELECT COUNT(*) FROM vec_memories", [], |r| r.get(0))
            .unwrap();
        (blobs, indexed)
    }

    #[test]
    fn embedding_state_of_absent_file_is_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("absent.db");
        assert_eq!(SqliteStore::read_embedding_state(&path).unwrap(), None);
        assert_eq!(SqliteStore::read_embedding_coverage(&path).unwrap(), None);
        assert!(!path.exists(), "a peek must not create the database");
    }

    #[test]
    fn embedding_state_of_schemaless_file_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.db");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap(),
            Some(EmbeddingState {
                dims: None,
                model: None,
                has_vectors: false,
            })
        );
        assert_eq!(SqliteStore::read_embedding_coverage(&path).unwrap(), None);
    }

    #[test]
    fn embedding_state_reports_dims_model_and_vectors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.db");
        {
            let store = SqliteStore::with_dims(&path, 64).unwrap();
            store
                .store(Memory::new("t".into(), "no vector".into(), Importance::Low))
                .unwrap();
        }
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap(),
            Some(EmbeddingState {
                dims: Some(64),
                model: None,
                has_vectors: false,
            })
        );
        assert_eq!(
            SqliteStore::read_embedding_coverage(&path).unwrap(),
            Some((0, 1))
        );

        {
            let store = SqliteStore::with_dims(&path, 64).unwrap();
            store.store(memory_with_vector("one", 64)).unwrap();
            store
                .set_metadata_str(META_EMBEDDING_MODEL, "some/model")
                .unwrap();
        }
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap(),
            Some(EmbeddingState {
                dims: Some(64),
                model: Some("some/model".into()),
                has_vectors: true,
            })
        );
        assert_eq!(
            SqliteStore::read_embedding_coverage(&path).unwrap(),
            Some((1, 2))
        );
    }

    /// Legacy databases carry a vector index but no `embedding_dims` row.
    #[test]
    fn embedding_state_reads_dims_from_the_index_when_metadata_is_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("legacy.db");
        {
            let store = SqliteStore::with_dims(&path, 128).unwrap();
            store
                .conn
                .execute("DELETE FROM icm_metadata WHERE key = 'embedding_dims'", [])
                .unwrap();
        }
        let state = SqliteStore::read_embedding_state(&path).unwrap().unwrap();
        assert_eq!(state.dims, Some(128));
    }

    /// The peek runs on every start-up: it must not write to the database.
    /// (It may leave WAL sidecars behind: that is the price of seeing the
    /// WAL, see `open_for_peek`.)
    #[test]
    fn embedding_state_peek_does_not_modify_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("peek.db");
        seed(&path, 64, 3);
        let before = std::fs::read(&path).unwrap();

        let state = SqliteStore::read_embedding_state(&path).unwrap().unwrap();
        assert!(state.has_vectors);

        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// Issue #263: in a directory the process cannot write to, the WAL
    /// reader cannot create its `-shm` file; the peek falls back to the
    /// immutable snapshot instead of failing.
    #[cfg(unix)]
    #[test]
    fn embedding_state_peek_works_in_a_readonly_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ro-dir.db");
        seed(&path, 64, 2);
        for sidecar in ["ro-dir.db-wal", "ro-dir.db-shm"] {
            let _ = std::fs::remove_file(dir.path().join(sidecar));
        }
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let state = SqliteStore::read_embedding_state(&path);

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            state.unwrap(),
            Some(EmbeddingState {
                dims: Some(64),
                model: None,
                has_vectors: true,
            })
        );
    }

    /// The peek must see what other connections committed, checkpointed or
    /// not: a long-lived server keeps its writes in the WAL for hours.
    #[test]
    fn embedding_state_peek_sees_commits_still_in_the_wal() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("wal.db");
        seed(&path, 64, 3);

        // Another process, still running: its commits stay in the WAL.
        let server = SqliteStore::with_dims(&path, 64).unwrap();
        server
            .set_metadata_str(META_EMBEDDING_MODEL, "model-a")
            .unwrap();
        let state = SqliteStore::read_embedding_state(&path).unwrap().unwrap();
        assert_eq!(state.model.as_deref(), Some("model-a"));

        // A migration in progress in that process: index emptied and
        // resized, new model recorded, nothing checkpointed.
        assert_eq!(
            server.reset_vector_index_for_model(128, "model-b").unwrap(),
            3
        );
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap(),
            Some(EmbeddingState {
                dims: Some(128),
                model: Some("model-b".into()),
                has_vectors: false,
            })
        );
        assert_eq!(
            SqliteStore::read_embedding_coverage(&path).unwrap(),
            Some((0, 4))
        );
    }

    #[test]
    fn embedding_reset_clears_vectors_and_recreates_the_index() {
        let store = SqliteStore::in_memory_with_dims(64).unwrap();
        let id = store.store(memory_with_vector("a", 64)).unwrap();
        store.store(memory_with_vector("b", 64)).unwrap();
        store
            .store(Memory::new("t".into(), "plain".into(), Importance::Low))
            .unwrap();
        store
            .set_metadata_str(META_EMBEDDING_MODEL, "old/model")
            .unwrap();
        // Warm the cache: a cached `Memory` must not keep its old vector.
        assert!(store.get(&id).unwrap().unwrap().embedding.is_some());
        assert_eq!(vector_count(&store), (2, 2));

        assert_eq!(store.reset_vector_index(128).unwrap(), 2);

        assert_eq!(vector_count(&store), (0, 0));
        assert_eq!(store.count().unwrap(), 3, "memories themselves are kept");
        assert!(store.get(&id).unwrap().unwrap().embedding.is_none());
        assert_eq!(
            store.get_metadata_str("embedding_dims").unwrap().as_deref(),
            Some("128")
        );
        assert_eq!(store.get_metadata_str(META_EMBEDDING_MODEL).unwrap(), None);

        // The new index accepts vectors of the new dimension only.
        let mut m = store.get(&id).unwrap().unwrap();
        m.embedding = Some(vec![0.5_f32; 128]);
        store.update(&m).unwrap();
        assert_eq!(vector_count(&store), (1, 1));
        let hits = store.search_by_embedding(&[0.5_f32; 128], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, id);
    }

    #[test]
    fn embedding_reset_with_invalid_dims_leaves_everything_in_place() {
        let store = SqliteStore::in_memory_with_dims(64).unwrap();
        store.store(memory_with_vector("a", 64)).unwrap();

        assert!(matches!(
            store.reset_vector_index(8),
            Err(IcmError::Config(_))
        ));

        assert_eq!(vector_count(&store), (1, 1));
        assert_eq!(
            store.get_metadata_str("embedding_dims").unwrap().as_deref(),
            Some("64")
        );
    }

    #[test]
    fn embedding_reset_refuses_a_readonly_store() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ro.db");
        seed(&path, 64, 2);

        let ro = SqliteStore::open_readonly(&path).unwrap();
        assert!(matches!(
            ro.reset_vector_index(128),
            Err(IcmError::ReadOnly(_))
        ));
        assert_eq!(vector_count(&ro), (2, 2));
    }

    /// After a reset, an ordinary open at the new dimension must find a
    /// coherent database: no migration to run, no memory lost, no error.
    #[test]
    fn embedding_reset_then_reopen_at_new_dims_is_consistent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("reset.db");
        seed(&path, 64, 3);
        {
            let store = SqliteStore::with_dims(&path, 64).unwrap();
            assert_eq!(store.reset_vector_index(128).unwrap(), 3);
        }
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap(),
            Some(EmbeddingState {
                dims: Some(128),
                model: None,
                has_vectors: false,
            })
        );

        let store = SqliteStore::with_dims(&path, 128).unwrap();
        assert_eq!(store.count().unwrap(), 4);
        assert_eq!(vector_count(&store), (0, 0));
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::schema::SCHEMA_VERSION);
        store.store(memory_with_vector("new model", 128)).unwrap();
        assert_eq!(vector_count(&store), (1, 1));
        assert_eq!(
            SqliteStore::read_embedding_coverage(&path).unwrap(),
            Some((1, 5))
        );
    }

    /// The incident this module exists for: a database written at 768
    /// dimensions by a previous release, no model recorded, opened by a
    /// binary whose default model has 1024.
    #[test]
    fn embedding_vectors_survive_an_open_at_another_dimension() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mac.db");
        seed(&path, 768, 5);

        let expected = EmbeddingState {
            dims: Some(768),
            model: None,
            has_vectors: true,
        };
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap().as_ref(),
            Some(&expected)
        );

        {
            let store = SqliteStore::with_dims(&path, 1024).unwrap();
            assert_eq!(vector_count(&store), (5, 5));
            assert_eq!(
                store.get_metadata_str("embedding_dims").unwrap().as_deref(),
                Some("768")
            );
            // Keyword recall and vector-less writes keep working.
            assert_eq!(store.search_fts("vector", 10).unwrap().len(), 6);
            store
                .store(Memory::new(
                    "embedding-state".into(),
                    "written keyword-only".into(),
                    Importance::Medium,
                ))
                .unwrap();
        }

        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap().as_ref(),
            Some(&expected)
        );
        assert_eq!(
            SqliteStore::read_embedding_coverage(&path).unwrap(),
            Some((5, 7))
        );
    }

    /// A migration killed after its reset: the process never closed its
    /// connection, so nothing was checkpointed. Both the peek and a newly
    /// opened store must report the migrated state, not the one in the
    /// main file.
    #[test]
    fn embedding_state_after_a_killed_migration_is_the_migrated_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("killed.db");
        seed(&path, 64, 4);
        {
            let store = SqliteStore::with_dims(&path, 64).unwrap();
            store
                .set_metadata_str(META_EMBEDDING_MODEL, "model-a")
                .unwrap();
        }

        // Same dimension, another model, one memory re-embedded, then the
        // process dies: no close, no checkpoint.
        let migrating = SqliteStore::with_dims(&path, 64).unwrap();
        assert_eq!(
            migrating
                .reset_vector_index_for_model(64, "model-b")
                .unwrap(),
            4
        );
        let id = migrating.list_all().unwrap()[0].id.clone();
        assert!(migrating.set_embedding(&id, &[0.9_f32; 64]).unwrap());
        std::mem::forget(migrating);

        let expected = EmbeddingState {
            dims: Some(64),
            model: Some("model-b".into()),
            has_vectors: true,
        };
        assert_eq!(
            SqliteStore::read_embedding_state(&path).unwrap().as_ref(),
            Some(&expected)
        );
        let resumed = SqliteStore::with_dims(&path, 64).unwrap();
        assert_eq!(resumed.embedding_state().unwrap(), expected);
        assert_eq!(vector_count(&resumed), (1, 1));
    }

    #[test]
    fn embedding_reset_for_model_records_the_model_atomically() {
        let store = SqliteStore::in_memory_with_dims(64).unwrap();
        store.store(memory_with_vector("a", 64)).unwrap();
        store
            .set_metadata_str(META_EMBEDDING_MODEL, "old/model")
            .unwrap();

        // Invalid dimension: nothing changes, the old model stays.
        assert!(store.reset_vector_index_for_model(8, "new/model").is_err());
        assert_eq!(
            store.embedding_state().unwrap(),
            EmbeddingState {
                dims: Some(64),
                model: Some("old/model".into()),
                has_vectors: true,
            }
        );

        assert_eq!(
            store
                .reset_vector_index_for_model(128, "new/model")
                .unwrap(),
            1
        );
        assert_eq!(
            store.embedding_state().unwrap(),
            EmbeddingState {
                dims: Some(128),
                model: Some("new/model".into()),
                has_vectors: false,
            }
        );
    }

    #[test]
    fn embedding_model_is_recorded_once_and_never_overwritten() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("model.db");
        let a = SqliteStore::with_dims(&path, 64).unwrap();
        let b = SqliteStore::with_dims(&path, 64).unwrap();

        assert!(a.record_embedding_model("model-a").unwrap());
        // Another process, configured otherwise, arrives second.
        assert!(!b.record_embedding_model("model-b").unwrap());
        assert!(!a.record_embedding_model("model-a").unwrap());
        assert_eq!(
            b.embedding_state().unwrap().model.as_deref(),
            Some("model-a")
        );

        let ro = SqliteStore::open_readonly(&path).unwrap();
        assert!(matches!(
            ro.record_embedding_model("model-c"),
            Err(IcmError::ReadOnly(_))
        ));
    }

    /// Recording the model happens on the read path of any command: it
    /// must give up quickly behind a writer, not wait the 30 s a real
    /// write would.
    #[test]
    fn embedding_model_recording_does_not_wait_long_behind_a_writer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("busy.db");
        let reader = SqliteStore::with_dims(&path, 64).unwrap();
        let writer = SqliteStore::with_dims(&path, 64).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let started = std::time::Instant::now();
        let result = reader.record_embedding_model("model-a");
        let waited = started.elapsed();

        writer.conn.execute_batch("ROLLBACK;").unwrap();
        assert!(result.is_err(), "the write lock was held: {result:?}");
        assert!(
            waited < std::time::Duration::from_secs(10),
            "waited {waited:?}"
        );
        // Nothing lost: the next attempt succeeds.
        assert!(reader.record_embedding_model("model-a").unwrap());
    }

    /// `icm embed` writes vectors computed from an old read of the
    /// memories. Whatever another session changed in the meantime stays.
    #[test]
    fn embedding_set_touches_only_the_vector() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("set.db");
        let embedding_process = SqliteStore::with_dims(&path, 64).unwrap();
        let id = embedding_process
            .store(Memory::new(
                "t".into(),
                "original summary".into(),
                Importance::Medium,
            ))
            .unwrap();
        // Read at the start of the run, as `icm embed` does.
        let snapshot = embedding_process.list_all().unwrap();
        assert_eq!(snapshot[0].summary, "original summary");

        // Meanwhile, another session edits the memory.
        {
            let other = SqliteStore::with_dims(&path, 64).unwrap();
            let mut m = other.get(&id).unwrap().unwrap();
            m.summary = "corrected summary".into();
            m.importance = Importance::Critical;
            m.access_count = 7;
            m.related_ids = vec!["neighbour".into()];
            other.update(&m).unwrap();
        }

        assert!(
            embedding_process
                .set_embedding(&id, &[0.5_f32; 64])
                .unwrap()
        );

        let after = SqliteStore::with_dims(&path, 64)
            .unwrap()
            .get(&id)
            .unwrap()
            .unwrap();
        assert_eq!(after.summary, "corrected summary");
        assert_eq!(after.importance, Importance::Critical);
        assert_eq!(after.access_count, 7);
        assert_eq!(after.related_ids, vec!["neighbour".to_string()]);
        assert_eq!(after.embedding.as_deref(), Some(&[0.5_f32; 64][..]));
        assert_eq!(vector_count(&embedding_process), (1, 1));
        // The corrected text is still what the lexical index knows.
        assert_eq!(
            embedding_process.search_fts("corrected", 5).unwrap().len(),
            1
        );
        assert!(
            embedding_process
                .search_fts("original", 5)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn embedding_set_replaces_reports_missing_and_rejects_bad_input() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("set2.db");
        let store = SqliteStore::with_dims(&path, 64).unwrap();
        let id = store.store(memory_with_vector("a", 64)).unwrap();
        // Warm the cache: the next `get` must not serve the old vector.
        assert!(store.get(&id).unwrap().is_some());

        // Replacing keeps one index row per memory.
        assert!(store.set_embedding(&id, &[0.75_f32; 64]).unwrap());
        assert_eq!(vector_count(&store), (1, 1));
        assert_eq!(
            store.get(&id).unwrap().unwrap().embedding.as_deref(),
            Some(&[0.75_f32; 64][..])
        );
        assert_eq!(
            store.search_by_embedding(&[0.75_f32; 64], 1).unwrap()[0]
                .0
                .id,
            id
        );

        // A memory deleted since it was listed.
        assert!(!store.set_embedding("no-such-id", &[0.1_f32; 64]).unwrap());
        assert_eq!(vector_count(&store), (1, 1));

        // Wrong dimension: an error, and the stored vector is untouched.
        assert!(store.set_embedding(&id, &[0.1_f32; 128]).is_err());
        assert_eq!(vector_count(&store), (1, 1));
        assert_eq!(
            store.get(&id).unwrap().unwrap().embedding.as_deref(),
            Some(&[0.75_f32; 64][..])
        );

        drop(store);
        let ro = SqliteStore::open_readonly(&path).unwrap();
        assert!(matches!(
            ro.set_embedding(&id, &[0.1_f32; 64]),
            Err(IcmError::ReadOnly(_))
        ));
    }
}
