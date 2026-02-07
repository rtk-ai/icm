use rusqlite::Connection;

use icm_core::IcmError;

pub fn init_db(conn: &Connection) -> Result<(), IcmError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
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

    // Migration: add embedding column if missing (existing DBs)
    let has_embedding: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='embedding'")
        .and_then(|mut s| s.query_row([], |row| row.get(0)))
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !has_embedding {
        conn.execute_batch("ALTER TABLE memories ADD COLUMN embedding BLOB")
            .map_err(|e| IcmError::Database(e.to_string()))?;
    }

    // sqlite-vec virtual table for vector search
    let vec_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='vec_memories'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| IcmError::Database(e.to_string()))?;

    if !vec_exists {
        conn.execute_batch(
            "CREATE VIRTUAL TABLE vec_memories USING vec0(
                memory_id TEXT PRIMARY KEY,
                embedding float[384] distance_metric=cosine
            )",
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
