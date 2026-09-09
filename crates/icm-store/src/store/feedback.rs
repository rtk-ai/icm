//! SQLite backend — split out of the former monolithic `store.rs`.
//!
//! `SqliteStore` and the shared row/parse helpers live in `super`
//! (`store/mod.rs`); each submodule here holds one trait impl (or a
//! coherent group of inherent methods) on that type.

use super::*;

fn row_to_feedback(row: &rusqlite::Row) -> rusqlite::Result<Feedback> {
    let embedding: Option<Vec<f32>> = row
        .get::<_, Option<Vec<u8>>>(9)?
        .map(|b| blob_to_embedding(&b));
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
        embedding,
    })
}

const FEEDBACK_COLS: &str = "id, topic, context, predicted, corrected, reason, source, created_at, applied_count, embedding";
/// Same columns, qualified for the `feedback f JOIN feedback_fts fts` query
/// in `search_feedback` — both tables have an `id` column post-join, so the
/// unqualified `FEEDBACK_COLS` is ambiguous there (found via real testing:
/// `search_feedback` errored on every call once the join was introduced).
const FEEDBACK_COLS_F: &str = "f.id, f.topic, f.context, f.predicted, f.corrected, f.reason, \
     f.source, f.created_at, f.applied_count, f.embedding";

// FeedbackStore impl

/// Pure Rust cosine similarity for the feedback semantic fallback (no
/// vec0 virtual table — feedback volume is expected far lower than
/// memories, so a brute-force scan over rows with an embedding is simpler
/// and avoids replicating the vec0/dimension-migration machinery for a
/// low-cardinality table). A dimension mismatch returns -1.0 (below any
/// real cosine similarity) rather than silently comparing a truncated
/// prefix — same reasoning as `extract_semantic.rs`'s `cosine`.
fn feedback_cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return -1.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

impl FeedbackStore for SqliteStore {
    fn store_feedback(&self, feedback: Feedback) -> IcmResult<String> {
        let embedding_blob = feedback.embedding.as_deref().map(embedding_to_blob);
        self.conn
            .execute(
                "INSERT INTO feedback (id, topic, context, predicted, corrected, reason, source, created_at, applied_count, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                    embedding_blob,
                ],
            )
            .map_err(db_err)?;
        Ok(feedback.id)
    }

    fn search_feedback(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        topic: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<Feedback>> {
        let sanitized = sanitize_fts_query(query);

        if sanitized.is_empty() {
            return self.list_feedback(topic, limit);
        }

        let pool = limit.saturating_mul(4).max(limit);

        // FTS candidates, ranked (FTS5 bm25: more negative = more relevant).
        let (fts_sql, fts_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(t) = topic {
                (
                    format!(
                        "SELECT {FEEDBACK_COLS_F}, fts.rank as rnk FROM feedback f
                         JOIN feedback_fts fts ON fts.id = f.id
                         WHERE feedback_fts MATCH ?1 AND f.topic = ?2
                         ORDER BY fts.rank LIMIT ?3"
                    ),
                    vec![
                        Box::new(sanitized.clone()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(t.to_string()),
                        Box::new(pool as i64),
                    ],
                )
            } else {
                (
                    format!(
                        "SELECT {FEEDBACK_COLS_F}, fts.rank as rnk FROM feedback f
                         JOIN feedback_fts fts ON fts.id = f.id
                         WHERE feedback_fts MATCH ?1
                         ORDER BY fts.rank LIMIT ?2"
                    ),
                    vec![
                        Box::new(sanitized.clone()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(pool as i64),
                    ],
                )
            };

        let mut all: HashMap<String, Feedback> = HashMap::new();
        let mut fts_scores: HashMap<String, f32> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(&fts_sql).map_err(db_err)?;
            let refs: Vec<&dyn rusqlite::types::ToSql> =
                fts_params.iter().map(|p| p.as_ref()).collect();
            let col_count = 10; // FEEDBACK_COLS (10) + rnk
            let rows = stmt
                .query_map(refs.as_slice(), |row| {
                    let fb = row_to_feedback(row)?;
                    let rank: f32 = row.get(col_count)?;
                    Ok((fb, rank))
                })
                .map_err(db_err)?;
            for row in rows.flatten() {
                let (fb, rank) = row;
                // Same monotonic-increasing-in-relevance transform as
                // memories' search_hybrid (audit finding there: the naive
                // `1.0 / (1.0 + |rank|)` inverts the relationship).
                let score = rank.abs() / (1.0 + rank.abs());
                fts_scores.insert(fb.id.clone(), score);
                all.insert(fb.id.clone(), fb);
            }
        }

        // Semantic candidates: brute-force cosine over rows with an
        // embedding (see feedback_cosine's doc comment for why no vec0).
        let mut vec_scores: HashMap<String, f32> = HashMap::new();
        if let Some(qemb) = query_embedding {
            let topic_sql = if topic.is_some() {
                " WHERE topic = ?1"
            } else {
                ""
            };
            let sql = format!("SELECT {FEEDBACK_COLS} FROM feedback{topic_sql}");
            let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
            let candidates: Vec<Feedback> = if let Some(t) = topic {
                collect_rows(
                    stmt.query_map(params![t], row_to_feedback)
                        .map_err(db_err)?,
                )?
            } else {
                collect_rows(stmt.query_map([], row_to_feedback).map_err(db_err)?)?
            };
            for fb in candidates {
                if let Some(emb) = &fb.embedding {
                    let sim = feedback_cosine(qemb, emb);
                    if sim > 0.0 {
                        vec_scores.insert(fb.id.clone(), sim);
                        all.entry(fb.id.clone()).or_insert(fb);
                    }
                }
            }
        }

        let mut scored: Vec<(String, f32)> = all
            .keys()
            .map(|id| {
                let fts = fts_scores.get(id).copied().unwrap_or(0.0);
                let vec = vec_scores.get(id).copied().unwrap_or(0.0);
                (id.clone(), 0.3 * fts + 0.7 * vec)
            })
            .collect();
        // Pure-FTS fallback (no query embedding available) must still rank
        // by relevance, not just whatever HashMap order `all` iterates in.
        if query_embedding.is_none() {
            scored = fts_scores.iter().map(|(id, s)| (id.clone(), *s)).collect();
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .filter_map(|(id, _)| all.remove(&id))
            .collect())
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
