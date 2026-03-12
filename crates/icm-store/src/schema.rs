use rusqlite::Connection;

use icm_core::IcmError;

/// Initialize the database schema. `embedding_dims` controls the sqlite-vec vector size.
/// Pass `None` to skip vector table creation (no embeddings feature).
pub fn init_db(conn: &Connection) -> Result<(), IcmError> {
    init_db_with_dims(conn, 384)
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

            related_ids TEXT -- JSON array
        );

        CREATE INDEX IF NOT EXISTS idx_memories_topic ON memories(topic);
        CREATE INDEX IF NOT EXISTS idx_memories_weight ON memories(weight);
        CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);

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
    .map_err(|e| IcmError::Database(e.to_string()))?;

    // Check if FTS table already exists (memories)
    let fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories_fts'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !fts_exists {
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

            CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, topic, summary, keywords)
                VALUES('delete', old.rowid, old.id, old.topic, old.summary, old.keywords);
                INSERT INTO memories_fts(rowid, id, topic, summary, keywords)
                VALUES (new.rowid, new.id, new.topic, new.summary, new.keywords);
            END;
            ",
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;
    }

    // Check if concepts FTS table already exists
    let concepts_fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='concepts_fts'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !concepts_fts_exists {
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
        .map_err(|e| IcmError::Database(e.to_string()))?;
    }

    // Metadata key-value table for internal state (e.g. last_decay_at)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS icm_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| IcmError::Database(e.to_string()))?;

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
    .map_err(|e| IcmError::Database(e.to_string()))?;

    // Feedback FTS table
    let feedback_fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='feedback_fts'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !feedback_fts_exists {
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
        .map_err(|e| IcmError::Database(e.to_string()))?;
    }

    // Migration: add updated_at column if missing (existing DBs pre-0.3.1)
    let has_updated_at: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='updated_at'")
        .and_then(|mut s| s.query_row([], |row| row.get(0)))
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !has_updated_at {
        conn.execute_batch(
            "ALTER TABLE memories ADD COLUMN updated_at TEXT;
             UPDATE memories SET updated_at = created_at WHERE updated_at IS NULL;",
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;
    }

    // Migration: add embedding column if missing (existing DBs)
    let has_embedding: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='embedding'")
        .and_then(|mut s| s.query_row([], |row| row.get(0)))
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !has_embedding {
        conn.execute_batch("ALTER TABLE memories ADD COLUMN embedding BLOB")
            .map_err(|e| IcmError::Database(e.to_string()))?;
    }

    // sqlite-vec virtual table for vector search (dimension-aware)
    let vec_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='vec_memories'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if vec_exists {
        // Check if stored dims differ from requested dims — if so, recreate
        let stored_dims: Option<String> = conn
            .query_row(
                "SELECT value FROM icm_metadata WHERE key = 'embedding_dims'",
                [],
                |row| row.get(0),
            )
            .ok();
        let stored: usize = stored_dims.and_then(|s| s.parse().ok()).unwrap_or(384);
        if stored != embedding_dims {
            // Model changed — drop vec table and clear embeddings
            conn.execute_batch("DROP TABLE IF EXISTS vec_memories")
                .map_err(|e| IcmError::Database(e.to_string()))?;
            conn.execute("UPDATE memories SET embedding = NULL", [])
                .map_err(|e| IcmError::Database(e.to_string()))?;
            conn.execute_batch(&format!(
                "CREATE VIRTUAL TABLE vec_memories USING vec0(
                    memory_id TEXT PRIMARY KEY,
                    embedding float[{embedding_dims}] distance_metric=cosine
                )"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO icm_metadata (key, value) VALUES ('embedding_dims', ?1)",
                [&embedding_dims.to_string()],
            )
            .map_err(|e| IcmError::Database(e.to_string()))?;
        }
    } else {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE vec_memories USING vec0(
                memory_id TEXT PRIMARY KEY,
                embedding float[{embedding_dims}] distance_metric=cosine
            )"
        ))
        .map_err(|e| IcmError::Database(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO icm_metadata (key, value) VALUES ('embedding_dims', ?1)",
            [&embedding_dims.to_string()],
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;
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
