//! Tests for topic merge / rename (`store::tests`).

use super::*;

fn stored_hash(store: &SqliteStore, id: &str) -> String {
    store
        .conn
        .query_row(
            "SELECT summary_hash FROM memories WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
}

fn vec_count(store: &SqliteStore, id: &str) -> i64 {
    store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM vec_memories WHERE memory_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn merge_moves_memories_and_recomputes_hash() {
    let store = test_store();
    let moved = store.store(make_memory("old", "shared fact")).unwrap();
    let also_moved = store.store(make_memory("old", "only in old")).unwrap();
    let target = store.store(make_memory("new", "already here")).unwrap();

    let stats = store.merge_topics("old", "new", false, false).unwrap();
    assert_eq!(stats.scanned, 2);
    assert_eq!(stats.moved, 2);
    assert_eq!(stats.duplicates_skipped, 0);
    assert_eq!(stats.target_before, 1);
    assert_eq!(stats.target_after, 3);

    assert_eq!(store.count_by_topic("old").unwrap(), 0);
    assert_eq!(store.count_by_topic("new").unwrap(), 3);

    // summary_hash encodes the topic; reusing the stored value would leave
    // the dedup index stale and silently break future INSERT OR IGNORE.
    assert_eq!(
        stored_hash(&store, &moved),
        crate::store::rows::summary_hash("new", "shared fact")
    );
    assert_eq!(
        stored_hash(&store, &also_moved),
        crate::store::rows::summary_hash("new", "only in old")
    );
    assert_eq!(
        stored_hash(&store, &target),
        crate::store::rows::summary_hash("new", "already here")
    );

    // The memories_au trigger must follow the move into FTS.
    let fts_hits: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'shared'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_hits, 1);
}

#[test]
fn merge_dry_run_changes_nothing() {
    let store = test_store();
    store.store(make_memory("old", "fact one")).unwrap();
    store.store(make_memory("new", "fact two")).unwrap();

    let stats = store.merge_topics("old", "new", false, true).unwrap();
    assert_eq!(stats.moved, 1);
    assert_eq!(stats.target_after, 2);

    assert_eq!(store.count_by_topic("old").unwrap(), 1);
    assert_eq!(store.count_by_topic("new").unwrap(), 1);
}

#[test]
fn merge_collision_aborts_without_write() {
    let store = test_store();
    store
        .store(make_memory("old", "duplicate summary"))
        .unwrap();
    store
        .store(make_memory("new", "duplicate summary"))
        .unwrap();

    let err = store.merge_topics("old", "new", false, false).unwrap_err();
    assert!(matches!(err, IcmError::InvalidInput(_)));

    // The abort must roll the whole transaction back.
    assert_eq!(store.count_by_topic("old").unwrap(), 1);
    assert_eq!(store.count_by_topic("new").unwrap(), 1);
}

#[test]
fn merge_dedupe_keeps_target_and_cleans_vectors() {
    let store = test_store();
    let mut dup = make_memory("old", "duplicate summary");
    dup.embedding = Some(vec![0.1; icm_core::DEFAULT_EMBEDDING_DIMS]);
    let dup_id = store.store(dup).unwrap();
    let mut target_mem = make_memory("new", "duplicate summary");
    target_mem.embedding = Some(vec![0.2; icm_core::DEFAULT_EMBEDDING_DIMS]);
    let target = store.store(target_mem).unwrap();

    let stats = store.merge_topics("old", "new", true, false).unwrap();
    assert_eq!(stats.scanned, 1);
    assert_eq!(stats.moved, 0);
    assert_eq!(stats.duplicates_skipped, 1);
    assert_eq!(stats.target_after, 1);

    assert_eq!(store.count_by_topic("old").unwrap(), 0);
    assert_eq!(store.count_by_topic("new").unwrap(), 1);
    assert_eq!(
        vec_count(&store, &dup_id),
        0,
        "dropped duplicate must not leave an orphaned vector"
    );
    assert_eq!(vec_count(&store, &target), 1);
}

#[test]
fn merge_dedupe_catches_legacy_target_without_hash() {
    let store = test_store();
    let legacy = store.store(make_memory("new", "legacy summary")).unwrap();
    store
        .conn
        .execute(
            "UPDATE memories SET summary_hash = NULL WHERE id = ?1",
            params![legacy],
        )
        .unwrap();
    store.store(make_memory("old", "legacy summary")).unwrap();

    let stats = store.merge_topics("old", "new", true, false).unwrap();
    assert_eq!(stats.duplicates_skipped, 1);
    assert_eq!(stats.moved, 0);
    assert_eq!(store.count_by_topic("new").unwrap(), 1);
}

#[test]
fn merge_moves_feedback_rows() {
    let store = test_store();
    store.store(make_memory("old", "memory in old")).unwrap();
    store
        .store_feedback(make_feedback("old", "ctx", "a", "b"))
        .unwrap();

    store.merge_topics("old", "new", false, false).unwrap();

    let moved = store.list_feedback(Some("new"), 10).unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].topic, "new");
    assert!(store.list_feedback(Some("old"), 10).unwrap().is_empty());
}

#[test]
fn rename_topic_moves_memories() {
    let store = test_store();
    let id = store.store(make_memory("old", "rename me")).unwrap();

    let stats = store.rename_topic("old", "new", false).unwrap();
    assert_eq!(stats.moved, 1);
    assert_eq!(store.count_by_topic("old").unwrap(), 0);
    assert_eq!(store.count_by_topic("new").unwrap(), 1);
    assert_eq!(
        stored_hash(&store, &id),
        crate::store::rows::summary_hash("new", "rename me")
    );
}

#[test]
fn rename_into_existing_topic_is_rejected() {
    let store = test_store();
    store.store(make_memory("old", "a")).unwrap();
    store.store(make_memory("new", "b")).unwrap();

    let err = store.rename_topic("old", "new", false).unwrap_err();
    assert!(matches!(err, IcmError::InvalidInput(_)));
    assert_eq!(store.count_by_topic("old").unwrap(), 1);
    assert_eq!(store.count_by_topic("new").unwrap(), 1);
}

#[test]
fn merge_missing_source_is_not_found() {
    let store = test_store();
    store.store(make_memory("new", "b")).unwrap();

    let err = store
        .merge_topics("missing", "new", false, false)
        .unwrap_err();
    assert!(matches!(err, IcmError::NotFound(_)));
}

#[test]
fn merge_same_topic_is_rejected() {
    let store = test_store();
    store.store(make_memory("same", "a")).unwrap();

    let err = store
        .merge_topics("same", "same", false, false)
        .unwrap_err();
    assert!(matches!(err, IcmError::InvalidInput(_)));
}

#[test]
fn merge_empty_topic_is_rejected() {
    let store = test_store();
    store.store(make_memory("old", "a")).unwrap();

    let err = store.merge_topics("", "new", false, false).unwrap_err();
    assert!(matches!(err, IcmError::InvalidInput(_)));
}
