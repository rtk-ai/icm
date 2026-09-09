//! OpenSearch backend -- split out of the former monolithic opensearch.rs.

use super::*;

impl OpenSearchStore {
    pub(crate) fn refresh_param(&self) -> &'static str {
        // Force a refresh so writes are immediately visible to subsequent
        // searches (dedup, counts, the multi-replica path). ICM writes are
        // low-frequency curated memories, so the cost is acceptable.
        "refresh=true"
    }

    pub(crate) fn store_inner(&self, memory: &Memory) -> IcmResult<String> {
        let hash = summary_hash(&memory.topic, &memory.summary);
        // Dedup: an existing memory with the same (topic, summary_hash)
        // wins; merge importance (max) + keywords (union) + raw_excerpt
        // (prefer new) and return the existing id.
        //
        // Audit finding: this used to ALSO filter on `topic.keyword` (an
        // exact-byte-match `keyword` field, no normalizer) alongside
        // `summary_hash` — but `summary_hash` already encodes the topic via
        // Rust's Unicode-correct `to_lowercase()`, so storing topic="Kexa"
        // then topic="kexa" with the same summary hashed identically but
        // failed the exact `topic.keyword` filter, silently creating a
        // second document instead of deduping (broader than the SQLite/
        // Postgres accented-topic case — this fires on ANY case
        // difference). `summary_hash` alone is sufficient, matching the
        // SQLite/Postgres fix.
        let existing = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 1,
                "query": {"bool": {"filter": [
                    {"term": {"summary_hash": hash}}
                ]}}
            }),
        )?;
        if let Some(hit) = existing
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .and_then(|a| a.first())
        {
            let existing_id = hit
                .get("_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let src = hit.get("_source").cloned().unwrap_or(Value::Null);
            let existing_importance: Importance = src
                .get("importance")
                .and_then(|v| v.as_str())
                .unwrap_or("medium")
                .parse()
                .unwrap_or(Importance::Medium);
            let merged_importance = max_importance(existing_importance, memory.importance);
            let mut merged_keywords: Vec<String> = src
                .get("keywords")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            for kw in &memory.keywords {
                if !merged_keywords.contains(kw) {
                    merged_keywords.push(kw.clone());
                }
            }
            let raw = memory.raw_excerpt.clone().or_else(|| {
                src.get("raw_excerpt")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
            self.request(
                "POST",
                &format!(
                    "{IDX_MEMORIES}/_update/{existing_id}?{}",
                    self.refresh_param()
                ),
                Some(json!({"doc": {
                    "importance": merged_importance.to_string(),
                    "keywords": merged_keywords,
                    "raw_excerpt": raw,
                    "updated_at": Utc::now().to_rfc3339(),
                }})),
                false,
            )?;
            return Ok(existing_id);
        }

        self.request(
            "PUT",
            &format!(
                "{IDX_MEMORIES}/_doc/{}?{}",
                url_encode_path_segment(&memory.id),
                self.refresh_param()
            ),
            Some(Self::memory_to_source(memory)),
            false,
        )?;
        Ok(memory.id.clone())
    }
}

impl MemoryStore for OpenSearchStore {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        if self.readonly {
            return Err(IcmError::ReadOnly("store".into()));
        }
        let memory = validate_and_normalize(memory)?;
        self.check_dims(&memory)?;
        self.store_inner(&memory)
    }

    fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
        let path = format!("{IDX_MEMORIES}/_doc/{}", url_encode_path_segment(id));
        match self.get_json(&path)? {
            Some(v) => {
                if v.get("found").and_then(|f| f.as_bool()).unwrap_or(false) {
                    let src = v.get("_source").cloned().unwrap_or(Value::Null);
                    Ok(Some(Self::source_to_memory(id, &src)))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn update(&self, memory: &Memory) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("update".into()));
        }
        self.check_dims(memory)?;
        let mut doc = Self::memory_to_source(memory);
        doc["updated_at"] = json!(Utc::now().to_rfc3339());
        // Replace the document wholesale (index by id).
        self.request(
            "PUT",
            &format!(
                "{IDX_MEMORIES}/_doc/{}?{}",
                url_encode_path_segment(&memory.id),
                self.refresh_param()
            ),
            Some(doc),
            false,
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("delete".into()));
        }
        self.request(
            "DELETE",
            &format!(
                "{IDX_MEMORIES}/_doc/{}?{}",
                url_encode_path_segment(id),
                self.refresh_param()
            ),
            None,
            true,
        )?;

        // Manual-testing finding (same class as the SQLite/Postgres
        // backends): a deleted memory otherwise stays as a dangling entry
        // in every other memory's `related_ids` forever. `related_ids` is
        // mapped as a `keyword` array field, so a term query finds every
        // document that references it and a Painless script strips it out
        // in place. Best-effort: a failure here doesn't roll back the
        // delete above (OpenSearch has no cross-document transaction to
        // roll back into) — surfacing an error would make a successful
        // delete look like it failed, so log and move on.
        if let Err(e) = self.post(
            &format!(
                "{IDX_MEMORIES}/_update_by_query?conflicts=proceed&{}",
                self.refresh_param()
            ),
            json!({
                "script": {
                    "source": "ctx._source.related_ids.removeIf(x -> x == params.deleted_id)",
                    "params": {"deleted_id": id}
                },
                "query": {"term": {"related_ids": id}}
            }),
        ) {
            tracing::warn!(error = %e, id, "failed to clean up dangling related_ids after delete");
        }

        Ok(())
    }

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }
        // Audit finding: unlike Postgres/SQLite, `limit` was never clamped
        // here — a caller-supplied limit above OpenSearch's own
        // `index.max_result_window` (default 10,000) returns a hard 400
        // error instead of gracefully truncating like the other backends.
        let limit = limit.min(100);
        let joined = keywords.join(" ");
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": limit,
                "query": {"bool": {"should": [
                    {"terms": {"keywords": keywords}},
                    {"multi_match": {"query": joined, "fields": ["summary", "topic"]}}
                ], "minimum_should_match": 1}}
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.min(100);
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": limit,
                "query": {"multi_match": {
                    "query": query,
                    "fields": ["summary^2", "topic", "keywords"]
                }}
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn search_by_embedding(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let limit = limit.min(1000);
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": limit,
                "query": {"knn": {"embedding": {"vector": embedding, "k": limit}}}
            }),
        )?;
        Ok(Self::hits_to_scored(&resp))
    }

    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        let limit = limit.min(1000);
        let pool = limit * 4;

        // FTS candidates (BM25).
        let mut fts_scores: HashMap<String, f32> = HashMap::new();
        let mut memories: HashMap<String, Memory> = HashMap::new();
        if !query.trim().is_empty() {
            let resp = self.post(
                &format!("{IDX_MEMORIES}/_search"),
                json!({
                    "size": pool,
                    "query": {"multi_match": {"query": query, "fields": ["summary^2", "topic", "keywords"]}}
                }),
            )?;
            for (m, s) in Self::hits_to_scored(&resp) {
                fts_scores.insert(m.id.clone(), s);
                memories.insert(m.id.clone(), m);
            }
        }

        // Vector candidates.
        let mut vec_scores: HashMap<String, f32> = HashMap::new();
        for (m, s) in self.search_by_embedding(embedding, pool)? {
            vec_scores.insert(m.id.clone(), s);
            memories.entry(m.id.clone()).or_insert(m);
        }

        // Min-max normalize each score family to [0, 1] before blending.
        let norm = |scores: &HashMap<String, f32>| -> HashMap<String, f32> {
            if scores.is_empty() {
                return HashMap::new();
            }
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &v in scores.values() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let span = (hi - lo).max(f32::EPSILON);
            scores
                .iter()
                .map(|(k, v)| (k.clone(), (v - lo) / span))
                .collect()
        };
        let fts_n = norm(&fts_scores);
        let vec_n = norm(&vec_scores);

        let mut scored: Vec<(String, f32)> = memories
            .keys()
            .map(|id| {
                let f = fts_n.get(id).copied().unwrap_or(0.0);
                let v = vec_n.get(id).copied().unwrap_or(0.0);
                (id.clone(), 0.3 * f + 0.7 * v)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .filter_map(|(id, s)| memories.remove(&id).map(|m| (m, s)))
            .collect())
    }

    fn update_access(&self, id: &str) -> IcmResult<()> {
        if self.readonly {
            return Ok(());
        }
        // Best-effort; a missing doc is not an error for recall bookkeeping.
        let _ = self.request(
            "POST",
            &format!("{IDX_MEMORIES}/_update/{id}"),
            Some(json!({
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.access_count = (ctx._source.access_count == null ? 1 : ctx._source.access_count + 1); ctx._source.last_accessed = params.now;",
                    "params": {"now": Utc::now().to_rfc3339()}
                }
            })),
            true,
        )?;
        Ok(())
    }

    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize> {
        if self.readonly || ids.is_empty() {
            return Ok(0);
        }
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_update_by_query?{}&conflicts=proceed", self.refresh_param()),
            json!({
                "query": {"ids": {"values": ids}},
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.access_count = (ctx._source.access_count == null ? 1 : ctx._source.access_count + 1); ctx._source.last_accessed = params.now;",
                    "params": {"now": Utc::now().to_rfc3339()}
                }
            }),
        )?;
        Ok(resp.get("updated").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly("decay".into()));
        }
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_update_by_query?{}&conflicts=proceed", self.refresh_param()),
            json!({
                "query": {"bool": {"must_not": [{"term": {"importance": "critical"}}]}},
                // Audit finding: for `low` importance with low access count,
                // the raw multiplier goes negative once decay_factor < 0.5
                // (still inside the CLI's own validated [0.0, 1.0) range) —
                // same bug already fixed for SQLite/Postgres. Math.max is
                // Painless's equivalent clamp.
                "script": {
                    "lang": "painless",
                    "source": "double f = params.factor; String imp = ctx._source.importance; double mult = imp != null && imp.equals('high') ? 0.5 : (imp != null && imp.equals('low') ? 2.0 : 1.0); double ac = ctx._source.access_count == null ? 0 : ctx._source.access_count; if (ac > 5) ac = 5; double m = Math.max(0.0, 1.0 - (1.0 - f) * mult / (1.0 + ac * 0.1)); ctx._source.weight = ctx._source.weight * m;",
                    "params": {"factor": decay_factor as f64}
                }
            }),
        )?;
        Ok(resp.get("updated").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn prune(&self, weight_threshold: f32) -> IcmResult<usize> {
        if self.readonly {
            return Err(IcmError::ReadOnly("prune".into()));
        }
        let resp = self.post(
            &format!(
                "{IDX_MEMORIES}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({
                "query": {"bool": {
                    "must": [{"range": {"weight": {"lt": weight_threshold as f64}}}],
                    "must_not": [{"terms": {"importance": ["critical", "high"]}}]
                }}
            }),
        )?;
        Ok(resp.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 500,
                "query": {"term": {"topic.keyword": topic}},
                "sort": [{"weight": "desc"}]
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn list_all(&self) -> IcmResult<Vec<Memory>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({"size": 10000, "query": {"match_all": {}}, "sort": [{"weight": "desc"}]}),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({"size": 0, "aggs": {"topics": {"terms": {"field": "topic.keyword", "size": 10000}}}}),
        )?;
        let mut out = bucket_counts(&resp, "topics");
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("consolidate".into()));
        }
        // Audit findings, both fixed together:
        // 1. `critical` memories are never deleted — same contract
        //    apply_decay/prune already honor, and the same fix already
        //    applied to SQLite/Postgres consolidate_topic. This delete
        //    query previously wiped critical memories in the topic too.
        // 2. Still not atomic (no multi-document transaction in
        //    OpenSearch), but reordered to insert-then-delete: if
        //    `store_inner` fails (dimension mismatch, network blip,
        //    cluster unavailable), the originals are left untouched
        //    instead of being gone with no replacement ever written. The
        //    failure mode is now "harmless duplication a retry fixes",
        //    not data loss — the consolidated memory gets a fresh id, so
        //    it can't collide with any original.
        let consolidated = validate_and_normalize(consolidated)?;
        self.check_dims(&consolidated)?;
        // Manual-testing finding: captured before store_inner/delete below
        // (the deleted rows are gone afterward), so any *other* memory's
        // related_ids pointing at them can be cleaned up — same dangling-
        // reference bug already fixed for the single-id `delete`.
        let deleted_ids: Vec<String> = self
            .get_by_topic(topic)?
            .into_iter()
            .filter(|m| m.importance != Importance::Critical)
            .map(|m| m.id)
            .collect();
        // `store_inner` returns the id actually used — the caller's fresh
        // id on a normal insert, or an existing row's id if this exact
        // (topic, summary_hash) happened to already exist (dedup merge).
        // Either way it now has the SAME topic as the memories being
        // consolidated, so it must be excluded from the delete below or it
        // would delete the very memory it just wrote/merged into.
        let consolidated_id = self.store_inner(&consolidated)?;
        self.post(
            &format!(
                "{IDX_MEMORIES}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({"query": {"bool": {
                "must": [{"term": {"topic.keyword": topic}}],
                "must_not": [
                    {"term": {"importance": "critical"}},
                    {"ids": {"values": [consolidated_id]}}
                ]
            }}}),
        )?;

        if !deleted_ids.is_empty()
            && let Err(e) = self.post(
                &format!(
                    "{IDX_MEMORIES}/_update_by_query?conflicts=proceed&{}",
                    self.refresh_param()
                ),
                json!({
                    "script": {
                        "source": "ctx._source.related_ids.removeIf(x -> params.deleted_ids.contains(x))",
                        "params": {"deleted_ids": deleted_ids}
                    },
                    "query": {"terms": {"related_ids": deleted_ids}}
                }),
            ) {
                tracing::warn!(topic, error = %e, "consolidate_topic: failed to clean up dangling related_ids");
            }

        Ok(())
    }

    fn count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_MEMORIES}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn count_by_topic(&self, topic: &str) -> IcmResult<usize> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_count"),
            json!({"query": {"term": {"topic.keyword": topic}}}),
        )?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    fn stats(&self) -> IcmResult<StoreStats> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 0,
                "track_total_hits": true,
                "aggs": {
                    "avg_w": {"avg": {"field": "weight"}},
                    "topics": {"cardinality": {"field": "topic.keyword"}},
                    "oldest": {"min": {"field": "created_at", "format": "date_time"}},
                    "newest": {"max": {"field": "created_at", "format": "date_time"}}
                }
            }),
        )?;
        let total = resp
            .get("hits")
            .and_then(|h| h.get("total"))
            .and_then(|t| t.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let aggs = resp.get("aggregations").cloned().unwrap_or(Value::Null);
        let avg_weight = aggs
            .get("avg_w")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let total_topics = aggs
            .get("topics")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let parse_agg_date = |name: &str| -> Option<DateTime<Utc>> {
            aggs.get(name)
                .and_then(|a| a.get("value_as_string"))
                .and_then(|v| v.as_str())
                .map(parse_dt)
        };
        Ok(StoreStats {
            total_memories: total,
            total_topics,
            avg_weight,
            oldest_memory: parse_agg_date("oldest"),
            newest_memory: parse_agg_date("newest"),
        })
    }

    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 0,
                "track_total_hits": true,
                "query": {"term": {"topic.keyword": topic}},
                "aggs": {
                    "avg_w": {"avg": {"field": "weight"}},
                    "avg_ac": {"avg": {"field": "access_count"}},
                    "oldest": {"min": {"field": "created_at"}},
                    "newest": {"max": {"field": "created_at"}},
                    "last_acc": {"max": {"field": "last_accessed"}},
                    "stale": {"filter": {"bool": {"must": [
                        {"range": {"weight": {"lt": 0.5}}},
                        {"range": {"last_accessed": {"lt": "now-14d"}}}
                    ]}}}
                }
            }),
        )?;
        let entry_count = resp
            .get("hits")
            .and_then(|h| h.get("total"))
            .and_then(|t| t.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        if entry_count == 0 {
            return Err(IcmError::NotFound(format!(
                "no memories in topic '{topic}'"
            )));
        }
        let aggs = resp.get("aggregations").cloned().unwrap_or(Value::Null);
        let avg_weight = aggs
            .get("avg_w")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let avg_access_count = aggs
            .get("avg_ac")
            .and_then(|a| a.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let stale_count = aggs
            .get("stale")
            .and_then(|a| a.get("doc_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        Ok(TopicHealth {
            topic: topic.to_string(),
            entry_count,
            avg_weight,
            avg_access_count,
            oldest: agg_date(&aggs, "oldest"),
            newest: agg_date(&aggs, "newest"),
            last_accessed: agg_date(&aggs, "last_acc"),
            stale_count,
            needs_consolidation: entry_count > 5,
        })
    }
}

/// Read a `min`/`max` date aggregation into a `DateTime<Utc>`.
///
/// Prefers the ISO `value_as_string` OpenSearch returns and falls back to
/// the epoch-millis `value`. Returns `None` when the bucket is empty.
fn agg_date(aggs: &Value, name: &str) -> Option<DateTime<Utc>> {
    let node = aggs.get(name)?;
    if let Some(s) = node.get("value_as_string").and_then(|v| v.as_str())
        && let Ok(dt) = DateTime::parse_from_rfc3339(s)
    {
        return Some(dt.with_timezone(&Utc));
    }
    let ms = node.get("value").and_then(|v| v.as_f64())?;
    if ms <= 0.0 {
        return None;
    }
    Utc.timestamp_millis_opt(ms as i64).single()
}

/// Extract `(key, doc_count)` pairs from a terms aggregation.
fn bucket_counts(resp: &Value, agg: &str) -> Vec<(String, usize)> {
    resp.get("aggregations")
        .and_then(|a| a.get(agg))
        .and_then(|t| t.get("buckets"))
        .and_then(|b| b.as_array())
        .map(|buckets| {
            buckets
                .iter()
                .filter_map(|b| {
                    let key = b.get("key")?.as_str()?.to_string();
                    let count = b.get("doc_count")?.as_u64()? as usize;
                    Some((key, count))
                })
                .collect()
        })
        .unwrap_or_default()
}
