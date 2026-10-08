//! Per-domain tests for the SQLite backend (`store::tests`).

use super::*;

#[test]
fn test_bulk_insert_100() {
    let store = test_store();
    for i in 0..100 {
        store
            .store(make_memory("bulk", &format!("memory number {i}")))
            .unwrap();
    }
    assert_eq!(store.count().unwrap(), 100);
    let by_topic = store.get_by_topic("bulk").unwrap();
    assert_eq!(by_topic.len(), 100);
}

#[test]
fn test_fts_search_many_entries() {
    let store = test_store();
    for i in 0..50 {
        store
            .store(make_memory(
                "lang",
                &format!("programming language number {i}"),
            ))
            .unwrap();
    }
    store
        .store(make_memory(
            "unique",
            "Rust is a memory-safe systems language",
        ))
        .unwrap();

    let results = store.search_fts("memory-safe systems", 10).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].topic, "unique");
}

#[test]
fn test_decay_bulk() {
    let store = test_store();
    for i in 0..50 {
        let mut mem = make_memory("decay", &format!("entry {i}"));
        if i % 5 == 0 {
            mem.importance = Importance::Critical;
        }
        store.store(mem).unwrap();
    }
    // 10 critical, 40 non-critical
    let affected = store.apply_decay(0.9).unwrap();
    assert_eq!(affected, 40);
}

#[test]
fn test_prune_leaves_important() {
    let store = test_store();
    for i in 0..20 {
        let mut mem = make_memory("prune", &format!("entry {i}"));
        mem.weight = if i < 10 { 0.01 } else { 0.5 };
        store.store(mem).unwrap();
    }
    let pruned = store.prune(0.1).unwrap();
    assert_eq!(pruned, 10);
    assert_eq!(store.count().unwrap(), 10);
}

#[test]
fn test_many_topics_listing() {
    let store = test_store();
    for i in 0..30 {
        store
            .store(make_memory(&format!("topic-{i}"), &format!("content {i}")))
            .unwrap();
    }
    let topics = store.list_topics().unwrap();
    assert_eq!(topics.len(), 30);
}

#[test]
fn test_consolidate_large_topic() {
    let store = test_store();
    for i in 0..25 {
        store
            .store(make_memory("big-topic", &format!("detail {i}")))
            .unwrap();
    }
    let consolidated = make_memory("big-topic", "consolidated summary of 25 entries");
    store.consolidate_topic("big-topic", consolidated).unwrap();
    let remaining = store.get_by_topic("big-topic").unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].summary.contains("consolidated"));
}

#[test]
fn test_get_by_topic_returns_sorted_by_weight() {
    let store = test_store();
    let mut low = make_memory("ux", "low weight");
    low.weight = 0.3;
    store.store(low).unwrap();

    let mut high = make_memory("ux", "high weight");
    high.weight = 0.9;
    store.store(high).unwrap();

    let results = store.get_by_topic("ux").unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].weight >= results[1].weight);
}

#[test]
fn test_update_access_increments_correctly() {
    let store = test_store();
    let mem = make_memory("ux", "access counter");
    let id = mem.id.clone();
    store.store(mem).unwrap();

    for _ in 0..5 {
        store.update_access(&id).unwrap();
    }
    let retrieved = store.get(&id).unwrap().unwrap();
    assert_eq!(retrieved.access_count, 5);
}

#[test]
fn test_stats_on_empty_store() {
    let store = test_store();
    let stats = store.stats().unwrap();
    assert_eq!(stats.total_memories, 0);
    assert_eq!(stats.total_topics, 0);
    assert_eq!(stats.avg_weight, 0.0);
    assert!(stats.oldest_memory.is_none());
    assert!(stats.newest_memory.is_none());
}

#[test]
fn test_double_delete_returns_not_found() {
    let store = test_store();
    let mem = make_memory("ux", "delete twice");
    let id = mem.id.clone();
    store.store(mem).unwrap();

    store.delete(&id).unwrap();
    let result = store.delete(&id);
    assert!(matches!(result, Err(IcmError::NotFound(_))));
}

#[test]
fn test_update_syncs_embedding() {
    let store = test_store();
    let mut mem = make_memory("test", "before update");
    let id = mem.id.clone();
    store.store(mem.clone()).unwrap();

    // Initially no embedding
    assert!(store.get(&id).unwrap().unwrap().embedding.is_none());

    // Update with embedding
    mem.embedding = Some(vec![0.3; 384]);
    store.update(&mem).unwrap();

    let retrieved = store.get(&id).unwrap().unwrap();
    assert!(retrieved.embedding.is_some());

    // Should be findable via vector search
    let results = store.search_by_embedding(&vec![0.3; 384], 5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.id, id);
}

#[test]
fn perf_store_1000() {
    let store = test_store();
    let start = std::time::Instant::now();
    for i in 0..1000 {
        store
            .store(make_memory("perf", &format!("memory number {i}")))
            .unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 2000,
        "1000 stores took {}ms (max 2000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_store_with_embeddings_1000() {
    let store = test_store();
    let start = std::time::Instant::now();
    for i in 0..1000 {
        let mut mem = make_memory("perf", &format!("embedded memory {i}"));
        mem.embedding = Some(vec![0.1; 384]);
        store.store(mem).unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 3000,
        "1000 stores+embedding took {}ms (max 3000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_fts_search_100() {
    let store = test_store();
    for i in 0..500 {
        store
            .store(make_memory(
                "lang",
                &format!("programming language {i} with features"),
            ))
            .unwrap();
    }
    let start = std::time::Instant::now();
    for _ in 0..100 {
        store
            .search_fts("programming language features", 10)
            .unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 1000,
        "100 FTS searches took {}ms (max 1000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_vector_search_100() {
    let store = test_store();
    for i in 0..500 {
        let mut mem = make_memory("vec", &format!("vector memory {i}"));
        let mut emb = vec![0.0; 384];
        emb[i % 384] = 1.0;
        mem.embedding = Some(emb);
        store.store(mem).unwrap();
    }
    let query = vec![0.5; 384];
    let start = std::time::Instant::now();
    for _ in 0..100 {
        store.search_by_embedding(&query, 10).unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 5000,
        "100 vector searches took {}ms (max 5000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_hybrid_search_100() {
    let store = test_store();
    for i in 0..500 {
        let mut mem = make_memory("hybrid", &format!("hybrid searchable memory {i}"));
        mem.embedding = Some(vec![0.1; 384]);
        store.store(mem).unwrap();
    }
    let query_emb = vec![0.1; 384];
    let start = std::time::Instant::now();
    for _ in 0..100 {
        store
            .search_hybrid("hybrid searchable", &query_emb, 10)
            .unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 10000,
        "100 hybrid searches took {}ms (max 10000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_decay_1000() {
    let store = test_store();
    for i in 0..1000 {
        store
            .store(make_memory("decay", &format!("decayable {i}")))
            .unwrap();
    }
    let start = std::time::Instant::now();
    store.apply_decay(0.95).unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 500,
        "decay on 1000 memories took {}ms (max 500ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_get_by_id_1000() {
    let store = test_store();
    let mut ids = Vec::new();
    for i in 0..1000 {
        let mem = make_memory("get", &format!("lookup {i}"));
        let id = mem.id.clone();
        store.store(mem).unwrap();
        ids.push(id);
    }
    let start = std::time::Instant::now();
    for id in &ids {
        store.get(id).unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 1000,
        "1000 gets took {}ms (max 1000ms)",
        elapsed.as_millis()
    );
}

/// Measure cache-hot vs cache-cold `get()` cost.
///
/// Run with `cargo test -p icm-store -- --ignored --nocapture
/// bench_cache_hit_vs_miss`. Informational only — no assertion.
#[test]
#[ignore]
fn bench_cache_hit_vs_miss() {
    let store = test_store();
    let mut ids: Vec<String> = Vec::new();
    for i in 0..50 {
        let mem = make_memory("bench", &format!("memory {i}"));
        ids.push(mem.id.clone());
        store.store(mem).unwrap();
    }

    // Cold: clear cache, read each id once. Mix of cache-fill + DB hit.
    store.cache_clear();
    let cold = std::time::Instant::now();
    for id in &ids {
        store.get(id).unwrap();
    }
    let cold_elapsed = cold.elapsed();

    // Warm: cache already populated by the cold pass; 1000 iterations
    // of the same id set are all cache hits.
    let warm = std::time::Instant::now();
    for _ in 0..1000 {
        for id in &ids {
            store.get(id).unwrap();
        }
    }
    let warm_elapsed = warm.elapsed();
    let warm_per_get_ns = warm_elapsed.as_nanos() / (1000 * ids.len() as u128);
    let cold_per_get_ns = cold_elapsed.as_nanos() / ids.len() as u128;

    eprintln!("=== bench_cache_hit_vs_miss ===");
    eprintln!("  cold (50 fills,   first read each): {cold_per_get_ns} ns/get");
    eprintln!("  warm (50000 hits, all cache reads): {warm_per_get_ns} ns/get");
    if warm_per_get_ns > 0 {
        eprintln!(
            "  speedup on hot reads: {:.1}x",
            cold_per_get_ns as f64 / warm_per_get_ns as f64
        );
    }
}

/// Measure batched `get_many` vs per-id `get` round-trips.
///
/// Run with `cargo test -p icm-store -- --ignored --nocapture
/// bench_get_many_vs_n_plus_one`. Informational only.
#[test]
#[ignore]
fn bench_get_many_vs_n_plus_one() {
    let store = test_store();
    let mut ids: Vec<String> = Vec::new();
    for i in 0..50 {
        let mem = make_memory("bench", &format!("entry {i}"));
        ids.push(mem.id.clone());
        store.store(mem).unwrap();
    }
    let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();

    // Cold batched fetch.
    store.cache_clear();
    let t = std::time::Instant::now();
    let got = store.get_many(&id_refs).unwrap();
    let batch_elapsed = t.elapsed();
    assert_eq!(got.len(), 50);

    // Cold N+1 fetch.
    store.cache_clear();
    let t = std::time::Instant::now();
    for id in &id_refs {
        store.get(id).unwrap();
    }
    let n_plus_one_elapsed = t.elapsed();

    eprintln!("=== bench_get_many_vs_n_plus_one (50 ids) ===");
    eprintln!("  batched get_many: {} µs", batch_elapsed.as_micros());
    eprintln!("  N+1 individual:   {} µs", n_plus_one_elapsed.as_micros());
    if batch_elapsed.as_micros() > 0 {
        eprintln!(
            "  speedup: {:.1}x",
            n_plus_one_elapsed.as_micros() as f64 / batch_elapsed.as_micros() as f64
        );
    }
}

// === Additional performance tests ===

#[test]
fn perf_search_fts_latency_with_1000_entries() {
    let store = test_store();
    for i in 0..1000 {
        store
            .store(make_memory(
                &format!("topic-{}", i % 50),
                &format!(
                    "detailed description about system component {i} with features and architecture"
                ),
            ))
            .unwrap();
    }
    let start = std::time::Instant::now();
    for _ in 0..50 {
        let results = store
            .search_fts("system component architecture", 10)
            .unwrap();
        assert!(!results.is_empty());
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 2000,
        "50 FTS searches over 1000 entries took {}ms (max 2000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_sequential_store_operations_rapid() {
    let store = test_store();
    let start = std::time::Instant::now();
    // Simulate concurrent-like rapid sequential operations mixing stores, gets, searches
    for i in 0..500 {
        let mem = make_memory("rapid", &format!("rapid entry {i}"));
        let id = mem.id.clone();
        store.store(mem).unwrap();
        // Interleave reads
        if i % 5 == 0 {
            store.get(&id).unwrap();
        }
        // Interleave searches
        if i % 20 == 0 {
            store.search_fts("rapid entry", 5).unwrap();
        }
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 3000,
        "500 mixed store/get/search ops took {}ms (max 3000ms)",
        elapsed.as_millis()
    );
    assert_eq!(store.count().unwrap(), 500);
}

#[test]
fn perf_memoir_creation_and_concept_linking() {
    let store = test_store();
    let start = std::time::Instant::now();

    // Create 10 memoirs, each with 10 concepts and links between them
    for m in 0..10 {
        let m_id = store
            .create_memoir(make_memoir(&format!("perf-memoir-{m}")))
            .unwrap();
        let mut concept_ids = Vec::new();
        for c in 0..10 {
            let c_id = store
                .add_concept(make_concept(
                    &m_id,
                    &format!("concept-{m}-{c}"),
                    &format!("Definition for concept {c} in memoir {m}"),
                ))
                .unwrap();
            concept_ids.push(c_id);
        }
        // Link each concept to the next one (chain)
        for w in concept_ids.windows(2) {
            store
                .add_link(ConceptLink::new(
                    w[0].clone(),
                    w[1].clone(),
                    Relation::DependsOn,
                ))
                .unwrap();
        }
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 3000,
        "10 memoirs x 10 concepts + links took {}ms (max 3000ms)",
        elapsed.as_millis()
    );

    // Verify structure
    let memoirs = store.list_memoirs().unwrap();
    assert_eq!(memoirs.len(), 10);
}

#[test]
fn perf_neighborhood_bfs_large_graph() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("large-graph")).unwrap();

    // Create a large graph: 50 concepts in a chain
    let mut concept_ids = Vec::new();
    for i in 0..50 {
        let c_id = store
            .add_concept(make_concept(
                &m_id,
                &format!("node-{i}"),
                &format!("Graph node number {i}"),
            ))
            .unwrap();
        concept_ids.push(c_id);
    }
    // Chain: 0->1->2->...->49
    for w in concept_ids.windows(2) {
        store
            .add_link(ConceptLink::new(
                w[0].clone(),
                w[1].clone(),
                Relation::DependsOn,
            ))
            .unwrap();
    }
    // Add some cross-links for complexity
    for i in (0..50).step_by(5) {
        if i + 10 < 50 {
            store
                .add_link(ConceptLink::new(
                    concept_ids[i].clone(),
                    concept_ids[i + 10].clone(),
                    Relation::RelatedTo,
                ))
                .unwrap();
        }
    }

    let start = std::time::Instant::now();
    // BFS traversal at various depths
    for depth in 1..=5 {
        let (concepts, links) = store.get_neighborhood(&concept_ids[0], depth).unwrap();
        assert!(!concepts.is_empty());
        assert!(!links.is_empty());
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 2000,
        "BFS traversals (depth 1-5) on 50-node graph took {}ms (max 2000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_embedding_storage_batch() {
    let store = test_store();
    let start = std::time::Instant::now();
    for i in 0..500 {
        let mut mem = make_memory("embed-perf", &format!("embedding batch entry {i}"));
        let mut emb = vec![0.0f32; 384];
        // Vary embeddings so they're not all identical
        emb[i % 384] = 1.0;
        emb[(i * 7) % 384] = 0.5;
        mem.embedding = Some(emb);
        store.store(mem).unwrap();
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 3000,
        "500 stores with embeddings took {}ms (max 3000ms)",
        elapsed.as_millis()
    );

    // Now search
    let query = vec![0.5f32; 384];
    let search_start = std::time::Instant::now();
    for _ in 0..50 {
        let results = store.search_by_embedding(&query, 10).unwrap();
        assert!(!results.is_empty());
    }
    let search_elapsed = search_start.elapsed();
    assert!(
        search_elapsed.as_millis() < 3000,
        "50 vector searches over 500 entries took {}ms (max 3000ms)",
        search_elapsed.as_millis()
    );
}

#[test]
fn perf_keyword_search_with_many_entries() {
    let store = test_store();
    for i in 0..1000 {
        let mut mem = make_memory(
            &format!("kw-topic-{}", i % 20),
            &format!("keyword searchable entry number {i}"),
        );
        mem.keywords = vec![
            format!("keyword-{}", i % 10),
            format!("category-{}", i % 5),
            "common".into(),
        ];
        store.store(mem).unwrap();
    }

    let start = std::time::Instant::now();
    for i in 0..50 {
        let results = store
            .search_by_keywords(&[&format!("keyword-{}", i % 10)], 10)
            .unwrap();
        assert!(!results.is_empty());
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 2000,
        "50 keyword searches over 1000 entries took {}ms (max 2000ms)",
        elapsed.as_millis()
    );
}

#[test]
fn perf_consolidate_large_topic_timing() {
    let store = test_store();
    for i in 0..100 {
        store
            .store(make_memory(
                "consolidate-perf",
                &format!("detail entry {i} with various information"),
            ))
            .unwrap();
    }
    let start = std::time::Instant::now();
    let consolidated = make_memory("consolidate-perf", "All 100 entries consolidated");
    store
        .consolidate_topic("consolidate-perf", consolidated)
        .unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 1000,
        "Consolidating 100 entries took {}ms (max 1000ms)",
        elapsed.as_millis()
    );
    assert_eq!(store.get_by_topic("consolidate-perf").unwrap().len(), 1);
}

#[test]
fn perf_list_topics_many() {
    let store = test_store();
    // Create 200 distinct topics
    for i in 0..200 {
        store
            .store(make_memory(
                &format!("distinct-topic-{i}"),
                &format!("content for topic {i}"),
            ))
            .unwrap();
    }
    let start = std::time::Instant::now();
    for _ in 0..50 {
        let topics = store.list_topics().unwrap();
        assert_eq!(topics.len(), 200);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 1000,
        "50 list_topics calls over 200 topics took {}ms (max 1000ms)",
        elapsed.as_millis()
    );
}
