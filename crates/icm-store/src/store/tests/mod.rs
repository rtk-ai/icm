//! Test suite for the SQLite backend (`store::tests`).

use super::*;
use icm_core::Importance;

fn test_store() -> SqliteStore {
    SqliteStore::in_memory().unwrap()
}

fn make_memory(topic: &str, summary: &str) -> Memory {
    Memory::new(topic.into(), summary.into(), Importance::Medium)
}

fn make_memoir(name: &str) -> Memoir {
    Memoir::new(name.into(), format!("Description for {name}"))
}

fn make_concept(memoir_id: &str, name: &str, definition: &str) -> Concept {
    Concept::new(memoir_id.into(), name.into(), definition.into())
}

// === Integrity check / repair (issue #313) ===

fn seed_writable_db(path: &Path) -> Memory {
    let store = SqliteStore::new(path).unwrap();
    let mut m = make_memory("project:icm", "read-only fixture summary");
    m.embedding = Some(vec![0.1_f32; icm_core::DEFAULT_EMBEDDING_DIMS]);
    store.store(m.clone()).unwrap();
    m
}

fn make_feedback(topic: &str, context: &str, predicted: &str, corrected: &str) -> Feedback {
    Feedback::new(
        topic.into(),
        context.into(),
        predicted.into(),
        corrected.into(),
        None,
        "test".into(),
    )
}

fn insert(event: &str, duration_ms: i64, exit_code: i32) -> HookEventInsert {
    HookEventInsert {
        event: event.into(),
        project: None,
        session_id: None,
        tool_name: None,
        duration_ms: Some(duration_ms),
        exit_code,
        payload_size: None,
        note: None,
    }
}

use std::path::Path;

mod consolidate;
mod embeddings;
mod facts;
mod feedback;
mod graph;
mod hooks;
mod maintenance;
mod memoir;
mod memory;
mod perf;
mod search;
mod topic;
mod transcript;
mod validation;
