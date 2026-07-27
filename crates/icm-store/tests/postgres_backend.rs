//! Integration test for the opt-in PostgreSQL backend (issue #301).
//!
//! Only compiled under `--features postgres`. It needs a live PostgreSQL
//! with the `pgvector` extension available; point `ICM_POSTGRES_URL` at it
//! and run:
//!
//! ```sh
//! docker run -d --name icm-pg -e POSTGRES_PASSWORD=icm -e POSTGRES_USER=icm \
//!     -e POSTGRES_DB=icm -p 55432:5432 pgvector/pgvector:pg16
//! ICM_POSTGRES_URL=postgres://icm:icm@127.0.0.1:55432/icm \
//!     cargo test -p icm-store --no-default-features --features postgres
//! ```
//!
//! When `ICM_POSTGRES_URL` is unset the test prints a skip notice and
//! returns, so a backend-less CI run stays green.
#![cfg(feature = "postgres")]

use icm_core::{Importance, Memory, MemoryStore};
use icm_store::Store;

fn skip_if_no_pg() -> bool {
    if std::env::var("ICM_POSTGRES_URL").is_err() && std::env::var("DATABASE_URL").is_err() {
        eprintln!("skipping: ICM_POSTGRES_URL not set");
        return true;
    }
    // Manual-testing finding (same class as the OpenSearch test file):
    // `Store::with_dims` picks its backend from `ICM_DB_BACKEND`, which
    // defaults to sqlite when unset — a build with `--all-features` has
    // the sqlite backend compiled in too, so setting only
    // `ICM_POSTGRES_URL` and forgetting `ICM_DB_BACKEND=postgres` silently
    // runs every "postgres" test against an ephemeral SQLite store
    // instead, reporting a false-green "postgres works" without ever
    // touching PostgreSQL. Fail loudly instead.
    match std::env::var("ICM_DB_BACKEND").as_deref() {
        Ok("postgres") | Ok("postgresql") | Ok("pg") => {}
        other => panic!(
            "ICM_POSTGRES_URL is set but ICM_DB_BACKEND is {other:?}, not \"postgres\" — \
             these tests would silently run against sqlite instead. Set ICM_DB_BACKEND=postgres."
        ),
    }
    false
}

fn mem(topic: &str, summary: &str, imp: Importance) -> Memory {
    Memory::new(topic.to_string(), summary.to_string(), imp)
}

#[test]
fn postgres_core_memory_surface() {
    if skip_if_no_pg() {
        return;
    }

    // A unique topic namespace keeps this test isolated from any other
    // data already in the target database.
    let ns = format!("itest-{}", ulid::Ulid::new());
    let store =
        Store::with_dims(std::path::Path::new("ignored"), 384).expect("connect + migrate postgres");

    // --- store ---
    let mut m1 = mem(
        &ns,
        "PostgreSQL is a network-accessible backend",
        Importance::High,
    );
    m1.keywords = vec!["postgres".into(), "backend".into()];
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
            "PostgreSQL is a network-accessible backend",
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
        .search_by_keywords(&["postgres"], 10)
        .expect("kw search");
    assert!(kw.iter().any(|m| m.id == id1));
    let fts = store
        .search_fts("network-accessible", 10)
        .expect("fts search");
    assert!(fts.iter().any(|m| m.id == id1));

    // --- decay lowers non-critical weight, prune removes the weak ---
    let before = store.get(&id1).expect("get").expect("present").weight;
    let touched = store.apply_decay(0.5).expect("decay");
    assert!(touched >= 1);
    let after = store.get(&id1).expect("get").expect("present").weight;
    assert!(after < before, "weight should drop after decay");

    // --- delete ---
    store.delete(&id1).expect("delete");
    assert!(store.get(&id1).expect("get").is_none());
    assert_eq!(store.count_by_topic(&ns).expect("count"), 1);

    // cleanup the remaining row in this namespace
    for m in store.get_by_topic(&ns).expect("by topic") {
        let _ = store.delete(&m.id);
    }
}

#[test]
fn postgres_vector_knn_ranks_semantically() {
    if skip_if_no_pg() {
        return;
    }
    let ns = format!("itest-vec-{}", ulid::Ulid::new());
    // Use the store's configured dimension. An existing database is
    // authoritative on its dims, so build vectors that size regardless of
    // what we request here.
    let store = Store::with_dims(
        std::path::Path::new("ignored"),
        icm_core::DEFAULT_EMBEDDING_DIMS,
    )
    .expect("connect + migrate postgres");
    let dims = icm_core::DEFAULT_EMBEDDING_DIMS;

    // Hand-built embeddings so the test is deterministic and needs no
    // embedder: only the first two components vary. Query is closest to
    // `near`, far from `far`.
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
    assert!(
        results[0].1 > 0.5,
        "cosine similarity should be high for the near vector"
    );

    for m in store.get_by_topic(&ns).expect("by topic") {
        let _ = store.delete(&m.id);
    }
}

/// Manual-testing finding (same as the SQLite/OpenSearch backends):
/// deleting a memory left it as a dangling entry in every other memory's
/// `related_ids` forever. Verified real end to end against a live
/// PostgreSQL.
#[test]
fn postgres_delete_cleans_up_dangling_related_ids() {
    if skip_if_no_pg() {
        return;
    }
    let store = Store::with_dims(
        std::path::Path::new("ignored"),
        icm_core::DEFAULT_EMBEDDING_DIMS,
    )
    .expect("connect + migrate postgres");

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
