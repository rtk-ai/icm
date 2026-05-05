use rusqlite::Connection;

use icm_core::{IcmError, IcmResult};

use crate::store::db_err;

/// Check if a FTS virtual table exists in sqlite_master.
fn fts_table_exists(conn: &Connection, name: &str) -> Result<bool, IcmError> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get(0),
    )
    .map_err(db_err)
}

fn create_vec_table(conn: &Connection, embedding_dims: usize) -> Result<(), IcmError> {
    if !(64..=4096).contains(&embedding_dims) {
        return Err(IcmError::Config(format!(
            "embedding_dims must be between 64 and 4096, got {embedding_dims}"
        )));
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE vec_memories USING vec0(
            memory_id TEXT PRIMARY KEY,
            embedding float[{embedding_dims}] distance_metric=cosine
        )"
    ))
    .map_err(db_err)?;
    conn.execute(
        "INSERT OR REPLACE INTO icm_metadata (key, value) VALUES ('embedding_dims', ?1)",
        [&embedding_dims.to_string()],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Initialize the database schema. `embedding_dims` controls the sqlite-vec vector size.
/// Pass `None` to skip vector table creation (no embeddings feature).
pub fn init_db(conn: &Connection) -> Result<(), IcmError> {
    init_db_with_dims(conn, icm_core::DEFAULT_EMBEDDING_DIMS)
}

pub fn init_db_with_dims(conn: &Connection, embedding_dims: usize) -> Result<(), IcmError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT '',
            last_accessed TEXT NOT NULL,
            access_count INTEGER DEFAULT 0,
            weight REAL DEFAULT 1.0,

            topic TEXT NOT NULL,
            summary TEXT NOT NULL,
            raw_excerpt TEXT,
            keywords TEXT, -- JSON array

            importance TEXT NOT NULL,
            source_type TEXT NOT NULL,
            source_data TEXT, -- JSON

            related_ids TEXT, -- JSON array
            -- SHA-256 over normalize(topic + '\\0' + summary). Used by
            -- INSERT OR IGNORE dedup. NULL on rows that predate the
            -- migration (existing duplicates intentionally untouched).
            summary_hash TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_memories_topic ON memories(topic);
        CREATE INDEX IF NOT EXISTS idx_memories_weight ON memories(weight);
        CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
        -- The summary_hash partial unique index is created later, AFTER the
        -- idempotent ALTER TABLE migration that adds the column on legacy
        -- DBs (PRs #176 + this hotfix). Creating it here would crash with
        -- `no such column: summary_hash` on any pre-0.10.43 DB because
        -- `CREATE TABLE IF NOT EXISTS` is a no-op on existing tables — the
        -- new column declaration above only takes effect on fresh DBs.

        -- Memoir tables
        CREATE TABLE IF NOT EXISTS memoirs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            consolidation_threshold INTEGER NOT NULL DEFAULT 50
        );

        CREATE TABLE IF NOT EXISTS concepts (
            id TEXT PRIMARY KEY,
            memoir_id TEXT NOT NULL REFERENCES memoirs(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            definition TEXT NOT NULL,
            labels TEXT NOT NULL DEFAULT '[]', -- JSON array of {namespace, value}
            confidence REAL NOT NULL DEFAULT 0.5,
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            source_memory_ids TEXT NOT NULL DEFAULT '[]', -- JSON array of strings
            UNIQUE(memoir_id, name)
        );

        CREATE INDEX IF NOT EXISTS idx_concepts_memoir ON concepts(memoir_id);
        CREATE INDEX IF NOT EXISTS idx_concepts_name ON concepts(name);
        CREATE INDEX IF NOT EXISTS idx_concepts_confidence ON concepts(confidence);

        CREATE TABLE IF NOT EXISTS concept_links (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            target_id TEXT NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            relation TEXT NOT NULL,
            weight REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL,
            UNIQUE(source_id, target_id, relation),
            CHECK(source_id != target_id)
        );

        CREATE INDEX IF NOT EXISTS idx_concept_links_source ON concept_links(source_id);
        CREATE INDEX IF NOT EXISTS idx_concept_links_target ON concept_links(target_id);
        ",
    )
    .map_err(db_err)?;

    // Migration: add `summary_hash` column to existing DBs that predate
    // the dedup feature. SQLite has no `ADD COLUMN IF NOT EXISTS`, so we
    // try the ALTER and ignore the "duplicate column name" error.
    if let Err(e) = conn.execute("ALTER TABLE memories ADD COLUMN summary_hash TEXT", []) {
        let msg = e.to_string();
        if !msg.contains("duplicate column name") {
            return Err(db_err(e));
        }
    }
    // Ensure the partial unique index exists even on DBs that ran an old
    // CREATE TABLE (which had no summary_hash column to index against).
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_topic_hash
            ON memories(LOWER(topic), summary_hash) WHERE summary_hash IS NOT NULL;",
    )
    .map_err(db_err)?;

    // Check if FTS table already exists (memories)
    if !fts_table_exists(conn, "memories_fts")? {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE memories_fts USING fts5(
                id,
                topic,
                summary,
                keywords,
                content='memories',
                content_rowid='rowid'
            );

            CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, id, topic, summary, keywords)
                VALUES (new.rowid, new.id, new.topic, new.summary, new.keywords);
            END;

            CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, topic, summary, keywords)
                VALUES('delete', old.rowid, old.id, old.topic, old.summary, old.keywords);
            END;

            CREATE TRIGGER memories_au AFTER UPDATE OF topic, summary, keywords ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, topic, summary, keywords)
                VALUES('delete', old.rowid, old.id, old.topic, old.summary, old.keywords);
                INSERT INTO memories_fts(rowid, id, topic, summary, keywords)
                VALUES (new.rowid, new.id, new.topic, new.summary, new.keywords);
            END;
            ",
        )
        .map_err(db_err)?;
    }

    // Check if concepts FTS table already exists
    if !fts_table_exists(conn, "concepts_fts")? {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE concepts_fts USING fts5(
                id,
                name,
                definition,
                labels,
                content='concepts',
                content_rowid='rowid'
            );

            CREATE TRIGGER concepts_ai AFTER INSERT ON concepts BEGIN
                INSERT INTO concepts_fts(rowid, id, name, definition, labels)
                VALUES (new.rowid, new.id, new.name, new.definition, new.labels);
            END;

            CREATE TRIGGER concepts_ad AFTER DELETE ON concepts BEGIN
                INSERT INTO concepts_fts(concepts_fts, rowid, id, name, definition, labels)
                VALUES('delete', old.rowid, old.id, old.name, old.definition, old.labels);
            END;

            CREATE TRIGGER concepts_au AFTER UPDATE ON concepts BEGIN
                INSERT INTO concepts_fts(concepts_fts, rowid, id, name, definition, labels)
                VALUES('delete', old.rowid, old.id, old.name, old.definition, old.labels);
                INSERT INTO concepts_fts(rowid, id, name, definition, labels)
                VALUES (new.rowid, new.id, new.name, new.definition, new.labels);
            END;
            ",
        )
        .map_err(db_err)?;
    }

    // Metadata key-value table for internal state (e.g. last_decay_at)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS icm_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(db_err)?;

    // Feedback table
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS feedback (
            id TEXT PRIMARY KEY,
            topic TEXT NOT NULL,
            context TEXT NOT NULL,
            predicted TEXT NOT NULL,
            corrected TEXT NOT NULL,
            reason TEXT,
            source TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            applied_count INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_feedback_topic ON feedback(topic);
        ",
    )
    .map_err(db_err)?;

    // Feedback FTS table
    if !fts_table_exists(conn, "feedback_fts")? {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE feedback_fts USING fts5(
                id, topic, context, predicted, corrected, reason,
                content='feedback', content_rowid='rowid'
            );

            CREATE TRIGGER feedback_ai AFTER INSERT ON feedback BEGIN
                INSERT INTO feedback_fts(rowid, id, topic, context, predicted, corrected, reason)
                VALUES (new.rowid, new.id, new.topic, new.context, new.predicted, new.corrected, new.reason);
            END;

            CREATE TRIGGER feedback_ad AFTER DELETE ON feedback BEGIN
                INSERT INTO feedback_fts(feedback_fts, rowid, id, topic, context, predicted, corrected, reason)
                VALUES('delete', old.rowid, old.id, old.topic, old.context, old.predicted, old.corrected, old.reason);
            END;

            CREATE TRIGGER feedback_au AFTER UPDATE ON feedback BEGIN
                INSERT INTO feedback_fts(feedback_fts, rowid, id, topic, context, predicted, corrected, reason)
                VALUES('delete', old.rowid, old.id, old.topic, old.context, old.predicted, old.corrected, old.reason);
                INSERT INTO feedback_fts(rowid, id, topic, context, predicted, corrected, reason)
                VALUES (new.rowid, new.id, new.topic, new.context, new.predicted, new.corrected, new.reason);
            END;
            ",
        )
        .map_err(db_err)?;
    }

    // Transcripts (verbatim sessions + messages)
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent TEXT NOT NULL DEFAULT '',
            project TEXT,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            metadata TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project);
        CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at);

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            tool_name TEXT,
            tokens INTEGER,
            ts TEXT NOT NULL,
            metadata TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
        CREATE INDEX IF NOT EXISTS idx_messages_ts ON messages(ts);
        CREATE INDEX IF NOT EXISTS idx_messages_role ON messages(role);
        ",
    )
    .map_err(db_err)?;

    // FTS5 over messages.content (+ role/tool_name so 'role:tool' style filters work)
    if !fts_table_exists(conn, "messages_fts")? {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE messages_fts USING fts5(
                id UNINDEXED,
                session_id UNINDEXED,
                role,
                content,
                tool_name,
                content='messages',
                content_rowid='rowid'
            );

            CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, id, session_id, role, content, tool_name)
                VALUES (new.rowid, new.id, new.session_id, new.role, new.content, COALESCE(new.tool_name, ''));
            END;

            CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, id, session_id, role, content, tool_name)
                VALUES('delete', old.rowid, old.id, old.session_id, old.role, old.content, COALESCE(old.tool_name, ''));
            END;

            CREATE TRIGGER messages_au AFTER UPDATE OF role, content, tool_name ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, id, session_id, role, content, tool_name)
                VALUES('delete', old.rowid, old.id, old.session_id, old.role, old.content, COALESCE(old.tool_name, ''));
                INSERT INTO messages_fts(rowid, id, session_id, role, content, tool_name)
                VALUES (new.rowid, new.id, new.session_id, new.role, new.content, COALESCE(new.tool_name, ''));
            END;
            ",
        )
        .map_err(db_err)?;
    }

    // Migration: add updated_at column if missing (existing DBs pre-0.3.1)
    let has_updated_at: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='updated_at'")
        .and_then(|mut s| s.query_row([], |row| row.get(0)))
        .map_err(db_err)?;

    if !has_updated_at {
        conn.execute_batch(
            "ALTER TABLE memories ADD COLUMN updated_at TEXT;
             UPDATE memories SET updated_at = created_at WHERE updated_at IS NULL;",
        )
        .map_err(db_err)?;
    }

    // Migration: add embedding column if missing (existing DBs)
    let has_embedding: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='embedding'")
        .and_then(|mut s| s.query_row([], |row| row.get(0)))
        .map_err(db_err)?;

    if !has_embedding {
        conn.execute_batch("ALTER TABLE memories ADD COLUMN embedding BLOB")
            .map_err(db_err)?;
    }

    // Migration: scope FTS UPDATE trigger to indexed columns only (fixes #44).
    // The old trigger fired on ANY update (including update_access, apply_decay)
    // which churned the FTS index and could create ghost entries.
    migrate_fts_update_trigger(conn)?;

    // sqlite-vec virtual table for vector search (dimension-aware)
    let vec_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='vec_memories'",
            [],
            |row| row.get(0),
        )
        .map_err(db_err)?;

    if vec_exists {
        // Check if stored dims differ from requested dims — if so, recreate
        let stored_dims: Option<String> = conn
            .query_row(
                "SELECT value FROM icm_metadata WHERE key = 'embedding_dims'",
                [],
                |row| row.get(0),
            )
            .ok();
        let stored: usize = stored_dims
            .and_then(|s| s.parse().ok())
            .unwrap_or(icm_core::DEFAULT_EMBEDDING_DIMS);
        if stored != embedding_dims {
            // Model changed — drop vec table and clear embeddings
            conn.execute_batch("DROP TABLE IF EXISTS vec_memories")
                .map_err(db_err)?;
            conn.execute("UPDATE memories SET embedding = NULL", [])
                .map_err(db_err)?;
            create_vec_table(conn, embedding_dims)?;
        }
    } else {
        create_vec_table(conn, embedding_dims)?;
    }

    Ok(())
}

/// Migrate existing DBs: replace the broad `memories_au` trigger with one
/// scoped to `UPDATE OF topic, summary, keywords` so that `update_access` /
/// `apply_decay` no longer churn the FTS index.  Also rebuilds the FTS index
/// to purge any ghost entries accumulated before this fix.
fn migrate_fts_update_trigger(conn: &Connection) -> IcmResult<()> {
    // Check if the trigger already has the scoped form by inspecting its SQL.
    let trigger_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='trigger' AND name='memories_au'",
            [],
            |row| row.get(0),
        )
        .ok();

    let needs_migration = match &trigger_sql {
        // Trigger exists but uses the old broad form (no OF clause).
        Some(sql) => !sql.contains("UPDATE OF"),
        // Trigger doesn't exist — FTS table was just created with the new form.
        None => false,
    };

    if needs_migration {
        conn.execute_batch(
            "
            DROP TRIGGER IF EXISTS memories_au;
            CREATE TRIGGER memories_au AFTER UPDATE OF topic, summary, keywords ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, topic, summary, keywords)
                VALUES('delete', old.rowid, old.id, old.topic, old.summary, old.keywords);
                INSERT INTO memories_fts(rowid, id, topic, summary, keywords)
                VALUES (new.rowid, new.id, new.topic, new.summary, new.keywords);
            END;
            INSERT INTO memories_fts(memories_fts) VALUES('rebuild');
            ",
        )
        .map_err(db_err)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_helpers::ensure_vec_init;

    #[test]
    fn test_init_db() {
        ensure_vec_init();
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // Second call should be idempotent
        init_db(&conn).unwrap();
    }

    /// Simulates upgrading a pre-0.10.43 database (no `summary_hash`
    /// column on the `memories` table) with the new binary. The migration
    /// must (a) succeed without error, (b) add the missing column, and
    /// (c) leave the partial unique index in place. Regression test for
    /// the V8 retest blocker where the index was created in the same
    /// batch as `CREATE TABLE IF NOT EXISTS`, before the ALTER TABLE
    /// migration could run, bricking every existing user DB on upgrade.
    #[test]
    fn test_migration_from_pre_0_10_43_schema() {
        ensure_vec_init();
        let conn = Connection::open_in_memory().unwrap();

        // Build a legacy schema by hand: CREATE TABLE without summary_hash.
        conn.execute_batch(
            "CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT '',
                last_accessed TEXT NOT NULL,
                access_count INTEGER DEFAULT 0,
                weight REAL DEFAULT 1.0,
                topic TEXT NOT NULL,
                summary TEXT NOT NULL,
                raw_excerpt TEXT,
                keywords TEXT,
                importance TEXT NOT NULL,
                source_type TEXT NOT NULL,
                source_data TEXT,
                related_ids TEXT
            );",
        )
        .unwrap();

        // Seed one row that pre-dates the migration — must survive intact.
        conn.execute(
            "INSERT INTO memories (id, created_at, last_accessed, topic, summary, importance, source_type)
             VALUES ('legacy-1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'legacy', 'old summary', 'medium', 'manual')",
            [],
        )
        .unwrap();

        // Now run the new init — this is what the V8 test reproduced as a
        // bricking error. Must complete without panic.
        init_db(&conn).expect("upgrade must succeed on a legacy schema");

        // summary_hash column now exists.
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(memories)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            cols.iter().any(|c| c == "summary_hash"),
            "summary_hash column must be added by the migration; saw {cols:?}"
        );

        // Partial unique index now exists.
        let indices: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name = 'idx_memories_topic_hash'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            indices.len(),
            1,
            "idx_memories_topic_hash must exist after migration"
        );

        // Legacy row still readable, summary_hash is NULL on it.
        let (legacy_summary, legacy_hash): (String, Option<String>) = conn
            .query_row(
                "SELECT summary, summary_hash FROM memories WHERE id = 'legacy-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(legacy_summary, "old summary");
        assert!(
            legacy_hash.is_none(),
            "legacy row must keep NULL summary_hash so it doesn't conflict with the partial unique index"
        );

        // Re-running the migration is idempotent.
        init_db(&conn).expect("re-running migration must be a no-op");
    }

    #[test]
    fn test_embedding_dims_too_small() {
        ensure_vec_init();
        let conn = Connection::open_in_memory().unwrap();
        // dims < 64 should fail
        let result = init_db_with_dims(&conn, 32);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("embedding_dims"),
            "error should mention embedding_dims: {err}"
        );
    }

    #[test]
    fn test_embedding_dims_too_large() {
        ensure_vec_init();
        let conn = Connection::open_in_memory().unwrap();
        // dims > 4096 should fail
        let result = init_db_with_dims(&conn, 8192);
        assert!(result.is_err());
    }

    #[test]
    fn test_embedding_dims_boundary_valid() {
        ensure_vec_init();
        let conn = Connection::open_in_memory().unwrap();
        // 64 and 4096 are valid boundary values
        assert!(init_db_with_dims(&conn, 64).is_ok());

        let conn2 = Connection::open_in_memory().unwrap();
        assert!(init_db_with_dims(&conn2, 4096).is_ok());
    }

    #[test]
    fn test_memoir_tables_exist() {
        ensure_vec_init();
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Verify all new tables exist
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };

        assert!(tables.contains(&"memoirs".to_string()));
        assert!(tables.contains(&"concepts".to_string()));
        assert!(tables.contains(&"concept_links".to_string()));
        assert!(tables.contains(&"concepts_fts".to_string()));
        assert!(tables.contains(&"vec_memories".to_string()));
    }
}
