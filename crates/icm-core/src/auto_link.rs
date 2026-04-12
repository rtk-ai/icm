//! Automatic bidirectional linking of memories at store time.
//!
//! When a new memory is stored, `auto_link_memory` searches for the most
//! similar existing memories (by embedding cosine similarity) above a
//! threshold and populates the new memory's `related_ids` with their ids.
//! `add_backrefs` then updates the linked memories so the edges are
//! bidirectional.
//!
//! This turns ICM's manual knowledge graph into a self-growing associative
//! network. No schema change is required — both the `Memory.related_ids`
//! field and the `search_by_embedding` method already exist.

use crate::error::IcmResult;
use crate::memory::Memory;
use crate::store::MemoryStore;

/// Options controlling how auto-linking behaves.
#[derive(Debug, Clone, Copy)]
pub struct AutoLinkOptions {
    /// Enable/disable auto-linking entirely.
    pub enabled: bool,
    /// Cosine similarity threshold above which a candidate becomes a link.
    /// Typical value for `multilingual-e5-base` (768d): 0.75.
    pub threshold: f32,
    /// Maximum number of outgoing links from a newly stored memory.
    pub max_links: usize,
}

impl Default for AutoLinkOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.75,
            max_links: 5,
        }
    }
}

/// Find candidates similar to `new_memory` and populate its `related_ids`.
///
/// The new memory **must already have its embedding set**; otherwise this
/// function returns `Ok(Vec::new())` without touching the memory. The store
/// is queried via `search_by_embedding`.
///
/// Returns the list of memory ids that were added as forward links. The
/// caller is expected to then `store(new_memory)` (persisting the forward
/// links) and then call `add_backrefs` to complete the bidirectional graph.
///
/// Candidates that are the new memory itself are skipped (defensive, since
/// the new memory is typically not yet in the store at this point).
/// Candidates that are already in `related_ids` are not duplicated.
pub fn auto_link_memory<S: MemoryStore + ?Sized>(
    store: &S,
    new_memory: &mut Memory,
    opts: &AutoLinkOptions,
) -> IcmResult<Vec<String>> {
    if !opts.enabled || opts.max_links == 0 {
        return Ok(Vec::new());
    }

    let Some(ref emb) = new_memory.embedding else {
        // No embedding → no auto-link. This is expected when the embedder
        // is disabled; callers should skip gracefully.
        return Ok(Vec::new());
    };

    // Request one extra result so we can skip self-matches without running
    // out of candidates.
    let fetch_n = opts.max_links.saturating_add(1);
    let candidates = store.search_by_embedding(emb, fetch_n)?;

    let mut new_links: Vec<String> = Vec::new();
    for (candidate, score) in candidates {
        if score < opts.threshold {
            continue;
        }
        if candidate.id == new_memory.id {
            continue;
        }
        if new_memory.related_ids.contains(&candidate.id) {
            continue;
        }
        new_memory.related_ids.push(candidate.id.clone());
        new_links.push(candidate.id);
        if new_links.len() >= opts.max_links {
            break;
        }
    }

    Ok(new_links)
}

/// Update each memory in `linked_ids` so that it points back to
/// `new_memory_id` via its own `related_ids` list. Idempotent — if the
/// back-ref is already present, the memory is left unchanged.
///
/// Best-effort: a failure on one back-ref is logged by the caller but does
/// not roll back the others. The graph is allowed to be slightly asymmetric
/// under error conditions rather than losing the whole operation.
pub fn add_backrefs<S: MemoryStore + ?Sized>(
    store: &S,
    new_memory_id: &str,
    linked_ids: &[String],
) -> IcmResult<usize> {
    let mut updated = 0usize;
    for id in linked_ids {
        if let Some(mut existing) = store.get(id)? {
            if existing.related_ids.iter().any(|r| r == new_memory_id) {
                continue;
            }
            existing.related_ids.push(new_memory_id.to_string());
            store.update(&existing)?;
            updated += 1;
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::IcmResult;
    use crate::memory::{Importance, Memory, StoreStats, TopicHealth};
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Minimal in-memory `MemoryStore` implementation for unit testing the
    /// auto-link logic without pulling in the full sqlite store.
    struct FakeStore {
        memories: RefCell<HashMap<String, Memory>>,
        /// Pre-seeded similarity scores keyed by memory id, used to fake
        /// `search_by_embedding` deterministically.
        similarity: RefCell<HashMap<String, f32>>,
    }

    impl FakeStore {
        fn new() -> Self {
            Self {
                memories: RefCell::new(HashMap::new()),
                similarity: RefCell::new(HashMap::new()),
            }
        }

        fn insert(&self, mem: Memory, score: f32) {
            self.similarity.borrow_mut().insert(mem.id.clone(), score);
            self.memories.borrow_mut().insert(mem.id.clone(), mem);
        }
    }

    impl MemoryStore for FakeStore {
        fn store(&self, memory: Memory) -> IcmResult<String> {
            let id = memory.id.clone();
            self.memories.borrow_mut().insert(id.clone(), memory);
            Ok(id)
        }

        fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
            Ok(self.memories.borrow().get(id).cloned())
        }

        fn update(&self, memory: &Memory) -> IcmResult<()> {
            self.memories
                .borrow_mut()
                .insert(memory.id.clone(), memory.clone());
            Ok(())
        }

        fn delete(&self, id: &str) -> IcmResult<()> {
            self.memories.borrow_mut().remove(id);
            Ok(())
        }

        fn search_by_keywords(&self, _keywords: &[&str], _limit: usize) -> IcmResult<Vec<Memory>> {
            Ok(Vec::new())
        }

        fn search_fts(&self, _query: &str, _limit: usize) -> IcmResult<Vec<Memory>> {
            Ok(Vec::new())
        }

        fn search_by_embedding(
            &self,
            _embedding: &[f32],
            limit: usize,
        ) -> IcmResult<Vec<(Memory, f32)>> {
            // Return seeded memories sorted by their pre-set similarity desc.
            let mut results: Vec<(Memory, f32)> = self
                .memories
                .borrow()
                .values()
                .cloned()
                .map(|m| {
                    let score = *self.similarity.borrow().get(&m.id).unwrap_or(&0.0);
                    (m, score)
                })
                .collect();
            results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            results.truncate(limit);
            Ok(results)
        }

        fn search_hybrid(
            &self,
            _query: &str,
            embedding: &[f32],
            limit: usize,
        ) -> IcmResult<Vec<(Memory, f32)>> {
            self.search_by_embedding(embedding, limit)
        }

        fn update_access(&self, _id: &str) -> IcmResult<()> {
            Ok(())
        }
        fn batch_update_access(&self, _ids: &[&str]) -> IcmResult<usize> {
            Ok(0)
        }
        fn apply_decay(&self, _decay_factor: f32) -> IcmResult<usize> {
            Ok(0)
        }
        fn prune(&self, _weight_threshold: f32) -> IcmResult<usize> {
            Ok(0)
        }
        fn list_all(&self) -> IcmResult<Vec<Memory>> {
            Ok(self.memories.borrow().values().cloned().collect())
        }
        fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
            Ok(self
                .memories
                .borrow()
                .values()
                .filter(|m| m.topic == topic)
                .cloned()
                .collect())
        }
        fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
            Ok(Vec::new())
        }
        fn consolidate_topic(&self, _topic: &str, _consolidated: Memory) -> IcmResult<()> {
            Ok(())
        }
        fn count(&self) -> IcmResult<usize> {
            Ok(self.memories.borrow().len())
        }
        fn count_by_topic(&self, _topic: &str) -> IcmResult<usize> {
            Ok(0)
        }
        fn stats(&self) -> IcmResult<StoreStats> {
            Ok(StoreStats {
                total_memories: self.memories.borrow().len(),
                total_topics: 0,
                avg_weight: 1.0,
                oldest_memory: None,
                newest_memory: None,
            })
        }
        fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth> {
            Ok(TopicHealth {
                topic: topic.to_string(),
                entry_count: 0,
                avg_weight: 1.0,
                avg_access_count: 0.0,
                oldest: None,
                newest: None,
                last_accessed: None,
                needs_consolidation: false,
                stale_count: 0,
            })
        }
    }

    fn mem_with_emb(topic: &str, summary: &str, imp: Importance) -> Memory {
        let mut m = Memory::new(topic.into(), summary.into(), imp);
        m.embedding = Some(vec![0.1; 8]); // dimensions don't matter with FakeStore
        m
    }

    #[test]
    fn disabled_returns_empty_and_mutates_nothing() {
        let store = FakeStore::new();
        let mut new_mem = mem_with_emb("decisions-icm", "new decision", Importance::High);
        let opts = AutoLinkOptions {
            enabled: false,
            ..Default::default()
        };
        let links = auto_link_memory(&store, &mut new_mem, &opts).unwrap();
        assert!(links.is_empty());
        assert!(new_mem.related_ids.is_empty());
    }

    #[test]
    fn no_embedding_skips_linking() {
        let store = FakeStore::new();
        let mut new_mem = Memory::new("t".into(), "summary".into(), Importance::High);
        // No embedding set.
        let links = auto_link_memory(&store, &mut new_mem, &AutoLinkOptions::default()).unwrap();
        assert!(links.is_empty());
        assert!(new_mem.related_ids.is_empty());
    }

    #[test]
    fn links_candidates_above_threshold() {
        let store = FakeStore::new();
        let m1 = mem_with_emb("t", "related 1", Importance::High);
        let m2 = mem_with_emb("t", "related 2", Importance::Medium);
        let m3 = mem_with_emb("t", "unrelated", Importance::Low);
        store.insert(m1.clone(), 0.92);
        store.insert(m2.clone(), 0.81);
        store.insert(m3.clone(), 0.40); // below threshold

        let mut new_mem = mem_with_emb("t", "new", Importance::High);
        let links = auto_link_memory(&store, &mut new_mem, &AutoLinkOptions::default()).unwrap();

        assert_eq!(links.len(), 2);
        assert!(links.contains(&m1.id));
        assert!(links.contains(&m2.id));
        assert!(!links.contains(&m3.id));
        assert_eq!(new_mem.related_ids, links);
    }

    #[test]
    fn respects_max_links_cap() {
        let store = FakeStore::new();
        for i in 0..10 {
            let m = mem_with_emb("t", &format!("high-sim {i}"), Importance::High);
            store.insert(m, 0.9);
        }
        let mut new_mem = mem_with_emb("t", "new", Importance::High);
        let opts = AutoLinkOptions {
            enabled: true,
            threshold: 0.75,
            max_links: 3,
        };
        let links = auto_link_memory(&store, &mut new_mem, &opts).unwrap();
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn skips_candidate_matching_self_id() {
        let store = FakeStore::new();
        let self_mem = mem_with_emb("t", "self", Importance::High);
        let self_id = self_mem.id.clone();
        store.insert(self_mem, 0.99);

        // Attempt to auto-link a memory with the SAME id (pathological).
        let mut new_mem = mem_with_emb("t", "new", Importance::High);
        new_mem.id = self_id.clone();
        let links = auto_link_memory(&store, &mut new_mem, &AutoLinkOptions::default()).unwrap();
        assert!(links.is_empty(), "should not self-link: {links:?}");
    }

    #[test]
    fn does_not_duplicate_existing_related_ids() {
        let store = FakeStore::new();
        let m1 = mem_with_emb("t", "existing link", Importance::High);
        store.insert(m1.clone(), 0.9);

        let mut new_mem = mem_with_emb("t", "new", Importance::High);
        new_mem.related_ids.push(m1.id.clone());

        let links = auto_link_memory(&store, &mut new_mem, &AutoLinkOptions::default()).unwrap();
        assert!(links.is_empty(), "should skip already-linked id");
        assert_eq!(new_mem.related_ids.len(), 1);
    }

    #[test]
    fn add_backrefs_is_symmetric() {
        let store = FakeStore::new();
        let m1 = mem_with_emb("t", "target 1", Importance::High);
        let m2 = mem_with_emb("t", "target 2", Importance::High);
        store.insert(m1.clone(), 0.0);
        store.insert(m2.clone(), 0.0);

        let new_id = "01NEWMEMORY";
        let linked: Vec<String> = vec![m1.id.clone(), m2.id.clone()];
        let updated = add_backrefs(&store, new_id, &linked).unwrap();
        assert_eq!(updated, 2);

        // Verify each linked memory now points back to new_id.
        let m1_updated = store.get(&m1.id).unwrap().unwrap();
        let m2_updated = store.get(&m2.id).unwrap().unwrap();
        assert!(m1_updated.related_ids.contains(&new_id.to_string()));
        assert!(m2_updated.related_ids.contains(&new_id.to_string()));
    }

    #[test]
    fn add_backrefs_is_idempotent() {
        let store = FakeStore::new();
        let mut m1 = mem_with_emb("t", "already linked", Importance::High);
        m1.related_ids.push("01NEW".into());
        store.insert(m1.clone(), 0.0);

        let updated = add_backrefs(&store, "01NEW", &[m1.id.clone()]).unwrap();
        assert_eq!(
            updated, 0,
            "idempotent — should not re-add existing backref"
        );

        // And the related_ids should still have exactly one entry.
        let reread = store.get(&m1.id).unwrap().unwrap();
        assert_eq!(reread.related_ids.len(), 1);
    }

    #[test]
    fn add_backrefs_silently_ignores_missing_targets() {
        let store = FakeStore::new();
        // No memories inserted — linked_ids point to ghosts.
        let updated = add_backrefs(&store, "01NEW", &["ghost1".into(), "ghost2".into()]).unwrap();
        assert_eq!(updated, 0);
    }
}
