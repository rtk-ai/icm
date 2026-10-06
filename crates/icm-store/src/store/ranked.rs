//! Ranked (v2) search for the SQLite backend: deep candidates per arm,
//! rank fusion (RRF), caller filter applied before the cut.
//!
//! Lives beside `search_hybrid` (`store/memory.rs`) without replacing it.
//! The differences that matter:
//!
//! - candidate depth is a parameter, not `clamp(limit, 1, 5) * 4`, so the
//!   result count is no longer capped at 40;
//! - arms are fused on ranks, so a memory with no embedding is not capped
//!   under the 0.3 lexical weight;
//! - `keep` runs on the candidates, so a topic/project/keyword filter
//!   cannot empty a result set that was cut before it ran.
//!
//! Read-only: no access bookkeeping here, the caller decides.

use super::*;

use icm_core::fusion::{rrf_fuse, RankedHit, RankedList, RankedQuery, RRF_K};

/// Floor on candidates per arm: the legacy pool size, so v2 never looks
/// at fewer candidates than `search_hybrid` does.
const MIN_DEPTH: usize = 20;
/// Ceiling on candidates per arm. sqlite-vec rejects a KNN with k > 4096.
const MAX_DEPTH: usize = 2000;
/// Each widening pass multiplies the depth by this much.
const WIDEN_FACTOR: usize = 4;
/// Widening passes after the first fetch.
const MAX_WIDENINGS: usize = 3;
/// Bound parameters per statement when loading embeddings of final hits.
const HYDRATE_CHUNK: usize = 500;

/// One arm's candidates that passed the caller's filter, best first.
struct Arm {
    kept: Vec<Memory>,
    /// The arm returned as many rows as it was asked for: more may exist
    /// beyond `depth`.
    saturated: bool,
}

impl Arm {
    fn empty() -> Self {
        Self {
            kept: Vec::new(),
            saturated: false,
        }
    }
}

impl SqliteStore {
    /// Ranked search: lexical (BM25) and vector (KNN) arms fetched
    /// `q.depth` deep, filtered by `keep`, fused by Reciprocal Rank Fusion
    /// and cut to `q.limit`.
    ///
    /// - `q.depth` is clamped to `[20, 2000]`. When fewer than `q.limit`
    ///   candidates survive `keep` and an arm was saturated, saturated arms
    ///   are refetched four times deeper, up to three times.
    /// - Empty `q.text` (or one with no searchable token) gives the vector
    ///   ranking; `q.embedding = None` gives the BM25 ranking.
    /// - `q.window` adds a third list: kept candidates whose `created_at`
    ///   falls in the window, ordered by vector rank then lexical rank.
    ///   A window matching no candidate changes nothing.
    /// - `keep` sees lexical-arm candidates without their embedding blob
    ///   (loaded for the final hits only): it must not test `embedding`.
    /// - Ranks in the returned hits are positions after filtering, 1-based.
    ///   Scores are in (0, 1], 1.0 = first in every non-empty arm.
    ///
    /// Errors are propagated, except an FTS5 syntax error, treated as an
    /// empty lexical arm. Writes nothing.
    pub fn search_ranked(
        &self,
        q: &RankedQuery<'_>,
        keep: &dyn Fn(&Memory) -> bool,
    ) -> IcmResult<Vec<RankedHit>> {
        if q.limit == 0 {
            return Ok(Vec::new());
        }

        // OR-joined, same reasoning as `search_hybrid`.
        let match_expr = sanitize_fts_query_any(q.text);
        let embedding = q.embedding.filter(|e| !e.is_empty());

        let mut depth = q.depth.clamp(MIN_DEPTH, MAX_DEPTH);
        let mut fts = self.ranked_fts_arm(&match_expr, depth, keep)?;
        let mut vec = self.ranked_vec_arm(embedding, depth, keep)?;

        // A filter can discard most of a saturated arm. Fetch deeper rather
        // than return a short list while matching rows sit just past
        // `depth`.
        for _ in 0..MAX_WIDENINGS {
            if depth >= MAX_DEPTH
                || !(fts.saturated || vec.saturated)
                || union_len(&fts.kept, &vec.kept) >= q.limit
            {
                break;
            }
            depth = depth.saturating_mul(WIDEN_FACTOR).min(MAX_DEPTH);
            if fts.saturated {
                fts = self.ranked_fts_arm(&match_expr, depth, keep)?;
            }
            if vec.saturated {
                vec = self.ranked_vec_arm(embedding, depth, keep)?;
            }
        }

        let fts_ids: Vec<String> = fts.kept.iter().map(|m| m.id.clone()).collect();
        let vec_ids: Vec<String> = vec.kept.iter().map(|m| m.id.clone()).collect();

        // Vector-arm copies first: they carry the embedding blob, the
        // lexical ones do not.
        let mut by_id: HashMap<String, Memory> =
            HashMap::with_capacity(fts_ids.len() + vec_ids.len());
        for memory in vec.kept {
            by_id.insert(memory.id.clone(), memory);
        }
        for memory in fts.kept {
            by_id.entry(memory.id.clone()).or_insert(memory);
        }

        let time_ids: Vec<String> = match q.window {
            Some(window) => {
                let vec_pos = positions(&vec_ids);
                let fts_pos = positions(&fts_ids);
                let mut inside: Vec<(usize, usize, &str)> = by_id
                    .values()
                    .filter(|m| window.contains(m.created_at))
                    .map(|m| {
                        (
                            vec_pos.get(m.id.as_str()).copied().unwrap_or(usize::MAX),
                            fts_pos.get(m.id.as_str()).copied().unwrap_or(usize::MAX),
                            m.id.as_str(),
                        )
                    })
                    .collect();
                inside.sort_unstable();
                inside
                    .into_iter()
                    .map(|(_, _, id)| id.to_string())
                    .collect()
            }
            None => Vec::new(),
        };

        let fused = rrf_fuse(
            &[
                RankedList {
                    name: "fts",
                    ids: &fts_ids,
                    weight: 1.0,
                },
                RankedList {
                    name: "vec",
                    ids: &vec_ids,
                    weight: 1.0,
                },
                RankedList {
                    name: "time",
                    ids: &time_ids,
                    weight: 1.0,
                },
            ],
            RRF_K,
        );

        let mut hits: Vec<RankedHit> = Vec::with_capacity(q.limit.min(fused.len()));
        for hit in fused.into_iter().take(q.limit) {
            let rank = |arm: usize| hit.ranks.get(arm).copied().flatten();
            if let Some(memory) = by_id.remove(&hit.id) {
                hits.push(RankedHit {
                    memory,
                    score: hit.score,
                    fts_rank: rank(0),
                    vec_rank: rank(1),
                    time_rank: rank(2),
                });
            }
        }

        self.hydrate_embeddings(&mut hits)?;
        Ok(hits)
    }

    /// Lexical arm: BM25 order, at most `depth` rows, filtered by `keep`.
    fn ranked_fts_arm(
        &self,
        match_expr: &str,
        depth: usize,
        keep: &dyn Fn(&Memory) -> bool,
    ) -> IcmResult<Arm> {
        if match_expr.is_empty() {
            return Ok(Arm::empty());
        }

        // Same shape as `search_hybrid`'s FTS query, in `row_to_memory`
        // column order, except NULL instead of `m.embedding`: at depth
        // 2000 that would read 2000 blobs of several KB to rank ids.
        // `hydrate_embeddings` loads them for the final hits only.
        let sql =
            "SELECT m.id, m.created_at, m.updated_at, m.last_accessed, m.access_count, m.weight, \
                    m.topic, m.summary, m.raw_excerpt, m.keywords, \
                    m.importance, m.source_type, m.source_data, m.related_ids, NULL, \
                    fts.rank \
             FROM memories_fts fts \
             JOIN memories m ON m.id = fts.id \
             WHERE memories_fts MATCH ?1 \
             ORDER BY fts.rank \
             LIMIT ?2";

        let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
        let rows = stmt.query_map(params![match_expr, depth as i64], |row| {
            let memory = row_to_memory(row)?;
            let rank: f64 = row.get(15)?;
            Ok((memory, rank))
        });
        let rows = match rows {
            Ok(rows) => rows,
            Err(e) if is_fts5_syntax_error(&e) => return Ok(Arm::empty()),
            Err(e) => return Err(db_err(e)),
        };

        let mut scored: Vec<(Memory, f64)> = Vec::new();
        for row in rows {
            match row {
                Ok(pair) => scored.push(pair),
                Err(e) if is_fts5_syntax_error(&e) => return Ok(Arm::empty()),
                Err(e) => return Err(db_err(e)),
            }
        }

        let saturated = scored.len() >= depth;
        // bm25 rank: more negative = more relevant. Equal ranks are common
        // (same terms, same length); break them on id so two identical
        // calls give the same order.
        scored.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.id.cmp(&b.0.id)));

        Ok(Arm {
            kept: scored
                .into_iter()
                .map(|(memory, _)| memory)
                .filter(|memory| keep(memory))
                .collect(),
            saturated,
        })
    }

    /// Vector arm: KNN order, at most `depth` rows, filtered by `keep`.
    /// No embedding = empty arm.
    fn ranked_vec_arm(
        &self,
        embedding: Option<&[f32]>,
        depth: usize,
        keep: &dyn Fn(&Memory) -> bool,
    ) -> IcmResult<Arm> {
        let Some(embedding) = embedding else {
            return Ok(Arm::empty());
        };

        let mut scored = self.search_by_embedding(embedding, depth)?;
        let saturated = scored.len() >= depth;
        // Similarity descending, NaN (zero-norm vector) last, ties on id.
        let key = |similarity: f32| {
            if similarity.is_nan() {
                f32::NEG_INFINITY
            } else {
                similarity
            }
        };
        scored.sort_by(|a, b| {
            key(b.1)
                .total_cmp(&key(a.1))
                .then_with(|| a.0.id.cmp(&b.0.id))
        });

        Ok(Arm {
            kept: scored
                .into_iter()
                .map(|(memory, _)| memory)
                .filter(|memory| keep(memory))
                .collect(),
            saturated,
        })
    }

    /// Load the stored embedding of final hits that came from the lexical
    /// arm only, so a returned `Memory` equals what `get()` gives.
    /// `update()` writes `embedding` back as-is: a hit handed to it with a
    /// stripped `None` would erase the stored vector.
    fn hydrate_embeddings(&self, hits: &mut [RankedHit]) -> IcmResult<()> {
        let missing: Vec<usize> = hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| hit.memory.embedding.is_none())
            .map(|(idx, _)| idx)
            .collect();

        for chunk in missing.chunks(HYDRATE_CHUNK) {
            let mut blobs: HashMap<String, Vec<u8>> = {
                let placeholders: Vec<String> =
                    (1..=chunk.len()).map(|i| format!("?{i}")).collect();
                let sql = format!(
                    "SELECT id, embedding FROM memories \
                     WHERE id IN ({}) AND embedding IS NOT NULL",
                    placeholders.join(", ")
                );
                let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
                let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                    .iter()
                    .map(|&idx| &hits[idx].memory.id as &dyn rusqlite::types::ToSql)
                    .collect();
                let rows = stmt
                    .query_map(&*params, |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(db_err)?;
                collect_rows(rows)?.into_iter().collect()
            };

            for &idx in chunk {
                if let Some(blob) = blobs.remove(&hits[idx].memory.id) {
                    hits[idx].memory.embedding = Some(blob_to_embedding(&blob));
                }
            }
        }
        Ok(())
    }
}

/// Position of each id in a ranked list.
fn positions(ids: &[String]) -> HashMap<&str, usize> {
    ids.iter()
        .enumerate()
        .map(|(pos, id)| (id.as_str(), pos))
        .collect()
}

/// Distinct memories across both arms.
fn union_len(fts: &[Memory], vec: &[Memory]) -> usize {
    let seen: HashSet<&str> = fts.iter().map(|m| m.id.as_str()).collect();
    seen.len() + vec.iter().filter(|m| !seen.contains(m.id.as_str())).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use icm_core::temporal::TimeWindow;

    const DIMS: usize = 64;

    fn store() -> SqliteStore {
        SqliteStore::in_memory_with_dims(DIMS).unwrap()
    }

    /// Unit vector on `axis`.
    fn axis(axis: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; DIMS];
        v[axis % DIMS] = 1.0;
        v
    }

    /// Vector whose cosine similarity to `axis(0)` strictly decreases as
    /// `i` grows: 1 on axis 0, `(i + 1) / 50` on another axis.
    fn spread(i: usize) -> Vec<f32> {
        let mut v = axis(0);
        v[1 + i % (DIMS - 1)] = (i + 1) as f32 / 50.0;
        v
    }

    /// Store a memory under a fixed id, so tie-breaks are reproducible.
    fn put(store: &SqliteStore, id: &str, topic: &str, summary: &str, embedding: Option<Vec<f32>>) {
        let mut m = Memory::new(topic.into(), summary.into(), Importance::Medium);
        m.id = id.into();
        m.embedding = embedding;
        store.store(m).unwrap();
    }

    fn put_at(
        store: &SqliteStore,
        id: &str,
        summary: &str,
        embedding: Vec<f32>,
        created_at: DateTime<Utc>,
    ) {
        let mut m = Memory::new("notes".into(), summary.into(), Importance::Medium);
        m.id = id.into();
        m.embedding = Some(embedding);
        m.created_at = created_at;
        store.store(m).unwrap();
    }

    fn query<'a>(
        text: &'a str,
        embedding: Option<&'a [f32]>,
        limit: usize,
        depth: usize,
    ) -> RankedQuery<'a> {
        RankedQuery {
            text,
            embedding,
            limit,
            depth,
            window: None,
        }
    }

    fn all(_: &Memory) -> bool {
        true
    }

    fn ids(hits: &[RankedHit]) -> Vec<&str> {
        hits.iter().map(|h| h.memory.id.as_str()).collect()
    }

    /// 150 memories that all match "alpha", each with its own embedding.
    fn seed_150(store: &SqliteStore) {
        for i in 0..150 {
            put(
                store,
                &format!("m-{i:03}"),
                "notes",
                &format!("alpha entry {i}"),
                Some(spread(i)),
            );
        }
    }

    #[test]
    fn deep_limit_returns_more_than_forty() {
        let store = store();
        seed_150(&store);
        let q_emb = axis(0);

        let hits = store
            .search_ranked(&query("alpha", Some(&q_emb), 100, 100), &all)
            .unwrap();
        assert_eq!(hits.len(), 100);
        let distinct: HashSet<&str> = ids(&hits).into_iter().collect();
        assert_eq!(distinct.len(), 100);
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));

        // Asked too shallow for the limit: widening fills it anyway.
        let hits = store
            .search_ranked(&query("alpha", Some(&q_emb), 100, 20), &all)
            .unwrap();
        assert_eq!(hits.len(), 100);

        // The whole base is reachable.
        let hits = store
            .search_ranked(&query("alpha", Some(&q_emb), 500, 2000), &all)
            .unwrap();
        assert_eq!(hits.len(), 150);
        // m-000 leads both arms: nearest vector, and lowest id among
        // equal BM25 ranks.
        assert_eq!(hits[0].memory.id, "m-000");
        assert_eq!(hits[0].score, 1.0);
    }

    #[test]
    fn memory_without_embedding_can_rank_first() {
        let store = store();
        put(
            &store,
            "a-target",
            "notes",
            "zebra quartz migration checklist",
            None,
        );
        for i in 0..30 {
            put(
                &store,
                &format!("d-{i:02}"),
                "notes",
                &format!("grocery list item {i}"),
                Some(spread(i)),
            );
        }
        let q_emb = axis(0);

        let hits = store
            .search_ranked(&query("zebra quartz", Some(&q_emb), 10, 50), &all)
            .unwrap();

        // Best of the lexical arm, absent from the vector arm.
        assert_eq!(hits[0].memory.id, "a-target");
        assert_eq!(hits[0].fts_rank, Some(1));
        assert_eq!(hits[0].vec_rank, None);
        assert!(hits[0].memory.embedding.is_none());
        // Rank fusion puts it level with the leader of the vector arm
        // (id breaks the tie) and above every other embedded memory. The
        // weighted blend of `search_hybrid` caps it at 0.3.
        assert_eq!(hits[1].memory.id, "d-00");
        assert_eq!(hits[1].vec_rank, Some(1));
        assert_eq!(hits[0].score, hits[1].score);
        assert!(hits[2..].iter().all(|h| h.score < hits[0].score));
    }

    #[test]
    fn filter_applies_before_the_cut() {
        let store = store();
        // 60 memories of another topic, better on both arms.
        for i in 0..60 {
            put(
                &store,
                &format!("o-{i:02}"),
                "other",
                &format!("deploy pipeline deploy pipeline rollout {i}"),
                Some(spread(i)),
            );
        }
        // 5 of the wanted topic, weaker on both arms.
        for i in 0..5 {
            put(
                &store,
                &format!("t-{i}"),
                "wanted",
                &format!("pipeline cleanup task {i} with a longer unrelated tail of words"),
                Some(spread(100 + i)),
            );
        }
        // Background, so the query terms are not in nearly every row.
        for i in 0..100 {
            put(
                &store,
                &format!("z-{i:03}"),
                "misc",
                &format!("unrelated background fact {i}"),
                None,
            );
        }
        let q_emb = axis(0);
        let wanted = |m: &Memory| m.topic == "wanted";

        // The defect being reproduced: at the starting depth, neither arm
        // holds a single memory of the wanted topic.
        let match_expr = sanitize_fts_query_any("deploy pipeline");
        let fts = store.ranked_fts_arm(&match_expr, 20, &wanted).unwrap();
        let vec = store.ranked_vec_arm(Some(&q_emb), 20, &wanted).unwrap();
        assert!(fts.kept.is_empty() && fts.saturated);
        assert!(vec.kept.is_empty() && vec.saturated);

        // Unfiltered, the wanted topic is nowhere near the top 5.
        let unfiltered = store
            .search_ranked(&query("deploy pipeline", Some(&q_emb), 5, 20), &all)
            .unwrap();
        assert!(unfiltered.iter().all(|h| h.memory.topic == "other"));

        let hits = store
            .search_ranked(&query("deploy pipeline", Some(&q_emb), 5, 20), &wanted)
            .unwrap();
        let mut got = ids(&hits);
        got.sort_unstable();
        assert_eq!(got, vec!["t-0", "t-1", "t-2", "t-3", "t-4"]);
        // Reaching these took a wider fetch. Ranks are counted after
        // filtering.
        for h in &hits {
            assert!(matches!(h.fts_rank, Some(1..=5)), "{:?}", h.fts_rank);
            assert!(matches!(h.vec_rank, Some(1..=5)), "{:?}", h.vec_rank);
        }

        // A filter nothing passes ends cleanly.
        let none = store
            .search_ranked(&query("deploy pipeline", Some(&q_emb), 5, 20), &|_| false)
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn no_embedding_gives_bm25_order() {
        let store = store();
        put(&store, "b-3", "notes", "rust filler filler", Some(axis(1)));
        put(&store, "b-1", "notes", "rust rust rust", None);
        put(&store, "b-2", "notes", "rust rust filler", Some(axis(2)));
        for i in 0..7 {
            put(
                &store,
                &format!("f-{i}"),
                "notes",
                &format!("filler filler other{i}"),
                Some(axis(3 + i)),
            );
        }

        let hits = store
            .search_ranked(&query("rust", None, 10, 50), &all)
            .unwrap();
        assert_eq!(ids(&hits), vec!["b-1", "b-2", "b-3"]);
        assert_eq!(hits[0].score, 1.0);
        for (pos, h) in hits.iter().enumerate() {
            assert_eq!(h.fts_rank, Some(pos as u32 + 1));
            assert_eq!(h.vec_rank, None);
            assert_eq!(h.time_rank, None);
        }

        // An empty slice is "no embedding", not a KNN on zero dimensions.
        let empty: [f32; 0] = [];
        let same = store
            .search_ranked(&query("rust", Some(&empty), 10, 50), &all)
            .unwrap();
        assert_eq!(ids(&same), ids(&hits));
    }

    #[test]
    fn empty_text_gives_vector_order() {
        let store = store();
        // Stored out of order; similarity to axis 0 falls with the suffix.
        for i in [3_usize, 0, 4, 1, 2] {
            put(
                &store,
                &format!("v-{i}"),
                "notes",
                &format!("note number {i}"),
                Some(spread(i * 10)),
            );
        }
        put(&store, "v-none", "notes", "note without vector", None);
        let q_emb = axis(0);
        let expected = vec!["v-0", "v-1", "v-2", "v-3", "v-4"];

        // Empty, blank, and operator-only texts all leave no lexical token.
        for text in ["", "   ", "-- ** \"\" ()"] {
            let hits = store
                .search_ranked(&query(text, Some(&q_emb), 10, 50), &all)
                .unwrap();
            assert_eq!(ids(&hits), expected, "text = {text:?}");
            assert_eq!(hits[0].score, 1.0);
            for (pos, h) in hits.iter().enumerate() {
                assert_eq!(h.vec_rank, Some(pos as u32 + 1));
                assert_eq!(h.fts_rank, None);
            }
        }

        // Neither arm has anything to rank.
        let nothing = store.search_ranked(&query("", None, 10, 50), &all).unwrap();
        assert!(nothing.is_empty());
    }

    #[test]
    fn window_lifts_a_memory_dated_inside() {
        let store = store();
        let january = Utc.with_ymd_and_hms(2024, 1, 5, 9, 0, 0).unwrap();
        for i in 0..6_usize {
            let created_at = match i {
                3 => Utc.with_ymd_and_hms(2024, 3, 10, 12, 0, 0).unwrap(),
                5 => Utc.with_ymd_and_hms(2024, 3, 20, 12, 0, 0).unwrap(),
                _ => january,
            };
            put_at(
                &store,
                &format!("w-{i}"),
                &format!("sprint review notes {i}"),
                spread(i * 10),
                created_at,
            );
        }
        let q_emb = axis(0);
        let base = query("sprint review", Some(&q_emb), 10, 50);

        let plain = store.search_ranked(&base, &all).unwrap();
        assert_eq!(ids(&plain), vec!["w-0", "w-1", "w-2", "w-3", "w-4", "w-5"]);
        assert!(plain.iter().all(|h| h.time_rank.is_none()));

        let march = TimeWindow {
            start: Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2024, 4, 1, 0, 0, 0).unwrap(),
        };
        let windowed = store
            .search_ranked(
                &RankedQuery {
                    window: Some(march),
                    ..base
                },
                &all,
            )
            .unwrap();
        // Both March memories pass the January ones, in vector-rank order.
        assert_eq!(
            ids(&windowed),
            vec!["w-3", "w-5", "w-0", "w-1", "w-2", "w-4"]
        );
        assert_eq!(windowed[0].time_rank, Some(1));
        assert_eq!(windowed[1].time_rank, Some(2));
        assert!(windowed[2..].iter().all(|h| h.time_rank.is_none()));
        // Still ranked 4th and 6th on the other two arms.
        assert_eq!(windowed[0].vec_rank, Some(4));
        assert_eq!(windowed[1].fts_rank, Some(6));

        // A window with nothing in it changes neither order nor scores.
        let empty_window = TimeWindow {
            start: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2030, 2, 1, 0, 0, 0).unwrap(),
        };
        let unchanged = store
            .search_ranked(
                &RankedQuery {
                    window: Some(empty_window),
                    ..base
                },
                &all,
            )
            .unwrap();
        assert_eq!(ids(&unchanged), ids(&plain));
        for (a, b) in unchanged.iter().zip(&plain) {
            assert_eq!(a.score, b.score);
        }
    }

    #[test]
    fn identical_calls_give_identical_order() {
        let store = store();
        seed_150(&store);
        let q_emb = axis(0);
        let q = query("alpha entry", Some(&q_emb), 100, 60);

        let first = store.search_ranked(&q, &all).unwrap();
        let second = store.search_ranked(&q, &all).unwrap();
        assert_eq!(first.len(), 100);
        assert_eq!(ids(&first), ids(&second));
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.score, b.score);
            assert_eq!(
                (a.fts_rank, a.vec_rank, a.time_rank),
                (b.fts_rank, b.vec_rank, b.time_rank)
            );
        }
    }

    /// A query vector of the wrong dimension (embedder and index out of
    /// step) must degrade to the lexical ranking, exactly: no error, no
    /// panic, no vector rank computed from a mismatched vector.
    #[test]
    fn wrong_dimension_query_vector_falls_back_to_the_lexical_ranking() {
        let store = store();
        seed_150(&store);
        let wrong = vec![0.5_f32; DIMS / 2];

        let hits = store
            .search_ranked(&query("alpha", Some(&wrong), 10, 50), &all)
            .expect("a mismatched query vector must not fail the search");
        assert_eq!(hits.len(), 10);
        assert!(hits.iter().all(|h| h.vec_rank.is_none()));
        assert!(hits.iter().all(|h| h.fts_rank.is_some()));

        let lexical = store
            .search_ranked(&query("alpha", None, 10, 50), &all)
            .unwrap();
        let ranking = |hits: &[RankedHit]| -> Vec<(String, f32)> {
            hits.iter()
                .map(|h| (h.memory.id.clone(), h.score))
                .collect()
        };
        assert_eq!(ranking(&hits), ranking(&lexical));
    }

    #[test]
    fn depth_is_clamped_and_zero_limit_is_empty() {
        let store = store();
        seed_150(&store);
        let q_emb = axis(0);

        // Unclamped, usize::MAX would be refused by sqlite-vec (k > 4096)
        // and 0 would fetch nothing.
        for depth in [0, 1, usize::MAX] {
            let hits = store
                .search_ranked(&query("alpha", Some(&q_emb), 10, depth), &all)
                .unwrap();
            assert_eq!(hits.len(), 10, "depth = {depth}");
            assert!(hits.iter().any(|h| h.vec_rank.is_some()));
            assert!(hits.iter().any(|h| h.fts_rank.is_some()));
        }

        let none = store
            .search_ranked(&query("alpha", Some(&q_emb), 0, 50), &all)
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn lexical_only_hits_keep_their_stored_embedding() {
        let store = store();
        put(&store, "h-1", "notes", "kiwi harvest plan", Some(spread(1)));
        put(&store, "h-2", "notes", "kiwi storage plan", None);

        // No query embedding: both come from the lexical arm, which does
        // not read blobs.
        let hits = store
            .search_ranked(&query("kiwi", None, 10, 50), &all)
            .unwrap();
        assert_eq!(hits.len(), 2);
        for h in &hits {
            let stored = store.get(&h.memory.id).unwrap().unwrap();
            assert_eq!(h.memory.embedding, stored.embedding, "{}", h.memory.id);
        }
        let with_vector = hits.iter().find(|h| h.memory.id == "h-1").unwrap();
        assert_eq!(
            with_vector.memory.embedding.as_deref(),
            Some(&spread(1)[..])
        );
    }

    #[test]
    fn search_writes_nothing() {
        let store = store();
        put(
            &store,
            "r-1",
            "notes",
            "mango ripening log",
            Some(spread(1)),
        );
        let before = store.get("r-1").unwrap().unwrap();
        store.cache_clear();

        let q_emb = axis(0);
        let hits = store
            .search_ranked(&query("mango", Some(&q_emb), 5, 50), &all)
            .unwrap();
        assert_eq!(ids(&hits), vec!["r-1"]);

        store.cache_clear();
        let after = store.get("r-1").unwrap().unwrap();
        assert_eq!(after.access_count, before.access_count);
        assert_eq!(after.last_accessed, before.last_accessed);
        assert_eq!(after.weight, before.weight);
    }

    #[test]
    fn query_with_fts_operators_is_not_an_error() {
        let store = store();
        put(&store, "s-1", "notes", "sqlite vec extension notes", None);

        for text in ["sqlite-vec", "\"sqlite", "vec AND (", "notes: ^vec*"] {
            let hits = store
                .search_ranked(&query(text, None, 5, 50), &all)
                .unwrap();
            assert_eq!(ids(&hits), vec!["s-1"], "text = {text:?}");
        }
    }
}
