//! PostgreSQL backend -- split out of the former monolithic postgres.rs.

use super::*;

// MemoryStore

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
        let mut tx = c.transaction().map_err(pg_err)?;
        let changed = tx
            .execute("DELETE FROM memories WHERE id = $1", &[&id])
            .map_err(pg_err)?;
        if changed == 0 {
            return Err(IcmError::NotFound(id.to_string()));
        }
        // Manual-testing finding (same class as the SQLite backend): a
        // deleted memory otherwise stays as a dangling entry in every
        // other memory's `related_ids` forever. Strip it out. `LIKE` is a
        // cheap prefilter; the JSON-quoted match guards against a ULID
        // that happens to be a literal substring of another.
        tx.execute(
            "UPDATE memories
                SET related_ids = (
                    SELECT COALESCE(jsonb_agg(elem), '[]'::jsonb)::text
                    FROM jsonb_array_elements_text(related_ids::jsonb) AS elem
                    WHERE elem != $1
                )
                WHERE related_ids LIKE '%\"' || $1 || '\"%'",
            &[&id],
        )
        .map_err(pg_err)?;
        tx.commit().map_err(pg_err)?;
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
            // Audit finding: a keyword containing `%`/`_` was interpolated
            // straight into the ILIKE pattern unescaped (same bug already
            // fixed for SQLite's search_by_keywords). Escape and declare the
            // escape character explicitly.
            owned.push(Box::new(format!("%{}%", escape_like_wildcards(k))));
            let p = owned.len();
            where_parts.push(format!(
                "(keywords ILIKE ${p} ESCAPE '\\' OR summary ILIKE ${p} ESCAPE '\\' \
                 OR topic ILIKE ${p} ESCAPE '\\')"
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
        // Found while auditing OpenSearch's equivalent (which had no clamp
        // at all): this function was also missing one, unlike its sibling
        // search functions in this same file.
        let limit = limit.min(1000);
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
                // Audit finding: for `low` importance with low access count,
                // the raw multiplier goes negative once decay_factor < 0.5
                // (still inside the CLI's own validated [0.0, 1.0) range) —
                // the same bug already fixed for SQLite (GREATEST here is
                // Postgres's equivalent of SQLite's MAX). See store.rs
                // apply_decay for the full derivation.
                "UPDATE memories SET weight = weight * GREATEST(0.0,
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
        // The consolidated memory goes through the same validation as any
        // other write — an MCP-provided consolidate summary previously
        // bypassed every size/NUL check (same gap already closed on SQLite).
        let consolidated = validate_and_normalize(consolidated)?;
        let mut c = self.conn()?;
        let mut tx = c.transaction().map_err(pg_err)?;
        // Audit finding: `critical` memories are never deleted — same
        // contract `apply_decay`/`prune` already honor, and the same fix
        // already applied to SQLite's consolidate_topic. This unconditional
        // DELETE previously wiped critical memories in a consolidated topic
        // too.
        // Manual-testing finding: captured before the delete, since
        // afterward these rows are gone. Used to clean up any *other*
        // memory's related_ids that pointed at them — same dangling-
        // reference bug already fixed for the single-id `delete`.
        let deleted_ids: Vec<String> = tx
            .query(
                "SELECT id FROM memories WHERE topic = $1 AND importance <> 'critical'",
                &[&topic],
            )
            .map_err(pg_err)?
            .iter()
            .map(|row| row.get(0))
            .collect();

        tx.execute(
            "DELETE FROM memories WHERE topic = $1 AND importance <> 'critical'",
            &[&topic],
        )
        .map_err(pg_err)?;

        if !deleted_ids.is_empty() {
            tx.execute(
                "UPDATE memories
                    SET related_ids = (
                        SELECT COALESCE(jsonb_agg(elem), '[]'::jsonb)::text
                        FROM jsonb_array_elements_text(related_ids::jsonb) AS elem
                        WHERE elem <> ALL($1::text[])
                    )
                    WHERE related_ids::jsonb ?| $1::text[]",
                &[&deleted_ids],
            )
            .map_err(pg_err)?;
        }

        insert_or_merge_memory(&mut tx, &consolidated)?;
        tx.commit().map_err(pg_err)?;
        Ok(())
    }

    fn consolidate_ids(
        &self,
        topic: &str,
        read: &[icm_core::ReadMemory],
        consolidated: Memory,
    ) -> IcmResult<icm_core::Consolidated> {
        if self.readonly {
            return Err(IcmError::ReadOnly("consolidate_ids".into()));
        }
        // Same contract as the SQLite backend, in one transaction: the
        // listed memories are replaced only if each is still in the topic
        // and, where the caller passed what it read, unchanged. The rows
        // are locked (`FOR UPDATE`) so that holds until the delete.
        let consolidated = validate_and_normalize(consolidated)?;
        if topic.trim() != consolidated.topic {
            return Err(IcmError::InvalidInput(format!(
                "consolidation topic {topic:?} does not match the summary's topic {:?}",
                consolidated.topic
            )));
        }
        let topic = consolidated.topic.clone();
        let wanted: Vec<String> = read.iter().map(|r| r.id.clone()).collect();
        let mut c = self.conn()?;
        let mut tx = c.transaction().map_err(pg_err)?;
        let current: std::collections::HashMap<String, (String, String)> = tx
            .query(
                "SELECT id, importance, summary FROM memories \
                  WHERE topic = $1 AND id = ANY($2::text[]) FOR UPDATE",
                &[&topic, &wanted],
            )
            .map_err(pg_err)?
            .iter()
            .map(|row| (row.get(0), (row.get(1), row.get(2))))
            .collect();
        let mut changed = Vec::new();
        let mut deleted_ids: Vec<String> = Vec::new();
        for item in read.iter().filter(|r| r.id != consolidated.id) {
            match current.get(&item.id) {
                None => changed.push(item.id.clone()),
                Some((_, now)) if item.summary.as_ref().is_some_and(|was| was != now) => {
                    changed.push(item.id.clone())
                }
                Some((importance, _)) if importance != "critical" => {
                    deleted_ids.push(item.id.clone())
                }
                Some(_) => {}
            }
        }
        if !changed.is_empty() {
            // Dropping the transaction rolls it back.
            return Ok(icm_core::Consolidated::Stale { changed });
        }

        let mut removed = 0usize;
        if !deleted_ids.is_empty() {
            removed = tx
                .execute(
                    "DELETE FROM memories \
                      WHERE id = ANY($1::text[]) AND topic = $2 AND importance <> 'critical'",
                    &[&deleted_ids, &topic],
                )
                .map_err(pg_err)? as usize;
            tx.execute(
                "UPDATE memories
                    SET related_ids = (
                        SELECT COALESCE(jsonb_agg(elem), '[]'::jsonb)::text
                        FROM jsonb_array_elements_text(related_ids::jsonb) AS elem
                        WHERE elem <> ALL($1::text[])
                    )
                    WHERE related_ids::jsonb ?| $1::text[]",
                &[&deleted_ids],
            )
            .map_err(pg_err)?;
        }

        let written_id = insert_or_merge_memory(&mut tx, &consolidated)?;
        if written_id != consolidated.id {
            // Deduplicated onto an existing memory: it must be in this
            // topic, or the originals are gone and the summary is not here.
            let landed: String = tx
                .query_one("SELECT topic FROM memories WHERE id = $1", &[&written_id])
                .map_err(pg_err)?
                .get(0);
            if landed != topic {
                return Err(IcmError::InvalidInput(format!(
                    "an identical summary already exists in topic {landed:?} (memory \
                     {written_id}); nothing was consolidated"
                )));
            }
        }
        tx.commit().map_err(pg_err)?;
        Ok(icm_core::Consolidated::Replaced {
            removed,
            id: written_id,
        })
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

// Unsupported subsystems on this backend (first cut, issue #301).
//
// These return `IcmError::Unsupported` so the binary keeps working for the
// core shared-memory use case while the heavier subsystems remain on the
// default SQLite backend. A follow-up can port them.
