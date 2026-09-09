//! SQLite backend — split out of the former monolithic `store.rs`.
//!
//! `SqliteStore` and the shared row/parse helpers live in `super`
//! (`store/mod.rs`); each submodule here holds one trait impl (or a
//! coherent group of inherent methods) on that type.

use super::*;
use rusqlite::OptionalExtension;

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

// MemoirStore impl

impl MemoirStore for SqliteStore {
    // Memoir CRUD

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

    // Concept CRUD

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

    // Concept Search

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
        // Search JSON labels column using LIKE with the serialized label pattern.
        // namespace/value come from the caller (MCP tool args) and can contain
        // %/_ ; escape them so they can't turn into unintended wildcards.
        let pattern = format!(
            "%\"namespace\":\"{}\"%\"value\":\"{}\"%",
            escape_like_wildcards(&label.namespace),
            escape_like_wildcards(&label.value)
        );

        let sql = format!(
            "SELECT {CONCEPT_COLS} FROM concepts
             WHERE memoir_id = ?1 AND labels LIKE ?2 ESCAPE '\\'
             ORDER BY confidence DESC
             LIMIT ?3"
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params![memoir_id, pattern, limit as i64], row_to_concept)
            .map_err(db_err)?;

        collect_rows(rows)
    }

    // Refinement

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

    // Graph

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
                if !visited.contains(&link.target_id)
                    && let Some(c) = self.get_concept(&link.target_id)?
                {
                    visited.insert(c.id.clone());
                    queue.push_back((c.id.clone(), current_depth + 1));
                    concepts.push(c);
                }
                links.push(link);
            }

            // Incoming links
            let incoming = self.get_links_to(&current_id)?;
            for link in incoming {
                if !visited.contains(&link.source_id)
                    && let Some(c) = self.get_concept(&link.source_id)?
                {
                    visited.insert(c.id.clone());
                    queue.push_back((c.id.clone(), current_depth + 1));
                    concepts.push(c);
                }
                links.push(link);
            }
        }

        Ok((concepts, links))
    }

    // Stats

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
