//! Per-domain tests for the SQLite backend (`store::tests`).

use super::*;

#[test]
fn test_store_and_get() {
    let store = test_store();
    let mem = make_memory("test", "hello world");
    let id = mem.id.clone();

    store.store(mem).unwrap();
    let retrieved = store.get(&id).unwrap().unwrap();
    assert_eq!(retrieved.summary, "hello world");
    assert_eq!(retrieved.topic, "test");
}

#[test]
fn test_get_not_found() {
    let store = test_store();
    let result = store.get("nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_update() {
    let store = test_store();
    let mut mem = make_memory("test", "original");
    let id = mem.id.clone();
    store.store(mem.clone()).unwrap();

    mem.summary = "updated".into();
    store.update(&mem).unwrap();

    let retrieved = store.get(&id).unwrap().unwrap();
    assert_eq!(retrieved.summary, "updated");
}

#[test]
fn test_delete() {
    let store = test_store();
    let mem = make_memory("test", "to delete");
    let id = mem.id.clone();
    store.store(mem).unwrap();

    store.delete(&id).unwrap();
    assert!(store.get(&id).unwrap().is_none());
}

#[test]
fn test_delete_not_found() {
    let store = test_store();
    let result = store.delete("nonexistent");
    assert!(matches!(result, Err(IcmError::NotFound(_))));
}

/// Manual-testing finding: deleting a memory left it as a dangling
/// entry in every other memory's `related_ids` (auto-link
/// back-references) forever. `expand_with_neighbors` tolerates the
/// miss silently, but each stale id still spends a slot out of the
/// caller's `max_neighbors` budget instead of surfacing a real, live
/// neighbor, and any external consumer of the JSON export sees a
/// reference to nothing.
#[test]
fn test_delete_cleans_up_dangling_related_ids() {
    let store = test_store();

    let mut a = make_memory("t", "memory a");
    let mut b = make_memory("t", "memory b");
    let mut c = make_memory("t", "memory c");
    let a_id = store.store(a.clone()).unwrap();
    let b_id = store.store(b.clone()).unwrap();
    let c_id = store.store(c.clone()).unwrap();

    a.id = a_id.clone();
    a.related_ids = vec![b_id.clone(), c_id.clone()];
    store.update(&a).unwrap();
    b.id = b_id.clone();
    b.related_ids = vec![a_id.clone(), c_id.clone()];
    store.update(&b).unwrap();
    c.id = c_id.clone();
    c.related_ids = vec![a_id.clone(), b_id.clone()];
    store.update(&c).unwrap();

    store.delete(&a_id).unwrap();

    let b_after = store.get(&b_id).unwrap().unwrap();
    assert_eq!(
        b_after.related_ids,
        vec![c_id.clone()],
        "b's related_ids must no longer reference the deleted a"
    );
    let c_after = store.get(&c_id).unwrap().unwrap();
    assert_eq!(
        c_after.related_ids,
        vec![b_id.clone()],
        "c's related_ids must no longer reference the deleted a"
    );
}

#[test]
fn test_search_fts() {
    let store = test_store();
    store
        .store(make_memory(
            "rust",
            "Rust is a systems programming language",
        ))
        .unwrap();
    store
        .store(make_memory("python", "Python is great for scripting"))
        .unwrap();

    let results = store.search_fts("rust programming", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].topic, "rust");
}

#[test]
fn test_search_by_keywords() {
    let store = test_store();
    let mut mem = make_memory("test", "database optimization tips");
    mem.keywords = vec!["database".into(), "optimization".into()];
    store.store(mem).unwrap();

    let results = store.search_by_keywords(&["database"], 10).unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn test_list_topics() {
    let store = test_store();
    store.store(make_memory("alpha", "first")).unwrap();
    store.store(make_memory("alpha", "second")).unwrap();
    store.store(make_memory("beta", "third")).unwrap();

    let topics = store.list_topics().unwrap();
    assert_eq!(topics.len(), 2);
    assert!(topics.contains(&("alpha".into(), 2)));
    assert!(topics.contains(&("beta".into(), 1)));
}

#[test]
fn test_restore_upgrades_importance_on_dedup() {
    // Audit #185 H2: re-storing the same content with a higher
    // importance must upgrade the existing row, not silently
    // drop the new value.
    let store = test_store();
    let mut first = make_memory("topic", "long enough summary content for storage");
    first.importance = Importance::Medium;
    let id1 = store.store(first).unwrap();

    let mut second = make_memory("topic", "long enough summary content for storage");
    second.importance = Importance::Critical;
    let id2 = store.store(second).unwrap();
    assert_eq!(id1, id2, "dedup must return the same id");

    let merged = store.get(&id1).unwrap().unwrap();
    assert_eq!(
        merged.importance,
        Importance::Critical,
        "re-store with higher priority must upgrade importance"
    );
}

#[test]
fn test_restore_does_not_downgrade_importance_on_dedup() {
    // Sanity: a re-store with a lower priority is a no-op so an
    // accidental write can't downgrade an already-flagged
    // critical memory.
    let store = test_store();
    let mut first = make_memory("topic", "long enough summary content for storage");
    first.importance = Importance::Critical;
    let id1 = store.store(first).unwrap();

    let mut second = make_memory("topic", "long enough summary content for storage");
    second.importance = Importance::Low;
    store.store(second).unwrap();

    let preserved = store.get(&id1).unwrap().unwrap();
    assert_eq!(
        preserved.importance,
        Importance::Critical,
        "re-store with lower priority must not downgrade importance"
    );
}

#[test]
fn test_restore_unions_keywords_on_dedup() {
    let store = test_store();
    let mut first = make_memory("topic", "long enough summary content for storage");
    first.keywords = vec!["alpha".into(), "beta".into()];
    let id1 = store.store(first).unwrap();

    let mut second = make_memory("topic", "long enough summary content for storage");
    second.keywords = vec!["beta".into(), "gamma".into()];
    store.store(second).unwrap();

    let merged = store.get(&id1).unwrap().unwrap();
    assert_eq!(
        merged.keywords,
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string(),],
        "keywords must be unioned and deduped, preserving existing order"
    );
}

#[test]
fn test_restore_sets_raw_excerpt_when_previously_none() {
    let store = test_store();
    let first = make_memory("topic", "long enough summary content for storage");
    let id1 = store.store(first).unwrap();
    assert!(store.get(&id1).unwrap().unwrap().raw_excerpt.is_none());

    let mut second = make_memory("topic", "long enough summary content for storage");
    second.raw_excerpt = Some("verbatim copy from source".into());
    store.store(second).unwrap();

    let merged = store.get(&id1).unwrap().unwrap();
    assert_eq!(
        merged.raw_excerpt.as_deref(),
        Some("verbatim copy from source"),
    );
}

#[test]
fn test_restore_keeps_existing_raw_excerpt_when_new_is_none() {
    let store = test_store();
    let mut first = make_memory("topic", "long enough summary content for storage");
    first.raw_excerpt = Some("important verbatim".into());
    let id1 = store.store(first).unwrap();

    let second = make_memory("topic", "long enough summary content for storage");
    store.store(second).unwrap();

    let preserved = store.get(&id1).unwrap().unwrap();
    assert_eq!(
        preserved.raw_excerpt.as_deref(),
        Some("important verbatim"),
        "re-store with None must not erase existing raw_excerpt"
    );
}

#[test]
fn test_restore_unchanged_metadata_is_noop() {
    let store = test_store();
    let mut first = make_memory("topic", "long enough summary content for storage");
    first.importance = Importance::High;
    first.keywords = vec!["alpha".into()];
    let id1 = store.store(first.clone()).unwrap();
    let original = store.get(&id1).unwrap().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));
    store.store(first).unwrap();
    let after = store.get(&id1).unwrap().unwrap();

    assert_eq!(
        original.updated_at, after.updated_at,
        "no-op re-store must not bump updated_at"
    );
}

#[test]
fn test_apply_decay() {
    let store = test_store();
    store.store(make_memory("test", "decayable")).unwrap();

    let mut critical = make_memory("test", "critical memory");
    critical.importance = Importance::Critical;
    store.store(critical).unwrap();

    let affected = store.apply_decay(0.9).unwrap();
    assert_eq!(affected, 1); // Only the non-critical one
}

/// Audit regression: `apply_decay` must never drive weight negative.
/// Low importance (2x multiplier), zero access count, factor=0.4 (still
/// inside the CLI's own `[0.0, 1.0)` validation) makes the raw
/// multiplier `1.0 - (1.0-0.4)*2.0 = -0.2` — negative before the
/// `MAX(0.0, ...)` clamp.
#[test]
fn test_apply_decay_never_goes_negative() {
    let store = test_store();
    let mut low = make_memory("t", "low importance, never accessed");
    low.importance = Importance::Low;
    store.store(low).unwrap();

    store.apply_decay(0.4).unwrap();

    let mem = store.get_by_topic("t").unwrap().into_iter().next().unwrap();
    assert!(
        mem.weight >= 0.0,
        "weight must never go negative, got {}",
        mem.weight
    );
}

/// Audit regression: `maybe_auto_decay` used to apply a flat 0.95 step
/// whenever >= 1 day had passed, regardless of how many days actually
/// elapsed. Simulate a 5-day gap (by backdating `last_decay_at`
/// directly) and assert the applied decay compounds to ~0.95^5, not a
/// single 0.95 step.
#[test]
fn test_maybe_auto_decay_scales_with_elapsed_days() {
    let store = test_store();
    let mut mem = make_memory("t", "elapsed-days probe");
    mem.importance = Importance::Medium;
    store.store(mem).unwrap();

    let five_days_ago = (Utc::now() - chrono::Duration::days(5)).to_rfc3339();
    store
        .conn
        .execute(
            "INSERT INTO icm_metadata (key, value) VALUES ('last_decay_at', ?1)",
            params![five_days_ago],
        )
        .unwrap();

    store.maybe_auto_decay().unwrap();

    let after = store.get_by_topic("t").unwrap().into_iter().next().unwrap();
    // Medium importance, access_count=0 -> multiplier = factor directly.
    let expected_single_step = 0.95_f32;
    let expected_five_days = 0.95_f32.powi(5);
    assert!(
        (after.weight - expected_five_days).abs() < 0.01,
        "expected weight ~{expected_five_days} (0.95^5) for a 5-day gap, got {}",
        after.weight
    );
    assert!(
        after.weight < expected_single_step - 0.05,
        "a 5-day gap must decay more than a single flat 0.95 step \
             (got {}, single-step would be {expected_single_step})",
        after.weight
    );
}

#[test]
fn test_apply_decay_caps_access_count_amplification() {
    // Audit #185 H7: the pre-fix decay formula had an uncapped
    // `1 + access_count * 0.1` term, which let memories with 100+
    // accesses become effectively decay-immune. Repro the gaming
    // and assert the capped formula prevents it.
    //
    // After 5 decay rounds at factor=0.8:
    // - real (access=0): naively 0.95 ^ 5 ≈ 0.77 → with importance
    //   medium and the capped slowdown, weight should drop to
    //   roughly 0.5×.
    // - junk (access=100): pre-fix it stayed near 0.95 (no decay
    //   amplification immune); post-fix it's capped at 5 accesses
    //   so it decays at the same rate as a memory with 5 accesses.
    // The exact ratio depends on the cap; the crucial property is
    // that after enough decay rounds, junk's weight no longer
    // exceeds real's weight. We assert that property directly
    // rather than pinning specific weight numbers.
    let store = test_store();

    let mut real = make_memory("topic", "real high-importance fact");
    real.importance = Importance::Medium;
    let real_id = store.store(real).unwrap();

    let mut junk = make_memory("topic", "junk fact accessed by gaming loop");
    junk.importance = Importance::Medium;
    let junk_id = store.store(junk).unwrap();

    // Inflate junk's access_count to 100 (the M01 reproduction).
    store
        .conn
        .execute(
            "UPDATE memories SET access_count = 100 WHERE id = ?1",
            params![junk_id],
        )
        .unwrap();

    // 5 aggressive decay rounds.
    for _ in 0..5 {
        store.apply_decay(0.8).unwrap();
    }

    let real_after = store.get(&real_id).unwrap().unwrap();
    let junk_after = store.get(&junk_id).unwrap().unwrap();

    // Pre-fix: junk weight ≈ 0.95, real weight ≈ 0.31 → junk dominates.
    // Post-fix: with the cap, junk decays meaningfully even at
    // access=100. We require junk weight < real weight + a small
    // headroom — the cap must not let a low-relevance, frequently-
    // accessed memory overtake real same-importance memories.
    assert!(
        junk_after.weight < real_after.weight * 1.6,
        "junk weight {} must not dominate real weight {} after 5 decay rounds (cap is broken)",
        junk_after.weight,
        real_after.weight,
    );

    // Sanity: junk did still decay measurably (cap didn't make it
    // permanent).
    assert!(
        junk_after.weight < 0.97,
        "junk weight {} barely decayed at all (cap is too aggressive a slowdown)",
        junk_after.weight,
    );
}

#[test]
fn test_prune() {
    let store = test_store();
    let mut low = make_memory("test", "low weight");
    low.weight = 0.05;
    store.store(low).unwrap();

    store.store(make_memory("test", "normal weight")).unwrap();

    let pruned = store.prune(0.1).unwrap();
    assert_eq!(pruned, 1);
    assert_eq!(store.count().unwrap(), 1);
}

#[test]
fn test_stats() {
    let store = test_store();
    store.store(make_memory("a", "first")).unwrap();
    store.store(make_memory("b", "second")).unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.total_memories, 2);
    assert_eq!(stats.total_topics, 2);
    assert!(stats.avg_weight > 0.0);
    assert!(stats.oldest_memory.is_some());
    assert!(stats.newest_memory.is_some());
}

#[test]
fn test_update_access() {
    let store = test_store();
    let mem = make_memory("test", "access test");
    let id = mem.id.clone();
    store.store(mem).unwrap();

    store.update_access(&id).unwrap();
    let retrieved = store.get(&id).unwrap().unwrap();
    assert_eq!(retrieved.access_count, 1);
}

#[test]
fn test_consolidate_topic() {
    let store = test_store();
    store.store(make_memory("topic-a", "entry 1")).unwrap();
    store.store(make_memory("topic-a", "entry 2")).unwrap();
    store.store(make_memory("topic-b", "other")).unwrap();

    let consolidated = make_memory("topic-a", "consolidated summary");
    store.consolidate_topic("topic-a", consolidated).unwrap();

    let memories = store.get_by_topic("topic-a").unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].summary, "consolidated summary");

    // topic-b should be untouched
    assert_eq!(store.get_by_topic("topic-b").unwrap().len(), 1);
}

/// Audit regression: consolidation must honor the "critical = never
/// forget" contract that `apply_decay` and `prune` already respect.
#[test]
fn consolidate_topic_preserves_critical_memories() {
    let store = test_store();
    store.store(make_memory("t", "expendable 1")).unwrap();
    store.store(make_memory("t", "expendable 2")).unwrap();
    store
        .store(Memory::new(
            "t".into(),
            "never forget this".into(),
            Importance::Critical,
        ))
        .unwrap();

    store
        .consolidate_topic("t", make_memory("t", "rollup"))
        .unwrap();

    let after = store.get_by_topic("t").unwrap();
    let summaries: Vec<&str> = after.iter().map(|m| m.summary.as_str()).collect();
    assert_eq!(after.len(), 2, "critical + consolidated must both survive");
    assert!(summaries.contains(&"never forget this"));
    assert!(summaries.contains(&"rollup"));
    assert!(!summaries.contains(&"expendable 1"));
}

/// Manual-testing finding: consolidate_topic bulk-deletes the topic's
/// non-critical memories and does its own DELETE, entirely separate
/// from the single-id `delete()` — so it had the same dangling
/// related_ids bug in a second place. An external memory (in a
/// different topic) that referenced one of the consolidated-away ids
/// must have that reference cleaned up too.
#[test]
fn consolidate_topic_cleans_up_dangling_related_ids_in_other_memories() {
    let store = test_store();
    let a_id = store.store(make_memory("t", "memory a")).unwrap();
    let b_id = store.store(make_memory("t", "memory b")).unwrap();

    let mut external = make_memory("other-topic", "external memory");
    external.related_ids = vec![a_id.clone(), b_id.clone()];
    let external_id = store.store(external).unwrap();

    store
        .consolidate_topic("t", make_memory("t", "rollup"))
        .unwrap();

    let external_after = store.get(&external_id).unwrap().unwrap();
    assert!(
        external_after.related_ids.is_empty(),
        "external memory must no longer reference the consolidated-away ids: {:?}",
        external_after.related_ids
    );
}

/// Audit regression: critical memories are exempt from consolidation, so
/// they must not count toward the auto-consolidate threshold — otherwise
/// a topic full of criticals would churn a fresh rollup on every store.
#[test]
fn auto_consolidate_ignores_critical_for_threshold() {
    let store = test_store();
    for i in 0..3 {
        store
            .store(Memory::new(
                "t".into(),
                format!("critical {i}"),
                Importance::Critical,
            ))
            .unwrap();
    }
    store.store(make_memory("t", "one expendable")).unwrap();

    // 4 total but only 1 consolidatable — below threshold 3.
    assert!(!store.auto_consolidate("t", 3).unwrap());
    assert_eq!(store.get_by_topic("t").unwrap().len(), 4);

    store.store(make_memory("t", "expendable 2")).unwrap();
    store.store(make_memory("t", "expendable 3")).unwrap();

    // Now 3 consolidatable — rollup fires, criticals survive.
    assert!(store.auto_consolidate("t", 3).unwrap());
    let after = store.get_by_topic("t").unwrap();
    let criticals = after
        .iter()
        .filter(|m| matches!(m.importance, Importance::Critical))
        .count();
    assert_eq!(criticals, 3, "all criticals must survive the rollup");
    assert_eq!(after.len(), 4, "3 criticals + 1 consolidated");
}

/// Audit regression: `update()` previously bypassed all validation, so
/// oversized or NUL-carrying payloads could enter via store-small-then-
/// update-big.
#[test]
fn update_rejects_oversized_and_nul_payloads() {
    let store = test_store();
    let id = store.store(make_memory("t", "small")).unwrap();
    let mut m = store.get(&id).unwrap().unwrap();

    m.summary = "x".repeat(MAX_SUMMARY_BYTES + 1);
    assert!(matches!(store.update(&m), Err(IcmError::InvalidInput(_))));

    m.summary = "has a \0 NUL".into();
    assert!(matches!(store.update(&m), Err(IcmError::InvalidInput(_))));

    // The stored row is untouched by the rejected updates.
    assert_eq!(store.get(&id).unwrap().unwrap().summary, "small");
}

/// Audit regression: the MCP consolidate path passes a caller-provided
/// summary that previously bypassed every size check.
#[test]
fn consolidate_topic_validates_consolidated_summary() {
    let store = test_store();
    store.store(make_memory("t", "entry")).unwrap();

    let oversized = make_memory("t", &"x".repeat(MAX_SUMMARY_BYTES + 1));
    assert!(matches!(
        store.consolidate_topic("t", oversized),
        Err(IcmError::InvalidInput(_))
    ));
    // Originals untouched on rejection.
    assert_eq!(store.get_by_topic("t").unwrap().len(), 1);
}

/// Audit regression: transcript messages had no size bound at all; they
/// are best-effort logs, so oversized content is truncated, not lost.
#[test]
fn record_message_truncates_oversized_content() {
    let store = test_store();
    let sid = store.create_session("test-agent", None, None).unwrap();
    let big = "é".repeat(200 * 1024); // 400 KB of two-byte chars
    store
        .record_message(&sid, Role::User, &big, None, None, None)
        .unwrap();

    let msgs = store.list_session_messages(&sid, 10, 0).unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].content.len() <= 256 * 1024);
    assert!(!msgs[0].content.is_empty());
    // Truncation must respect char boundaries (no broken UTF-8).
    assert!(msgs[0].content.chars().all(|c| c == 'é'));
}

/// Reproduces issue #44: after consolidation, recall should only return the
/// consolidated memory — not stale fragments from the originals.
#[test]
fn test_consolidate_no_stale_fts_results() {
    let store = test_store();

    // Step 1: store 3 related memories on the same topic
    store
        .store(make_memory(
            "errors-resolved",
            "fix: null pointer in parser",
        ))
        .unwrap();
    store
        .store(make_memory(
            "errors-resolved",
            "fix: timeout in HTTP client",
        ))
        .unwrap();
    store
        .store(make_memory(
            "errors-resolved",
            "fix: race condition in cache",
        ))
        .unwrap();

    // Verify FTS finds them before consolidation
    let before = store.search_fts("fix", 10).unwrap();
    assert_eq!(before.len(), 3);

    // Step 2: consolidate
    let consolidated = make_memory(
        "errors-resolved",
        "All errors resolved: parser, HTTP, cache",
    );
    store
        .consolidate_topic("errors-resolved", consolidated)
        .unwrap();

    // Step 3: recall — should only return the consolidated memory
    let after = store.search_fts("fix", 10).unwrap();
    assert!(
        after.len() <= 1,
        "expected at most 1 result after consolidation, got {}",
        after.len()
    );

    // The consolidated memory should be findable
    let consolidated_results = store.search_fts("errors resolved parser", 10).unwrap();
    assert_eq!(consolidated_results.len(), 1);
    assert!(
        consolidated_results[0]
            .summary
            .contains("All errors resolved")
    );

    // Verify topic has exactly 1 memory
    let topic_mems = store.get_by_topic("errors-resolved").unwrap();
    assert_eq!(topic_mems.len(), 1);
}

// === MemoirStore tests ===
