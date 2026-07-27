//! Integration test for the opt-in OpenSearch backend (issue #301).
//!
//! Only compiled under `--features opensearch`. It needs a live OpenSearch;
//! point `ICM_OPENSEARCH_URL` at it and run:
//!
//! ```sh
//! docker run -d --name icm-os -p 9201:9200 \
//!     -e discovery.type=single-node -e DISABLE_SECURITY_PLUGIN=true \
//!     -e DISABLE_INSTALL_DEMO_CONFIG=true \
//!     opensearchproject/opensearch:2
//! ICM_OPENSEARCH_URL=http://localhost:9201 ICM_DB_BACKEND=opensearch \
//!     cargo test -p icm-store --no-default-features --features opensearch
//! ```
//!
//! `ICM_DB_BACKEND=opensearch` is required too — `Store::with_dims` resolves
//! the backend from it and defaults to sqlite otherwise, which fails to
//! connect with a confusing "sqlite backend was requested but this build
//! was not compiled with its Cargo feature" error (manual-testing finding).
//!
//! When `ICM_OPENSEARCH_URL` is unset the test prints a skip notice and
//! returns, so a backend-less CI run stays green.
#![cfg(feature = "opensearch")]

use icm_core::{Importance, Memory, MemoryStore};
use icm_store::Store;

fn skip_if_no_os() -> bool {
    if std::env::var("ICM_OPENSEARCH_URL").is_err() {
        eprintln!("skipping: ICM_OPENSEARCH_URL not set");
        return true;
    }
    // Manual-testing finding: `Store::with_dims` picks its backend from
    // `ICM_DB_BACKEND`, which defaults to sqlite when unset — a build with
    // `--all-features` has the sqlite backend compiled in too, so setting
    // only `ICM_OPENSEARCH_URL` and forgetting `ICM_DB_BACKEND=opensearch`
    // silently runs every "opensearch" test against an ephemeral SQLite
    // store instead. All the CRUD/KNN assertions still pass (SQLite
    // supports them natively), so this reports a false-green "opensearch
    // works" without ever touching OpenSearch. Fail loudly instead.
    match std::env::var("ICM_DB_BACKEND").as_deref() {
        Ok("opensearch") | Ok("os") => {}
        other => panic!(
            "ICM_OPENSEARCH_URL is set but ICM_DB_BACKEND is {other:?}, not \"opensearch\" — \
             these tests would silently run against sqlite instead. Set ICM_DB_BACKEND=opensearch."
        ),
    }
    false
}

fn mem(topic: &str, summary: &str, imp: Importance) -> Memory {
    Memory::new(topic.to_string(), summary.to_string(), imp)
}

#[test]
fn opensearch_core_memory_surface() {
    if skip_if_no_os() {
        return;
    }

    let ns = format!("itest-{}", ulid::Ulid::new());
    let store = Store::with_dims(std::path::Path::new("ignored"), 384)
        .expect("connect + migrate opensearch");

    // --- store ---
    let mut m1 = mem(
        &ns,
        "OpenSearch is a network-accessible backend",
        Importance::High,
    );
    m1.keywords = vec!["opensearch".into(), "backend".into()];
    let id1 = store.store(m1.clone()).expect("store m1");
    let _ = store
        .store(mem(
            &ns,
            "SQLite remains the default backend",
            Importance::Medium,
        ))
        .expect("store m2");

    // --- dedup: same (topic, summary) returns the same id ---
    let id1_again = store
        .store(mem(
            &ns,
            "OpenSearch is a network-accessible backend",
            Importance::Low,
        ))
        .expect("store dup");
    assert_eq!(id1, id1_again, "dedup must return the existing id");

    // importance must NOT be downgraded by the low-priority re-store
    let fetched = store.get(&id1).expect("get").expect("present");
    assert_eq!(fetched.importance, Importance::High);

    // --- count / topic listing ---
    assert_eq!(store.count_by_topic(&ns).expect("count"), 2);
    let topics = store.list_topics().expect("topics");
    assert!(topics.iter().any(|(t, n)| t == &ns && *n == 2));

    // --- keyword + FTS search ---
    let kw = store
        .search_by_keywords(&["opensearch"], 10)
        .expect("kw search");
    assert!(kw.iter().any(|m| m.id == id1));
    let fts = store
        .search_fts("network-accessible", 10)
        .expect("fts search");
    assert!(fts.iter().any(|m| m.id == id1));

    // --- decay lowers non-critical weight ---
    let before = store.get(&id1).expect("get").expect("present").weight;
    let touched = store.apply_decay(0.5).expect("decay");
    assert!(touched >= 1);
    let after = store.get(&id1).expect("get").expect("present").weight;
    assert!(after < before, "weight should drop after decay");

    // --- delete ---
    store.delete(&id1).expect("delete");
    assert!(store.get(&id1).expect("get").is_none());
    assert_eq!(store.count_by_topic(&ns).expect("count"), 1);

    // cleanup
    for m in store.get_by_topic(&ns).expect("by topic") {
        let _ = store.delete(&m.id);
    }
}

#[test]
fn opensearch_vector_knn_ranks_semantically() {
    if skip_if_no_os() {
        return;
    }
    let ns = format!("itest-vec-{}", ulid::Ulid::new());
    let store = Store::with_dims(
        std::path::Path::new("ignored"),
        icm_core::DEFAULT_EMBEDDING_DIMS,
    )
    .expect("connect + migrate opensearch");
    let dims = icm_core::DEFAULT_EMBEDDING_DIMS;

    // Hand-built embeddings so the test is deterministic: only the first
    // two components vary. Query is closest to `near`, far from `far`.
    let onehot = |a: f32, b: f32| {
        let mut v = vec![0.0_f32; dims];
        v[0] = a;
        v[1] = b;
        v
    };
    let mk = |summary: &str, emb: Vec<f32>| {
        let mut m = mem(&ns, summary, Importance::Medium);
        m.embedding = Some(emb);
        m
    };
    let near_id = store
        .store(mk("near vector", onehot(1.0, 0.0)))
        .expect("store near");
    let _far_id = store
        .store(mk("far vector", onehot(0.0, 1.0)))
        .expect("store far");

    let query = onehot(0.9, 0.1);
    let results = store.search_by_embedding(&query, 5).expect("knn");
    assert!(!results.is_empty());
    assert_eq!(results[0].0.id, near_id, "nearest vector must rank first");

    for m in store.get_by_topic(&ns).expect("by topic") {
        let _ = store.delete(&m.id);
    }
}

/// Manual-testing finding: deleting a memory left it as a dangling entry
/// in every other memory's `related_ids` forever. Verified real end to end
/// against a live OpenSearch (2026-07-27): before the fix, deleting `a`
/// left `["a", "c"]` in `b`'s document; after, `b` correctly has just
/// `["c"]`.
#[test]
fn opensearch_delete_cleans_up_dangling_related_ids() {
    if skip_if_no_os() {
        return;
    }
    let store = Store::with_dims(
        std::path::Path::new("ignored"),
        icm_core::DEFAULT_EMBEDDING_DIMS,
    )
    .expect("connect + migrate opensearch");

    let ns = format!("itest-related-{}", ulid::Ulid::new());
    let mut a = mem(&ns, "memory a", Importance::Medium);
    let mut b = mem(&ns, "memory b", Importance::Medium);
    let mut c = mem(&ns, "memory c", Importance::Medium);

    let a_id = store.store(a.clone()).expect("store a");
    let b_id = store.store(b.clone()).expect("store b");
    let c_id = store.store(c.clone()).expect("store c");

    a.id = a_id.clone();
    a.related_ids = vec![b_id.clone(), c_id.clone()];
    store.update(&a).expect("link a");
    b.id = b_id.clone();
    b.related_ids = vec![a_id.clone(), c_id.clone()];
    store.update(&b).expect("link b");
    c.id = c_id.clone();
    c.related_ids = vec![a_id.clone(), b_id.clone()];
    store.update(&c).expect("link c");

    store.delete(&a_id).expect("delete a");

    let b_after = store.get(&b_id).expect("get b").expect("b exists");
    assert!(
        !b_after.related_ids.contains(&a_id),
        "b's related_ids must no longer reference the deleted a: {:?}",
        b_after.related_ids
    );
    assert_eq!(b_after.related_ids, vec![c_id.clone()]);

    let c_after = store.get(&c_id).expect("get c").expect("c exists");
    assert!(
        !c_after.related_ids.contains(&a_id),
        "c's related_ids must no longer reference the deleted a: {:?}",
        c_after.related_ids
    );
    assert_eq!(c_after.related_ids, vec![b_id.clone()]);

    let _ = store.delete(&b_id);
    let _ = store.delete(&c_id);
}
