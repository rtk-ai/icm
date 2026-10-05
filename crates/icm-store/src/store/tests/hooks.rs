//! Per-domain tests for the SQLite backend (`store::tests`).

use super::*;

// Hook telemetry

#[test]
fn test_record_hook_event_persists() {
    let store = test_store();
    let id = store.record_hook_event(&insert("post", 12, 0)).unwrap();
    assert!(id > 0);
    assert_eq!(store.hook_event_count().unwrap(), 1);
}

#[test]
fn test_hook_events_recent_orders_newest_first_and_filters() {
    let store = test_store();
    store.record_hook_event(&insert("post", 10, 0)).unwrap();
    store.record_hook_event(&insert("end", 9, 0)).unwrap();
    store.record_hook_event(&insert("post", 20, 1)).unwrap();

    let all = store.hook_events_recent(10, None).unwrap();
    assert_eq!(all.len(), 3);
    // Newest first: third insert wins position 0.
    assert_eq!(all[0].event, "post");
    assert_eq!(all[0].exit_code, 1);

    let posts = store.hook_events_recent(10, Some("post")).unwrap();
    assert_eq!(posts.len(), 2);
    assert!(posts.iter().all(|r| r.event == "post"));

    let ends = store.hook_events_recent(10, Some("end")).unwrap();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].duration_ms, Some(9));
}

#[test]
fn test_hook_stats_buckets_by_event_and_computes_percentiles() {
    let store = test_store();
    // post: durations [10, 20, 30] — p50=20, p99=30
    store.record_hook_event(&insert("post", 10, 0)).unwrap();
    store.record_hook_event(&insert("post", 20, 0)).unwrap();
    store.record_hook_event(&insert("post", 30, 1)).unwrap();
    // end: single 9ms success
    store.record_hook_event(&insert("end", 9, 0)).unwrap();

    // Use a wide window so all rows fall inside.
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let stats = store.hook_stats(&cutoff).unwrap();
    let by_event: std::collections::HashMap<_, _> =
        stats.into_iter().map(|r| (r.event.clone(), r)).collect();

    let post = &by_event["post"];
    assert_eq!(post.count, 3);
    assert_eq!(post.error_count, 1);
    assert_eq!(post.p50_duration_ms, 20);
    assert_eq!(post.p99_duration_ms, 30);

    let end = &by_event["end"];
    assert_eq!(end.count, 1);
    assert_eq!(end.error_count, 0);
    assert_eq!(end.p50_duration_ms, 9);
}

#[test]
fn test_prune_hook_events_drops_old_rows_only() {
    let store = test_store();
    store.record_hook_event(&insert("post", 10, 0)).unwrap();
    // Cutoff in the future → wipes everything.
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let n = store.prune_hook_events(&future).unwrap();
    assert_eq!(n, 1);
    assert_eq!(store.hook_event_count().unwrap(), 0);

    // Re-insert, then prune with a past cutoff → keeps row.
    store.record_hook_event(&insert("post", 10, 0)).unwrap();
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let n = store.prune_hook_events(&past).unwrap();
    assert_eq!(n, 0);
    assert_eq!(store.hook_event_count().unwrap(), 1);
}

// code_areas (issue #196)

#[test]
fn test_upsert_code_area_inserts_then_increments_touch_count() {
    let store = test_store();
    store
        .upsert_code_area("proj", "src/foo.rs", None, Some("s1"), Some("Edit"))
        .unwrap();
    let after_first = store.list_code_areas(None, None, None, 10).unwrap();
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].touch_count, 1);
    assert_eq!(after_first[0].project, "proj");
    assert_eq!(after_first[0].file_path, "src/foo.rs");

    // Same path again — touch_count++ and last_touched_at updates.
    let first_touched_at = after_first[0].first_touched_at;
    store
        .upsert_code_area("proj", "src/foo.rs", None, Some("s2"), Some("Write"))
        .unwrap();
    let after_second = store.list_code_areas(None, None, None, 10).unwrap();
    assert_eq!(after_second.len(), 1, "no duplicate row on re-touch");
    assert_eq!(after_second[0].touch_count, 2);
    // first_touched_at is preserved across re-touches.
    assert_eq!(after_second[0].first_touched_at, first_touched_at);
    // session_id / tool_name are refreshed to the latest.
    assert_eq!(after_second[0].session_id.as_deref(), Some("s2"));
    assert_eq!(after_second[0].tool_name.as_deref(), Some("Write"));
}

#[test]
fn test_upsert_code_area_preserves_existing_description_when_passed_none() {
    let store = test_store();
    store
        .upsert_code_area("proj", "f.rs", Some("initial note"), None, None)
        .unwrap();
    store
        .upsert_code_area("proj", "f.rs", None, None, None)
        .unwrap();
    let rows = store.list_code_areas(None, None, None, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].description.as_deref(), Some("initial note"));
}

#[test]
fn test_upsert_code_area_overwrites_description_when_passed_some() {
    let store = test_store();
    store
        .upsert_code_area("proj", "f.rs", Some("v1"), None, None)
        .unwrap();
    store
        .upsert_code_area("proj", "f.rs", Some("v2"), None, None)
        .unwrap();
    let rows = store.list_code_areas(None, None, None, 10).unwrap();
    assert_eq!(rows[0].description.as_deref(), Some("v2"));
}

#[test]
fn test_list_code_areas_filters_by_project_and_path_suffix() {
    let store = test_store();
    store
        .upsert_code_area("alpha", "src/a.rs", None, None, None)
        .unwrap();
    store
        .upsert_code_area("alpha", "src/b.rs", None, None, None)
        .unwrap();
    store
        .upsert_code_area("beta", "src/a.rs", None, None, None)
        .unwrap();

    let only_alpha = store
        .list_code_areas(Some("alpha"), None, None, 10)
        .unwrap();
    assert_eq!(only_alpha.len(), 2);

    // Suffix match catches both alpha/src/a.rs and beta/src/a.rs.
    let any_a = store
        .list_code_areas(None, Some("src/a.rs"), None, 10)
        .unwrap();
    assert_eq!(any_a.len(), 2);
    for r in &any_a {
        assert!(r.file_path.ends_with("src/a.rs"));
    }

    // Project + path combine.
    let alpha_a = store
        .list_code_areas(Some("alpha"), Some("src/a.rs"), None, 10)
        .unwrap();
    assert_eq!(alpha_a.len(), 1);
    assert_eq!(alpha_a[0].project, "alpha");
}

#[test]
fn test_list_code_areas_filters_by_since_timestamp() {
    let store = test_store();
    store
        .upsert_code_area("p", "old.rs", None, None, None)
        .unwrap();
    // Cutoff one hour ahead skips everything we just inserted.
    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let after = store.list_code_areas(None, None, Some(cutoff), 10).unwrap();
    assert!(after.is_empty());
    // Cutoff one hour behind keeps the row.
    let past = chrono::Utc::now() - chrono::Duration::hours(1);
    let after = store.list_code_areas(None, None, Some(past), 10).unwrap();
    assert_eq!(after.len(), 1);
}

#[test]
fn test_list_code_areas_orders_by_last_touched_desc() {
    let store = test_store();
    store
        .upsert_code_area("p", "first.rs", None, None, None)
        .unwrap();
    // Sleep so the second insert lands at a strictly later second.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    store
        .upsert_code_area("p", "second.rs", None, None, None)
        .unwrap();
    let rows = store.list_code_areas(None, None, None, 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].file_path, "second.rs");
    assert_eq!(rows[1].file_path, "first.rs");
}

#[test]
fn test_code_area_count_matches_unique_paths() {
    let store = test_store();
    assert_eq!(store.code_area_count().unwrap(), 0);
    store
        .upsert_code_area("p", "f.rs", None, None, None)
        .unwrap();
    store
        .upsert_code_area("p", "f.rs", None, None, None)
        .unwrap(); // re-touch
    store
        .upsert_code_area("p", "g.rs", None, None, None)
        .unwrap();
    assert_eq!(store.code_area_count().unwrap(), 2);
}

// Consolidation queue

/// One `pending` job per topic: every store past the threshold used to add
/// a job, and each one after the first was a provider call spent on
/// re-summarizing the summary the first had written.
#[test]
fn enqueue_pending_consolidation_keeps_one_pending_job_per_topic() {
    let store = test_store();
    let first = store.enqueue_pending_consolidation("busy", "").unwrap();
    for _ in 0..4 {
        assert_eq!(
            store.enqueue_pending_consolidation("busy", "").unwrap(),
            first
        );
    }
    let other = store.enqueue_pending_consolidation("quiet", "").unwrap();
    assert_ne!(other, first);
    assert_eq!(store.list_pending_consolidation_jobs(50).unwrap().len(), 2);

    // Once the job is no longer waiting, the topic can be queued again.
    store.mark_consolidation_job_done(&first).unwrap();
    let again = store.enqueue_pending_consolidation("busy", "").unwrap();
    assert_ne!(again, first);
    assert_eq!(store.list_pending_consolidation_jobs(50).unwrap().len(), 2);
}
