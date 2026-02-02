use std::collections::{HashSet, VecDeque};
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use icm_core::{
    Concept, ConceptLink, IcmError, IcmResult, Importance, Label, Memoir, MemoirStats, MemoirStore,
    Memory, MemorySource, MemoryStore, Relation, StoreStats,
};

use crate::schema::init_db;

pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    pub fn new(path: &Path) -> IcmResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| IcmError::Database(format!("cannot create db directory: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| IcmError::Database(format!("cannot open database: {e}")))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| IcmError::Database(e.to_string()))?;
        init_db(&conn)?;
        Ok(Self { conn })
    }

    pub fn in_memory() -> IcmResult<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| IcmError::Database(format!("cannot open in-memory db: {e}")))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| IcmError::Database(e.to_string()))?;
        init_db(&conn)?;
        Ok(Self { conn })
    }
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

fn row_to_memory(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    let keywords_json: String = row.get::<_, Option<String>>(8)?.unwrap_or_default();
    let keywords: Vec<String> = serde_json::from_str(&keywords_json).unwrap_or_default();

    let importance_str: String = row.get(9)?;
    let importance = importance_str.parse().unwrap_or(Importance::Medium);

    let source_type_str: String = row.get(10)?;
    let source_data_str: Option<String> = row.get(11)?;
    let source = parse_source(&source_type_str, source_data_str);

    let related_json: String = row.get::<_, Option<String>>(12)?.unwrap_or_default();
    let related_ids: Vec<String> = serde_json::from_str(&related_json).unwrap_or_default();

    let created_at_str: String = row.get(1)?;
    let last_accessed_str: String = row.get(2)?;

    Ok(Memory {
        id: row.get(0)?,
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_accessed: DateTime::parse_from_rfc3339(&last_accessed_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        access_count: row.get::<_, u32>(3)?,
        weight: row.get(4)?,
        topic: row.get(5)?,
        summary: row.get(6)?,
        raw_excerpt: row.get(7)?,
        keywords,
        importance,
        source,
        related_ids,
    })
}

const SELECT_COLS: &str = "id, created_at, last_accessed, access_count, weight, \
                           topic, summary, raw_excerpt, keywords, \
                           importance, source_type, source_data, related_ids";

// ---------------------------------------------------------------------------
// MemoryStore impl
// ---------------------------------------------------------------------------

impl MemoryStore for SqliteStore {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);

        self.conn
            .execute(
                "INSERT INTO memories (id, created_at, last_accessed, access_count, weight,
                 topic, summary, raw_excerpt, keywords,
                 importance, source_type, source_data, related_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    memory.id,
                    memory.created_at.to_rfc3339(),
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
                ],
            )
            .map_err(|e| IcmError::Database(e.to_string()))?;

        Ok(memory.id)
    }

    fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {SELECT_COLS} FROM memories WHERE id = ?1"))
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let result = stmt
            .query_row(params![id], row_to_memory)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))?;

        Ok(result)
    }

    fn update(&self, memory: &Memory) -> IcmResult<()> {
        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);

        let changed = self
            .conn
            .execute(
                "UPDATE memories SET
                 last_accessed = ?2, access_count = ?3, weight = ?4,
                 topic = ?5, summary = ?6, raw_excerpt = ?7, keywords = ?8,
                 importance = ?9, source_type = ?10, source_data = ?11, related_ids = ?12
                 WHERE id = ?1",
                params![
                    memory.id,
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
                ],
            )
            .map_err(|e| IcmError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(IcmError::NotFound(memory.id.clone()));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| IcmError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }

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

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = keywords
            .iter()
            .map(|k| Box::new(format!("%{k}%")) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        param_values.push(Box::new(limit as i64));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_ref.as_slice(), row_to_memory)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>> {
        if query.trim().is_empty() {
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

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![query, limit as i64], row_to_memory)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
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
            .map_err(|e| IcmError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize> {
        let changed = self
            .conn
            .execute(
                "UPDATE memories SET weight = weight * ?1 WHERE importance != 'critical'",
                params![decay_factor],
            )
            .map_err(|e| IcmError::Database(e.to_string()))?;

        Ok(changed)
    }

    fn prune(&self, weight_threshold: f32) -> IcmResult<usize> {
        let changed = self
            .conn
            .execute(
                "DELETE FROM memories WHERE weight < ?1 AND importance != 'critical'",
                params![weight_threshold],
            )
            .map_err(|e| IcmError::Database(e.to_string()))?;

        Ok(changed)
    }

    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM memories WHERE topic = ?1 ORDER BY weight DESC"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![topic], row_to_memory)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT topic, COUNT(*) FROM memories GROUP BY topic ORDER BY topic")
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()> {
        self.conn
            .execute("DELETE FROM memories WHERE topic = ?1", params![topic])
            .map_err(|e| IcmError::Database(e.to_string()))?;

        self.store(consolidated)?;
        Ok(())
    }

    fn count(&self) -> IcmResult<usize> {
        self.conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn stats(&self) -> IcmResult<StoreStats> {
        let total_memories = self.count()?;

        let total_topics: usize = self
            .conn
            .query_row("SELECT COUNT(DISTINCT topic) FROM memories", [], |row| {
                row.get(0)
            })
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let avg_weight: f32 = if total_memories > 0 {
            self.conn
                .query_row("SELECT AVG(weight) FROM memories", [], |row| row.get(0))
                .map_err(|e| IcmError::Database(e.to_string()))?
        } else {
            0.0
        };

        let oldest_memory: Option<DateTime<Utc>> = self
            .conn
            .query_row("SELECT MIN(created_at) FROM memories", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(|e| IcmError::Database(e.to_string()))?
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc));

        let newest_memory: Option<DateTime<Utc>> = self
            .conn
            .query_row("SELECT MAX(created_at) FROM memories", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(|e| IcmError::Database(e.to_string()))?
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
            .map_err(|e| IcmError::Database(e.to_string()))?;
        Ok(memoir.id)
    }

    fn get_memoir(&self, id: &str) -> IcmResult<Option<Memoir>> {
        self.conn
            .prepare(&format!("SELECT {MEMOIR_COLS} FROM memoirs WHERE id = ?1"))
            .map_err(|e| IcmError::Database(e.to_string()))?
            .query_row(params![id], row_to_memoir)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn get_memoir_by_name(&self, name: &str) -> IcmResult<Option<Memoir>> {
        self.conn
            .prepare(&format!(
                "SELECT {MEMOIR_COLS} FROM memoirs WHERE name = ?1"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?
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
            .map_err(|e| IcmError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(IcmError::NotFound(memoir.id.clone()));
        }
        Ok(())
    }

    fn delete_memoir(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM memoirs WHERE id = ?1", params![id])
            .map_err(|e| IcmError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        Ok(())
    }

    fn list_memoirs(&self) -> IcmResult<Vec<Memoir>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {MEMOIR_COLS} FROM memoirs ORDER BY name"))
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], row_to_memoir)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
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
            .map_err(|e| IcmError::Database(e.to_string()))?;
        Ok(concept.id)
    }

    fn get_concept(&self, id: &str) -> IcmResult<Option<Concept>> {
        self.conn
            .prepare(&format!(
                "SELECT {CONCEPT_COLS} FROM concepts WHERE id = ?1"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?
            .query_row(params![id], row_to_concept)
            .optional()
            .map_err(|e| IcmError::Database(e.to_string()))
    }

    fn get_concept_by_name(&self, memoir_id: &str, name: &str) -> IcmResult<Option<Concept>> {
        self.conn
            .prepare(&format!(
                "SELECT {CONCEPT_COLS} FROM concepts WHERE memoir_id = ?1 AND name = ?2"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?
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
            .map_err(|e| IcmError::Database(e.to_string()))?;

        if changed == 0 {
            return Err(IcmError::NotFound(concept.id.clone()));
        }
        Ok(())
    }

    fn delete_concept(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM concepts WHERE id = ?1", params![id])
            .map_err(|e| IcmError::Database(e.to_string()))?;

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
                "SELECT {CONCEPT_COLS} FROM concepts WHERE memoir_id = ?1 ORDER BY name"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![memoir_id], row_to_concept)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    fn search_concepts_fts(
        &self,
        memoir_id: &str,
        query: &str,
        limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT {CONCEPT_COLS} FROM concepts
             WHERE memoir_id = ?1
               AND id IN (SELECT id FROM concepts_fts WHERE concepts_fts MATCH ?2)
             ORDER BY confidence DESC
             LIMIT ?3"
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![memoir_id, query, limit as i64], row_to_concept)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
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

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![memoir_id, pattern, limit as i64], row_to_concept)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
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
            .map_err(|e| IcmError::Database(e.to_string()))?;

        Ok(())
    }

    // --- Graph ---

    fn add_link(&self, link: ConceptLink) -> IcmResult<String> {
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
            .map_err(|e| IcmError::Database(e.to_string()))?;
        Ok(link.id)
    }

    fn get_links_from(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {LINK_COLS} FROM concept_links WHERE source_id = ?1"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![concept_id], row_to_link)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    fn get_links_to(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {LINK_COLS} FROM concept_links WHERE target_id = ?1"
            ))
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = stmt
            .query_map(params![concept_id], row_to_link)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
    }

    fn delete_link(&self, id: &str) -> IcmResult<()> {
        let changed = self
            .conn
            .execute("DELETE FROM concept_links WHERE id = ?1", params![id])
            .map_err(|e| IcmError::Database(e.to_string()))?;

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

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let rows = if relation.is_some() {
            stmt.query_map(params![concept_id, p_relation], row_to_concept)
                .map_err(|e| IcmError::Database(e.to_string()))?
        } else {
            stmt.query_map(params![concept_id], row_to_concept)
                .map_err(|e| IcmError::Database(e.to_string()))?
        };

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| IcmError::Database(e.to_string()))?);
        }
        Ok(results)
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
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let total_links: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM concept_links
                 WHERE source_id IN (SELECT id FROM concepts WHERE memoir_id = ?1)",
                params![memoir_id],
                |row| row.get(0),
            )
            .map_err(|e| IcmError::Database(e.to_string()))?;

        let avg_confidence: f32 = if total_concepts > 0 {
            self.conn
                .query_row(
                    "SELECT AVG(confidence) FROM concepts WHERE memoir_id = ?1",
                    params![memoir_id],
                    |row| row.get(0),
                )
                .map_err(|e| IcmError::Database(e.to_string()))?
        } else {
            0.0
        };

        // Count distinct label namespace:value pairs
        let concepts = self.list_concepts(memoir_id)?;
        let mut label_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for c in &concepts {
            for l in &c.labels {
                *label_map.entry(l.to_string()).or_insert(0) += 1;
            }
        }
        let mut label_counts: Vec<(String, usize)> = label_map.into_iter().collect();
        label_counts.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(MemoirStats {
            total_concepts,
            total_links,
            avg_confidence,
            label_counts,
        })
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
}
