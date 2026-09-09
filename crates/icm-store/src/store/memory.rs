//! SQLite backend — split out of the former monolithic `store.rs`.
//!
//! `SqliteStore` and the shared row/parse helpers live in `super`
//! (`store/mod.rs`); each submodule here holds one trait impl (or a
//! coherent group of inherent methods) on that type.

use super::*;
use rusqlite::OptionalExtension;

impl MemoryStore for SqliteStore {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        let memory = validate_and_normalize(memory)?;

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
        // Same constraints as `store()` — without this, oversized or
        // NUL-carrying payloads could bypass validation by storing small
        // then updating big (audit finding).
        validate_fields(&memory.topic, &memory.summary)?;

        let keywords_json = serde_json::to_string(&memory.keywords)?;
        let related_json = serde_json::to_string(&memory.related_ids)?;
        let st = source_type(&memory.source);
        let sd = source_data(&memory.source);
        let emb_blob = memory.embedding.as_deref().map(embedding_to_blob);

        // Recompute summary_hash on update — topic or summary may have
        // changed, and the partial unique index on (topic, summary_hash)
        // would otherwise reflect stale state.
        let hash = summary_hash(&memory.topic, &memory.summary);

        // memories + vec_memories must move together: a failure between the
        // row update and the vector sync would leave a memory invisible to
        // (or stale in) vector search (audit finding — same pattern as
        // `store()` / `consolidate_topic`).
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        let result: IcmResult<()> = (|| {
            let changed = self
                .conn
                .execute(
                    "UPDATE memories SET
                     updated_at = ?2, last_accessed = ?3, access_count = ?4, weight = ?5,
                     topic = ?6, summary = ?7, raw_excerpt = ?8, keywords = ?9,
                     importance = ?10, source_type = ?11, source_data = ?12, related_ids = ?13,
                     embedding = ?14, summary_hash = ?15
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
                        hash,
                    ],
                )
                .map_err(db_err)?;

            if changed == 0 {
                return Err(IcmError::NotFound(memory.id.clone()));
            }

            // Sync vec_memories: always delete old, re-insert if embedding exists
            self.conn
                .execute(
                    "DELETE FROM vec_memories WHERE memory_id = ?1",
                    params![memory.id],
                )
                .map_err(db_err)?;
            if let Some(ref blob) = emb_blob {
                self.conn
                    .execute(
                        "INSERT INTO vec_memories (memory_id, embedding) VALUES (?1, ?2)",
                        params![memory.id, blob],
                    )
                    .map_err(db_err)?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;").map_err(db_err)?;
                self.cache_invalidate(&memory.id);
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn delete(&self, id: &str) -> IcmResult<()> {
        // Both deletes in one transaction so a failure can't strand an
        // orphaned vector or a memory whose vector is gone (audit finding).
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        let result: IcmResult<()> = (|| {
            self.conn
                .execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id])
                .map_err(db_err)?;

            let changed = self
                .conn
                .execute("DELETE FROM memories WHERE id = ?1", params![id])
                .map_err(db_err)?;

            if changed == 0 {
                return Err(IcmError::NotFound(id.to_string()));
            }

            // Manual-testing finding: deleting a memory left it as a
            // dangling entry in every other memory's `related_ids`
            // (auto-link back-references) forever — `expand_with_neighbors`
            // tolerates the miss silently, but each stale id still spends a
            // slot out of the caller's `max_neighbors` budget instead of
            // surfacing a real, live neighbor, and any external consumer of
            // the JSON export sees a reference to nothing. Strip the
            // deleted id from every `related_ids` array that mentions it.
            // The `LIKE` clause is a cheap prefilter (only rows that could
            // possibly match do the JSON rewrite); it matches the
            // JSON-quoted form specifically so a ULID that happens to be a
            // literal substring of another can't cause a false hit.
            self.conn
                .execute(
                    "UPDATE memories
                        SET related_ids = (
                            SELECT COALESCE(json_group_array(value), '[]')
                            FROM json_each(memories.related_ids)
                            WHERE value != ?1
                        )
                        WHERE related_ids LIKE '%\"' || ?1 || '\"%'",
                    params![id],
                )
                .map_err(db_err)?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;").map_err(db_err)?;
                // The related_ids cleanup above can touch an arbitrary
                // number of other rows, not just `id` — clear the whole
                // cache rather than tracking which ones, so a cached
                // neighbor's `related_ids` can't keep serving the
                // just-deleted id after this returns.
                self.cache_clear();
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }

        // Cap keywords to avoid massive SQL generation
        let keywords = &keywords[..keywords.len().min(50)];
        let limit = limit.min(100);

        // Audit finding: a keyword containing `%` or `_` was interpolated
        // straight into the LIKE pattern unescaped. `%` matches every row
        // (`"100%"` as a keyword becomes the pattern `%100%%%`, which
        // degrades to "contains 100" at best and can blow up matching);
        // `_` matches any single character (`"snake_case"` also matches
        // "snakeXcase"). Both are plausible keywords coming from an LLM via
        // MCP. Escape them and declare the escape character explicitly.
        let where_parts: Vec<String> = (0..keywords.len())
            .map(|i| {
                let p = i + 1;
                format!(
                    "(keywords LIKE ?{p} ESCAPE '\\' OR summary LIKE ?{p} ESCAPE '\\' \
                     OR topic LIKE ?{p} ESCAPE '\\')"
                )
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
            .map(|k| {
                Box::new(format!("%{}%", escape_like_wildcards(k)))
                    as Box<dyn rusqlite::types::ToSql>
            })
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
        // Candidate-pool sizing is deliberately decoupled from `limit`
        // once `limit` gets large: a caller widens `limit` well past what
        // it actually wants back when a topic/project/keyword filter is
        // about to run post-hoc (headroom for candidates the filter will
        // drop — see `recall_query_limit` in icm-cli). Scaling `pool_size`
        // with that inflated `limit` doesn't just fetch more candidates to
        // filter, it changes the blended score of candidates ALREADY in
        // range: `vec_scores`/`fts_scores` default a missing side to 0.0,
        // so a candidate outside the (small) vector pool but inside the
        // (now much larger) FTS pool used to score `0.3*fts + 0.7*0.0`;
        // widening the vector pool to match gives it a real, usually
        // higher, vector score too — shifting rankings among candidates
        // that were already well-scored. Measured on a LoCoMo benchmark
        // pilot: pool_size scaled with an inflated limit=50 dropped
        // recall@5 from 56.2% (unscaled) to 52.7%/49.1% under looser caps;
        // pinning the pool-sizing input to a small constant (5) held it at
        // 56.2% while a topic-filtered query still got its full headroom
        // via the unmodified final `truncate(limit)` below.
        let pool_size = limit.clamp(1, 5) * 4;
        // OR-joined (not `sanitize_fts_query`'s AND): a multi-word natural-
        // language question needs only one distinctive shared word with a
        // candidate to contribute a BM25 signal alongside the vector score.
        // See `sanitize_fts_query_any`'s docs.
        let sanitized = sanitize_fts_query_any(query);

        // 1. Get FTS results with rank scores
        let fts_sql = "SELECT m.id, m.created_at, m.updated_at, m.last_accessed, m.access_count, m.weight, \
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

        if !sanitized.is_empty()
            && let Ok(mut stmt) = self.conn.prepare(fts_sql)
            && let Ok(rows) = stmt.query_map(params![sanitized, pool_size as i64], |row| {
                let memory = row_to_memory(row)?;
                let rank: f32 = row.get(15)?;
                Ok((memory, rank))
            })
        {
            for row in rows.flatten() {
                let (memory, rank) = row;
                // FTS5 bm25 rank is <= 0, MORE negative = MORE
                // relevant. `1.0 / (1.0 + |rank|)` inverted this: it
                // DECREASES as relevance increases (audit finding,
                // proven wrong e.g. rank=-4.83 (strong match) scored
                // 0.17 while rank=-0.2 (weak match) scored 0.83).
                // `|rank| / (1.0 + |rank|)` keeps the same bounded
                // [0,1) shape but is correctly monotonically
                // INCREASING in relevance.
                let score = rank.abs() / (1.0 + rank.abs());
                fts_scores.insert(memory.id.clone(), score);
                all_memories.insert(memory.id.clone(), memory);
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
        // Read-only short-circuit (issue #263): callers of recall expect
        // this to be best-effort bookkeeping, not a hard precondition.
        // Skipping silently lets `icm recall` work against a DB the
        // process cannot write to.
        if self.readonly {
            return Ok(());
        }
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
        if self.readonly {
            // Same rationale as `update_access` (issue #263).
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
        if self.readonly {
            return Err(IcmError::ReadOnly("apply_decay".into()));
        }
        // Access-aware decay: frequently accessed memories decay slower.
        // decay = base_rate * importance_multiplier / (1 + min(access_count, 5) * 0.1)
        //
        // Audit #185 H7: the access-count term used to be uncapped
        // (`1 + access_count * 0.1`). A memory with `access_count=100`
        // got a 11x slowdown on its decay, which made it effectively
        // immune to pruning even at low importance. Anyone (or any
        // bench loop, or any benign hook-driven recall pattern) that
        // touched a memory many times pinned it near the top of the
        // ranking forever — the same gaming class as the M01 issue
        // the maintainer flagged earlier.
        //
        // Cap at 5 accesses → max 1.5x slowdown (33%). That preserves
        // the original intent ("useful memories decay a bit slower")
        // without giving any single memory infinite decay immunity.
        // Critical-importance memories still skip decay entirely.
        //
        // Importance multipliers:
        //   critical: never decays (filtered by WHERE clause)
        //   high:     0.5x decay (half speed)
        //   medium:   1.0x decay (normal)
        //   low:      2.0x decay (double speed)
        // Audit finding: for `low` importance (2x multiplier) with a
        // low-access memory, the multiplier `1.0 - (1.0-f)*mult/denom` goes
        // NEGATIVE once `f < 0.5` — `icm decay --factor 0.4` (accepted by
        // the CLI's own `[0.0, 1.0)` validation) drove low-importance
        // weights negative, putting them last in every `ORDER BY weight
        // DESC` and prunable on the next pass. `MAX(0.0, ...)` clamps the
        // multiplier at the SQL layer so weight can never go negative
        // regardless of the caller (CLI, MCP, or any future direct caller
        // that bypasses the CLI's own boundary check).
        let changed = self
            .conn
            .execute(
                "UPDATE memories SET weight = weight * MAX(0.0,
                    1.0 - (1.0 - ?1) *
                    CASE importance
                        WHEN 'high' THEN 0.5
                        WHEN 'low' THEN 2.0
                        ELSE 1.0
                    END
                    / (1.0 + MIN(access_count, 5) * 0.1)
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
        // Never prune critical or high importance memories. Both deletes in
        // one transaction, and the vec_memories error is propagated instead
        // of swallowed — a partial prune would leave orphaned vectors that
        // keep matching KNN search for rows that no longer exist (audit
        // finding).
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        let result: IcmResult<usize> = (|| {
            self.conn.execute(
                "DELETE FROM vec_memories WHERE memory_id IN (
                    SELECT id FROM memories WHERE weight < ?1 AND importance NOT IN ('critical', 'high')
                )",
                params![weight_threshold],
            )
            .map_err(db_err)?;

            self.conn
                .execute(
                    "DELETE FROM memories WHERE weight < ?1 AND importance NOT IN ('critical', 'high')",
                    params![weight_threshold],
                )
                .map_err(db_err)
        })();

        match result {
            Ok(changed) => {
                self.conn.execute_batch("COMMIT;").map_err(db_err)?;
                if changed > 0 {
                    self.cache_clear();
                }
                Ok(changed)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
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
        // POW-3: Removed the previous LIMIT 10000 — `list_all` must return
        // every row for `icm export` to produce a complete backup. A silent
        // truncation at 10 000 records would cause unnoticed data loss during
        // disaster recovery.
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SELECT_COLS} FROM memories ORDER BY weight DESC"
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
        // The consolidated memory goes through the same validation as any
        // other write — MCP `icm_memory_consolidate` passes a caller-provided
        // summary that previously bypassed every size/content check.
        let consolidated = validate_and_normalize(consolidated)?;

        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        // Manual-testing finding: captured before the delete below, since
        // afterward the rows (and thus this query) are gone. Used to clean
        // up any *other* memory's related_ids that pointed at these —
        // same dangling-reference bug already fixed for the single-id
        // `delete`, reachable here too since this is a second, separate
        // bulk-delete code path.
        let deleted_ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM memories WHERE topic = ?1 AND importance != 'critical'")
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![topic], |row| row.get::<_, String>(0))
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)?
        };

        // `critical` memories are never deleted — same contract as
        // `apply_decay` and `prune`. Consolidation replaces the expendable
        // tail of a topic, not its "never forget" entries (audit finding:
        // this DELETE previously wiped critical memories too).
        // Clean vec_memories for entries about to be deleted
        if let Err(e) = self.conn.execute(
            "DELETE FROM vec_memories WHERE memory_id IN (
                SELECT id FROM memories WHERE topic = ?1 AND importance != 'critical'
            )",
            params![topic],
        ) {
            tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after vec_memories delete failed");
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(IcmError::Database(e.to_string()));
        }

        if let Err(e) = self.conn.execute(
            "DELETE FROM memories WHERE topic = ?1 AND importance != 'critical'",
            params![topic],
        ) {
            tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after memories delete failed");
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(IcmError::Database(e.to_string()));
        }

        if !deleted_ids.is_empty() {
            let ids_json = serde_json::to_string(&deleted_ids).map_err(IcmError::from)?;
            if let Err(e) = self.conn.execute(
                "UPDATE memories
                    SET related_ids = (
                        SELECT COALESCE(json_group_array(value), '[]')
                        FROM json_each(memories.related_ids)
                        WHERE value NOT IN (SELECT value FROM json_each(?1))
                    )
                    WHERE EXISTS (
                        SELECT 1 FROM json_each(memories.related_ids)
                        WHERE value IN (SELECT value FROM json_each(?1))
                    )",
                params![ids_json],
            ) {
                tracing::warn!(topic, error = %e, "consolidate_topic: rolling back after related_ids cleanup failed");
                let _ = self.conn.execute_batch("ROLLBACK;");
                return Err(IcmError::Database(e.to_string()));
            }
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
