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
//! ICM_OPENSEARCH_URL=http://localhost:9201 \
//!     cargo test -p icm-store --no-default-features --features opensearch
//! ```
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
