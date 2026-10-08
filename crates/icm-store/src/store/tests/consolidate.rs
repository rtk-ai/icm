//! Per-domain tests for the SQLite backend (`store::tests`).

use super::*;

#[test]
fn test_auto_consolidate_below_threshold() {
    let store = test_store();
    store.store(make_memory("t", "one")).unwrap();
    store.store(make_memory("t", "two")).unwrap();
    // Threshold is 10, so no consolidation
    let result = store.auto_consolidate("t", 10).unwrap();
    assert!(!result);
    assert_eq!(store.count_by_topic("t").unwrap(), 2);
}

#[test]
fn test_auto_consolidate_above_threshold() {
    let store = test_store();
    for i in 0..12 {
        store
            .store(make_memory("bulk", &format!("entry {i}")))
            .unwrap();
    }
    let result = store.auto_consolidate("bulk", 10).unwrap();
    assert!(result);
    assert_eq!(store.count_by_topic("bulk").unwrap(), 1);
}

/// The store reads a topic 500 memories at a time. The rollup replaces the
/// memories that were read; the ones past the read used to be deleted with
/// the rest of the topic without ever being looked at.
#[test]
fn auto_consolidate_only_removes_the_memories_it_read() {
    let store = test_store();
    for i in 0..520 {
        store
            .store(make_memory("many", &format!("fact {i:03}")))
            .unwrap();
    }
    assert!(store.auto_consolidate("many", 10).unwrap());
    // 500 rows read and replaced by the rollup; the 20 that were never read
    // are still there.
    assert_eq!(store.count_by_topic("many").unwrap(), 1 + 20);
}

#[test]
fn test_auto_consolidate_with_embedder_attaches_embedding() {
    // Audit M2/AC2: the embedder-aware variant must produce a
    // consolidated memory that is recall-ready (embedding != None).
    struct StubEmbedder;
    impl icm_core::Embedder for StubEmbedder {
        fn embed(&self, _text: &str) -> IcmResult<Vec<f32>> {
            Ok(vec![0.42; icm_core::DEFAULT_EMBEDDING_DIMS])
        }
        fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|_| vec![0.42; icm_core::DEFAULT_EMBEDDING_DIMS])
                .collect())
        }
        fn dimensions(&self) -> usize {
            icm_core::DEFAULT_EMBEDDING_DIMS
        }
    }
    let store = test_store();
    for i in 0..11 {
        store
            .store(make_memory("rolled", &format!("fact {i}")))
            .unwrap();
    }
    let stub = StubEmbedder;
    let did = store
        .auto_consolidate_with_embedder("rolled", 10, Some(&stub))
        .unwrap();
    assert!(did);
    let consolidated = store.get_by_topic("rolled").unwrap();
    assert_eq!(consolidated.len(), 1);
    let embedding = consolidated[0]
        .embedding
        .as_ref()
        .expect("consolidated memory must have an embedding");
    assert_eq!(embedding.len(), icm_core::DEFAULT_EMBEDDING_DIMS);
    assert!((embedding[0] - 0.42).abs() < 1e-6);
}

#[test]
fn test_apply_decay_with_aggressive_factor() {
    let store = test_store();
    store.store(make_memory("t", "decayable")).unwrap();
    let affected = store.apply_decay(0.5).unwrap();
    assert!(affected > 0);
    let mems = store.get_by_topic("t").unwrap();
    assert!(mems[0].weight < 1.0);
}

#[test]
fn test_prune_low_weight() {
    let store = test_store();
    store.store(make_memory("t", "will be pruned")).unwrap();
    // Apply aggressive decay
    store.apply_decay(0.01).unwrap();
    let pruned = store.prune(0.5).unwrap();
    assert!(pruned > 0);
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn test_list_topics_multiple() {
    let store = test_store();
    store.store(make_memory("alpha", "a")).unwrap();
    store.store(make_memory("beta", "b")).unwrap();
    store.store(make_memory("alpha", "c")).unwrap();
    let topics = store.list_topics().unwrap();
    assert_eq!(topics.len(), 2);
}

#[test]
fn test_stats_multi_topic() {
    let store = test_store();
    store.store(make_memory("t1", "one")).unwrap();
    store.store(make_memory("t2", "two")).unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(stats.total_memories, 2);
    assert_eq!(stats.total_topics, 2);
}

#[test]
fn test_get_by_topic_prefix() {
    let store = test_store();
    store
        .store(make_memory("project:web", "web stuff"))
        .unwrap();
    store
        .store(make_memory("project:api", "api stuff"))
        .unwrap();
    store.store(make_memory("other", "unrelated")).unwrap();
    let results = store.get_by_topic_prefix("project:*").unwrap();
    assert_eq!(results.len(), 2);
}

// expand_with_neighbors
