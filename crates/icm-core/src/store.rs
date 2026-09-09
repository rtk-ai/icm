use crate::error::IcmResult;
use crate::memory::{Memory, StoreStats, TopicHealth};

/// Similarity score above which a new memory is considered a duplicate of an existing one.
///
/// Calibrated empirically against the real multilingual-e5-base embedder
/// (manual testing finding, 2026-07-27): short, topic-scoped sentences
/// that share a syntactic template ("we picked X over Y" / "on a choisi X
/// plutôt que Y") but describe genuinely DIFFERENT facts scored
/// 0.90-0.93 cosine similarity — e.g. "We picked PostgreSQL over MySQL
/// for the primary database" vs "We picked Kubernetes over Docker Swarm
/// for the orchestration layer" scored 0.9093. Genuine near-duplicate
/// restatements ("PostgreSQL was selected over MySQL for the main
/// datastore") scored 0.9156-0.9635 — overlapping enough with the
/// false-positive band that no threshold cleanly separates every case.
/// 0.95 was chosen to eliminate the measured false positives (0.90-0.93)
/// even at the cost of missing some looser true near-duplicates.
///
/// This threshold only decides whether a pair is a dedup *candidate* — it
/// does not prove the two say the same thing. A second, independently
/// measured false positive (two distinct LoCoMo benchmark greeting turns
/// scoring 0.98, 2026-08-28) got past even this threshold and confirmed a
/// case above 0.95 can still be unrelated content. The merge path
/// (`cmd_store` / `icm_memory_store`) therefore never wholesale-replaces
/// the existing summary — see `merge_summaries` — so a wrong candidate
/// costs an extra, redundant sentence rather than destroying a memory.
pub const DEDUP_SIMILARITY_THRESHOLD: f32 = 0.95;

/// Find an existing memory that is similar enough to be considered a duplicate.
///
/// Returns the closest match and its similarity score if the score exceeds `threshold`
/// and the match belongs to the same topic. Returns `None` otherwise.
///
/// Uses `search_by_embedding` (pure cosine similarity) rather than
/// `search_hybrid`. Dedup is fundamentally a "is this the same content"
/// question, which cosine similarity answers directly. The hybrid blend
/// (`0.3*fts + 0.7*cosine`) is tuned for human-facing recall RANKING, not
/// for a duplicate/not-duplicate decision — diluting an exact semantic
/// match (cosine=1.0) with an FTS component made the 0.85 threshold nearly
/// unreachable even for byte-identical content once the FTS side scored
/// low (e.g. AND-joined tokens not matching), and a purely vector-side
/// match (no shared keywords, cosine=1.0) could never exceed
/// `0.3*0 + 0.7*1.0 = 0.70` (audit finding).
pub fn find_similar_memory(
    store: &dyn MemoryStore,
    _embed_text: &str,
    embedding: &[f32],
    topic: &str,
    threshold: f32,
) -> IcmResult<Option<(Memory, f32)>> {
    let similar = store.search_by_embedding(embedding, 1)?;
    Ok(similar
        .into_iter()
        .find(|(m, score)| *score > threshold && m.topic == topic))
}

/// Combine an existing memory's summary with incoming content that scored
/// above `DEDUP_SIMILARITY_THRESHOLD` against it.
///
/// `find_similar_memory` only proves the two are *semantically close*
/// (cosine similarity), not that they say the same thing — the threshold's
/// own docs record real cases (short, templated sentences) where distinct
/// facts score above 0.95. Wholesale-replacing `existing` with `incoming`
/// in that band silently destroys whichever fact was already stored (found
/// empirically: two different greeting turns from a LoCoMo benchmark
/// conversation scored 0.98 and the second overwrote the first).
///
/// When the two strings are the same statement (after trimming), returns
/// `existing` unchanged — nothing to preserve, no growth. Otherwise appends
/// `incoming` rather than discarding `existing`, trading a little
/// redundancy for zero data loss; `auto_consolidate` (invoked right after
/// every dedup update) keeps a topic that keeps restating itself from
/// growing unbounded.
pub fn merge_summaries(existing: &str, incoming: &str) -> String {
    if existing.trim() == incoming.trim() {
        existing.to_string()
    } else {
        format!("{}\n{}", existing.trim(), incoming.trim())
    }
}

/// Union two keyword lists, preserving `existing`'s order and appending only
/// `incoming` entries not already present (case-sensitive, matching how
/// keywords are stored and filtered elsewhere).
///
/// Used alongside `merge_summaries` so a dedup update never drops the
/// original memory's keywords (e.g. a dialogue-turn id) just because the
/// incoming store call also supplied keywords.
pub fn union_keywords(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut merged = existing.to_vec();
    for kw in incoming {
        if !merged.contains(kw) {
            merged.push(kw.clone());
        }
    }
    merged
}

pub trait MemoryStore {
    // CRUD
    fn store(&self, memory: Memory) -> IcmResult<String>;
    fn get(&self, id: &str) -> IcmResult<Option<Memory>>;
    fn update(&self, memory: &Memory) -> IcmResult<()>;
    fn delete(&self, id: &str) -> IcmResult<()>;

    // Search
    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>>;
    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>>;
    fn search_by_embedding(&self, embedding: &[f32], limit: usize)
    -> IcmResult<Vec<(Memory, f32)>>;
    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>>;

    // Lifecycle
    fn update_access(&self, id: &str) -> IcmResult<()>;
    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize>;
    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize>;
    fn prune(&self, weight_threshold: f32) -> IcmResult<usize>;

    // Organization
    fn list_all(&self) -> IcmResult<Vec<Memory>>;
    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>>;
    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>>;
    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()>;

    // Stats
    fn count(&self) -> IcmResult<usize>;
    fn count_by_topic(&self, topic: &str) -> IcmResult<usize>;
    fn stats(&self) -> IcmResult<StoreStats>;
    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Importance;

    /// Minimal `MemoryStore` double: `search_by_embedding` returns a fixed
    /// (Memory, score) pair regardless of the query embedding, everything
    /// else is unused by `find_similar_memory` and panics if called.
    struct FixedMatchStore {
        candidate: Memory,
        score: f32,
    }

    impl MemoryStore for FixedMatchStore {
        fn store(&self, _memory: Memory) -> IcmResult<String> {
            unimplemented!()
        }
        fn get(&self, _id: &str) -> IcmResult<Option<Memory>> {
            unimplemented!()
        }
        fn update(&self, _memory: &Memory) -> IcmResult<()> {
            unimplemented!()
        }
        fn delete(&self, _id: &str) -> IcmResult<()> {
            unimplemented!()
        }
        fn search_by_keywords(&self, _keywords: &[&str], _limit: usize) -> IcmResult<Vec<Memory>> {
            unimplemented!()
        }
        fn search_fts(&self, _query: &str, _limit: usize) -> IcmResult<Vec<Memory>> {
            unimplemented!()
        }
        fn search_by_embedding(
            &self,
            _embedding: &[f32],
            _limit: usize,
        ) -> IcmResult<Vec<(Memory, f32)>> {
            Ok(vec![(self.candidate.clone(), self.score)])
        }
        fn search_hybrid(
            &self,
            _query: &str,
            _embedding: &[f32],
            _limit: usize,
        ) -> IcmResult<Vec<(Memory, f32)>> {
            unimplemented!()
        }
        fn update_access(&self, _id: &str) -> IcmResult<()> {
            unimplemented!()
        }
        fn batch_update_access(&self, _ids: &[&str]) -> IcmResult<usize> {
            unimplemented!()
        }
        fn apply_decay(&self, _decay_factor: f32) -> IcmResult<usize> {
            unimplemented!()
        }
        fn prune(&self, _weight_threshold: f32) -> IcmResult<usize> {
            unimplemented!()
        }
        fn list_all(&self) -> IcmResult<Vec<Memory>> {
            unimplemented!()
        }
        fn get_by_topic(&self, _topic: &str) -> IcmResult<Vec<Memory>> {
            unimplemented!()
        }
        fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
            unimplemented!()
        }
        fn consolidate_topic(&self, _topic: &str, _consolidated: Memory) -> IcmResult<()> {
            unimplemented!()
        }
        fn count(&self) -> IcmResult<usize> {
            unimplemented!()
        }
        fn count_by_topic(&self, _topic: &str) -> IcmResult<usize> {
            unimplemented!()
        }
        fn stats(&self) -> IcmResult<StoreStats> {
            unimplemented!()
        }
        fn topic_health(&self, _topic: &str) -> IcmResult<TopicHealth> {
            unimplemented!()
        }
    }

    /// Manual-testing finding (2026-07-27): short, topic-scoped sentences
    /// sharing a syntactic template but describing genuinely different
    /// facts measured 0.90-0.93 cosine similarity against the real
    /// multilingual-e5-base embedder — e.g. "We picked PostgreSQL over
    /// MySQL for the primary database" vs "We picked Kubernetes over
    /// Docker Swarm for the orchestration layer" scored 0.9093. At the old
    /// 0.85 threshold this silently overwrote the earlier memory's
    /// content. The new threshold (0.95) must reject a 0.90-similarity
    /// match; a near-exact 0.99 match must still be treated as a
    /// duplicate.
    #[test]
    fn dedup_threshold_rejects_the_measured_false_positive_similarity() {
        let candidate = Memory::new(
            "decisions".to_string(),
            "We picked PostgreSQL over MySQL for the primary database".to_string(),
            Importance::High,
        );
        let store = FixedMatchStore {
            candidate,
            score: 0.90,
        };
        let result = find_similar_memory(
            &store,
            "query",
            &[1.0, 0.0],
            "decisions",
            DEDUP_SIMILARITY_THRESHOLD,
        )
        .unwrap();
        assert!(
            result.is_none(),
            "a 0.90-similarity match (the measured false-positive band) must not be treated as a duplicate"
        );
    }

    #[test]
    fn dedup_threshold_still_accepts_a_near_exact_match() {
        let candidate = Memory::new(
            "decisions".to_string(),
            "We picked PostgreSQL over MySQL for the primary database".to_string(),
            Importance::High,
        );
        let store = FixedMatchStore {
            candidate,
            score: 0.99,
        };
        let result = find_similar_memory(
            &store,
            "query",
            &[1.0, 0.0],
            "decisions",
            DEDUP_SIMILARITY_THRESHOLD,
        )
        .unwrap();
        assert!(
            result.is_some(),
            "a near-exact 0.99-similarity match must still be treated as a duplicate"
        );
    }

    /// Regression for the destructive-overwrite bug (found running a LoCoMo
    /// benchmark pilot, 2026-08-28): two distinct dialogue turns scored 0.98
    /// cosine similarity and the dedup merge replaced the first with the
    /// second, losing it. `merge_summaries` must append, not replace, once
    /// the two strings are not the same statement.
    #[test]
    fn merge_summaries_appends_distinct_content_instead_of_replacing() {
        let existing = "Hey Mel! Good to see you! How have you been?";
        let incoming = "Hey Mel! Great to catch up! How's life been treating you?";
        let merged = merge_summaries(existing, incoming);
        assert!(
            merged.contains(existing) && merged.contains(incoming),
            "both distinct statements must survive the merge, got: {merged:?}"
        );
    }

    #[test]
    fn merge_summaries_does_not_duplicate_an_exact_restatement() {
        let text = "We picked PostgreSQL over MySQL for the primary database";
        let merged = merge_summaries(text, text);
        assert_eq!(
            merged, text,
            "merging identical content must not duplicate it"
        );
    }

    #[test]
    fn merge_summaries_ignores_surrounding_whitespace_when_comparing() {
        let merged = merge_summaries("same fact", "  same fact  \n");
        assert_eq!(merged, "same fact");
    }

    #[test]
    fn union_keywords_preserves_existing_and_appends_new() {
        let existing = vec!["D9:1".to_string(), "Melanie".to_string()];
        let incoming = vec!["D5:4".to_string(), "Melanie".to_string()];
        let merged = union_keywords(&existing, &incoming);
        assert_eq!(merged, vec!["D9:1", "Melanie", "D5:4"]);
    }

    #[test]
    fn union_keywords_with_empty_incoming_keeps_existing_unchanged() {
        let existing = vec!["a".to_string(), "b".to_string()];
        let merged = union_keywords(&existing, &[]);
        assert_eq!(merged, existing);
    }
}
