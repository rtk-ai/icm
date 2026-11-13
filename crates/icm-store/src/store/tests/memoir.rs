//! Per-domain tests for the SQLite backend (`store::tests`).

use super::*;

#[test]
fn test_memoir_crud() {
    let store = test_store();
    let m = make_memoir("my-project");
    let id = store.create_memoir(m).unwrap();

    let retrieved = store.get_memoir(&id).unwrap().unwrap();
    assert_eq!(retrieved.name, "my-project");

    let by_name = store.get_memoir_by_name("my-project").unwrap().unwrap();
    assert_eq!(by_name.id, id);

    store.delete_memoir(&id).unwrap();
    assert!(store.get_memoir(&id).unwrap().is_none());
}

#[test]
fn test_memoir_unique_name() {
    let store = test_store();
    store.create_memoir(make_memoir("dup")).unwrap();
    let result = store.create_memoir(make_memoir("dup"));
    assert!(result.is_err());
}

#[test]
fn test_list_memoirs() {
    let store = test_store();
    store.create_memoir(make_memoir("beta")).unwrap();
    store.create_memoir(make_memoir("alpha")).unwrap();

    let list = store.list_memoirs().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "alpha"); // sorted by name
    assert_eq!(list[1].name, "beta");
}

#[test]
fn test_concept_crud() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();

    let mut c = make_concept(&m_id, "event-sourcing", "Events stored in SQLite");
    c.labels = vec![Label::new("domain", "arch"), Label::new("type", "decision")];
    let c_id = store.add_concept(c).unwrap();

    let retrieved = store.get_concept(&c_id).unwrap().unwrap();
    assert_eq!(retrieved.name, "event-sourcing");
    assert_eq!(retrieved.labels.len(), 2);

    let by_name = store
        .get_concept_by_name(&m_id, "event-sourcing")
        .unwrap()
        .unwrap();
    assert_eq!(by_name.id, c_id);

    store.delete_concept(&c_id).unwrap();
    assert!(store.get_concept(&c_id).unwrap().is_none());
}

#[test]
fn test_concept_unique_within_memoir() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();

    store
        .add_concept(make_concept(&m_id, "dup", "first"))
        .unwrap();
    let result = store.add_concept(make_concept(&m_id, "dup", "second"));
    assert!(result.is_err());
}

#[test]
fn test_concept_same_name_different_memoirs() {
    let store = test_store();
    let m1 = store.create_memoir(make_memoir("proj1")).unwrap();
    let m2 = store.create_memoir(make_memoir("proj2")).unwrap();

    store
        .add_concept(make_concept(&m1, "sqlite", "def1"))
        .unwrap();
    store
        .add_concept(make_concept(&m2, "sqlite", "def2"))
        .unwrap();

    let c1 = store.get_concept_by_name(&m1, "sqlite").unwrap().unwrap();
    let c2 = store.get_concept_by_name(&m2, "sqlite").unwrap().unwrap();
    assert_ne!(c1.id, c2.id);
}

#[test]
fn test_refine_concept() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let c_id = store
        .add_concept(make_concept(&m_id, "es", "Events v1"))
        .unwrap();

    let orig = store.get_concept(&c_id).unwrap().unwrap();
    assert_eq!(orig.revision, 1);
    let orig_confidence = orig.confidence;

    store
        .refine_concept(&c_id, "Events v2 with snapshots", &["mem-1".into()])
        .unwrap();

    let refined = store.get_concept(&c_id).unwrap().unwrap();
    assert_eq!(refined.revision, 2);
    assert_eq!(refined.definition, "Events v2 with snapshots");
    assert!(refined.confidence > orig_confidence);
    assert!(refined.source_memory_ids.contains(&"mem-1".into()));
}

#[test]
fn test_concept_links() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let c1_id = store
        .add_concept(make_concept(&m_id, "event-sourcing", "ES pattern"))
        .unwrap();
    let c2_id = store
        .add_concept(make_concept(&m_id, "sqlite", "SQLite storage"))
        .unwrap();

    let link = ConceptLink::new(c1_id.clone(), c2_id.clone(), Relation::DependsOn);
    let link_id = store.add_link(link).unwrap();

    let from = store.get_links_from(&c1_id).unwrap();
    assert_eq!(from.len(), 1);
    assert_eq!(from[0].target_id, c2_id);
    assert_eq!(from[0].relation, Relation::DependsOn);

    let to = store.get_links_to(&c2_id).unwrap();
    assert_eq!(to.len(), 1);
    assert_eq!(to[0].source_id, c1_id);

    store.delete_link(&link_id).unwrap();
    assert!(store.get_links_from(&c1_id).unwrap().is_empty());
}

#[test]
fn test_self_link_rejected() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let c_id = store
        .add_concept(make_concept(&m_id, "concept", "def"))
        .unwrap();

    let link = ConceptLink::new(c_id.clone(), c_id, Relation::RelatedTo);
    let result = store.add_link(link);
    assert!(result.is_err());
}

#[test]
fn test_transitive_cycle_rejected() {
    // Audit M11/CYC1: A → B → C → A used to be silently accepted,
    // corrupting BFS in `get_neighborhood`. Now the third edge
    // (closing the cycle) is rejected with `InvalidInput`.
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let a = store.add_concept(make_concept(&m_id, "A", "a")).unwrap();
    let b = store.add_concept(make_concept(&m_id, "B", "b")).unwrap();
    let c = store.add_concept(make_concept(&m_id, "C", "c")).unwrap();

    // A → B: ok
    store
        .add_link(ConceptLink::new(a.clone(), b.clone(), Relation::DependsOn))
        .unwrap();
    // B → C: ok
    store
        .add_link(ConceptLink::new(b.clone(), c.clone(), Relation::Refines))
        .unwrap();
    // C → A: would close the cycle — reject
    let cycle_attempt = store.add_link(ConceptLink::new(c, a, Relation::RelatedTo));
    assert!(
        cycle_attempt.is_err(),
        "C → A should be rejected as a cycle"
    );
    let err_msg = cycle_attempt.unwrap_err().to_string();
    assert!(
        err_msg.contains("cycle"),
        "error message should mention cycle: {err_msg}"
    );
}

#[test]
fn test_dag_links_still_allowed() {
    // Sanity: rejecting cycles must not break legitimate DAG links.
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let a = store.add_concept(make_concept(&m_id, "A", "a")).unwrap();
    let b = store.add_concept(make_concept(&m_id, "B", "b")).unwrap();
    let c = store.add_concept(make_concept(&m_id, "C", "c")).unwrap();

    // A → B, A → C, B → C — three edges in a DAG, all should pass.
    store
        .add_link(ConceptLink::new(a.clone(), b.clone(), Relation::DependsOn))
        .unwrap();
    store
        .add_link(ConceptLink::new(a, c.clone(), Relation::DependsOn))
        .unwrap();
    store
        .add_link(ConceptLink::new(b, c, Relation::Refines))
        .unwrap();
}

#[test]
fn test_get_neighbors() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let c1 = store
        .add_concept(make_concept(&m_id, "a", "node a"))
        .unwrap();
    let c2 = store
        .add_concept(make_concept(&m_id, "b", "node b"))
        .unwrap();
    let c3 = store
        .add_concept(make_concept(&m_id, "c", "node c"))
        .unwrap();

    store
        .add_link(ConceptLink::new(
            c1.clone(),
            c2.clone(),
            Relation::DependsOn,
        ))
        .unwrap();
    store
        .add_link(ConceptLink::new(c3.clone(), c1.clone(), Relation::PartOf))
        .unwrap();

    let neighbors = store.get_neighbors(&c1, None).unwrap();
    assert_eq!(neighbors.len(), 2);

    let dep_neighbors = store.get_neighbors(&c1, Some(Relation::DependsOn)).unwrap();
    assert_eq!(dep_neighbors.len(), 1);
    assert_eq!(dep_neighbors[0].name, "b");
}

#[test]
fn test_get_neighborhood_bfs() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let c1 = store
        .add_concept(make_concept(&m_id, "a", "node a"))
        .unwrap();
    let c2 = store
        .add_concept(make_concept(&m_id, "b", "node b"))
        .unwrap();
    let c3 = store
        .add_concept(make_concept(&m_id, "c", "node c"))
        .unwrap();
    let c4 = store
        .add_concept(make_concept(&m_id, "d", "node d"))
        .unwrap();

    // a -> b -> c -> d
    store
        .add_link(ConceptLink::new(
            c1.clone(),
            c2.clone(),
            Relation::DependsOn,
        ))
        .unwrap();
    store
        .add_link(ConceptLink::new(
            c2.clone(),
            c3.clone(),
            Relation::DependsOn,
        ))
        .unwrap();
    store
        .add_link(ConceptLink::new(c3, c4, Relation::DependsOn))
        .unwrap();

    // depth=1 should get a + b
    let (concepts, links) = store.get_neighborhood(&c1, 1).unwrap();
    assert_eq!(concepts.len(), 2);
    assert!(!links.is_empty());

    // depth=2 should get a + b + c
    let (concepts, _) = store.get_neighborhood(&c1, 2).unwrap();
    assert_eq!(concepts.len(), 3);

    // depth=3 should get all 4
    let (concepts, _) = store.get_neighborhood(&c1, 3).unwrap();
    assert_eq!(concepts.len(), 4);
}

#[test]
fn test_cascade_delete_memoir() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();
    let c1 = store.add_concept(make_concept(&m_id, "a", "def")).unwrap();
    let c2 = store.add_concept(make_concept(&m_id, "b", "def")).unwrap();
    store
        .add_link(ConceptLink::new(c1, c2, Relation::RelatedTo))
        .unwrap();

    store.delete_memoir(&m_id).unwrap();

    // Concepts and links should be gone
    let concepts = store.list_concepts(&m_id).unwrap();
    assert!(concepts.is_empty());
}

#[test]
fn test_memoir_stats() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();

    let mut c = make_concept(&m_id, "es", "event sourcing");
    c.labels = vec![Label::new("domain", "arch")];
    let c1 = store.add_concept(c).unwrap();

    let mut c = make_concept(&m_id, "sqlite", "sqlite storage");
    c.labels = vec![Label::new("domain", "arch"), Label::new("type", "tech")];
    let c2 = store.add_concept(c).unwrap();

    store
        .add_link(ConceptLink::new(c1, c2, Relation::DependsOn))
        .unwrap();

    let stats = store.memoir_stats(&m_id).unwrap();
    assert_eq!(stats.total_concepts, 2);
    assert_eq!(stats.total_links, 1);
    assert!(stats.avg_confidence > 0.0);
    assert!(!stats.label_counts.is_empty());
}

#[test]
fn test_search_concepts_fts() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();

    store
        .add_concept(make_concept(
            &m_id,
            "event-sourcing",
            "Store domain events in append-only log",
        ))
        .unwrap();
    store
        .add_concept(make_concept(
            &m_id,
            "cqrs",
            "Command Query Responsibility Segregation",
        ))
        .unwrap();

    let results = store.search_concepts_fts(&m_id, "events", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "event-sourcing");
}

#[test]
fn test_search_concepts_by_label() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();

    let mut c1 = make_concept(&m_id, "es", "event sourcing");
    c1.labels = vec![Label::new("domain", "arch")];
    store.add_concept(c1).unwrap();

    let mut c2 = make_concept(&m_id, "sqlite", "storage");
    c2.labels = vec![Label::new("domain", "tech")];
    store.add_concept(c2).unwrap();

    let results = store
        .search_concepts_by_label(&m_id, &Label::new("domain", "arch"), 10)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "es");
}

/// Audit regression: the label pattern built by `search_concepts_by_label`
/// interpolated `namespace`/`value` into a LIKE pattern unescaped, so a
/// literal `_` in a search value acted as a SQL "any single char"
/// wildcard instead of matching only that exact character.
#[test]
fn test_search_concepts_by_label_escapes_wildcards() {
    let store = test_store();
    let m_id = store.create_memoir(make_memoir("proj")).unwrap();

    let mut c1 = make_concept(&m_id, "c1", "def1");
    c1.labels = vec![Label::new("domain", "test")];
    store.add_concept(c1).unwrap();

    let mut c2 = make_concept(&m_id, "c2", "def2");
    c2.labels = vec![Label::new("domain", "text")];
    store.add_concept(c2).unwrap();

    // "te_t" is not the literal value of either concept, but with an
    // unescaped `_` it matches both "test" and "text" as a wildcard.
    let results = store
        .search_concepts_by_label(&m_id, &Label::new("domain", "te_t"), 10)
        .unwrap();
    assert!(
        results.is_empty(),
        "unescaped '_' wildcard matched unrelated label values: {results:?}"
    );
}

// === Vector search tests ===

#[test]
fn invalidated_link_is_kept_as_history_but_ignored() {
    let store = test_store();
    let mid = store.create_memoir(make_memoir("hist")).unwrap();
    let a = store.add_concept(make_concept(&mid, "api", "d")).unwrap();
    let b = store.add_concept(make_concept(&mid, "redis", "d")).unwrap();
    let link_id = store
        .add_link(ConceptLink::new(a.clone(), b.clone(), Relation::DependsOn).with_source("pr:42"))
        .unwrap();

    let active = store.get_links_from(&a).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].source.as_deref(), Some("pr:42"));

    store.invalidate_link(&link_id).unwrap();
    assert!(store.get_links_from(&a).unwrap().is_empty());
    assert!(store.get_neighbors(&a, None).unwrap().is_empty());
    assert_eq!(store.memoir_stats(&mid).unwrap().total_links, 0);
    assert!(store.get_links_for_memoir(&mid).unwrap().is_empty());
    // Invalidating twice reports NotFound.
    assert!(matches!(
        store.invalidate_link(&link_id),
        Err(IcmError::NotFound(_))
    ));

    let hist = store.get_link_history(&a).unwrap();
    assert_eq!(hist.len(), 1);
    assert!(hist[0].invalidated_at.is_some());

    // An invalidated edge does not block the reverse edge (no stale cycle).
    store
        .add_link(ConceptLink::new(b.clone(), a.clone(), Relation::DependsOn))
        .unwrap();

    // Re-asserting the edge revives the same row instead of failing on UNIQUE.
    store
        .invalidate_link(&store.get_links_from(&b).unwrap()[0].id)
        .unwrap();
    let revived = store
        .add_link(ConceptLink::new(a.clone(), b.clone(), Relation::DependsOn).with_source("pr:57"))
        .unwrap();
    assert_eq!(revived, link_id);
    let now = store.get_links_from(&a).unwrap();
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].source.as_deref(), Some("pr:57"));
    assert!(now[0].is_active());
}
