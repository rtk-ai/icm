//! OpenSearch storage backend (issue #301, opt-in via `--features opensearch`).
//!
//! A search-native shared store: BM25 full-text and `knn_vector` HNSW
//! vector search live in one engine, so horizontally-scaled ICM replicas
//! share one memory store (a node-local SQLite file cannot be shared).
//!
//! Design notes:
//!
//! - **Blocking REST.** OpenSearch is an HTTP/JSON service, so this talks
//!   to it with the blocking `ureq` client and `serde_json` bodies. The
//!   store traits are synchronous, so — like the PostgreSQL backend —
//!   there is no async runtime and no sync-over-async bridge.
//! - **Vector search** uses a `knn_vector` field (HNSW, cosine space);
//!   similarity is reported from the kNN `_score`.
//! - **Full-text search** uses BM25 `match` queries; the hybrid path
//!   blends normalized BM25 and vector scores 30/70 to match the SQLite
//!   and PostgreSQL backends.
//! - **Connection** from `ICM_OPENSEARCH_URL` (e.g. `http://localhost:9200`),
//!   with optional basic auth from `ICM_OPENSEARCH_USER` /
//!   `ICM_OPENSEARCH_PASSWORD`.
//!
//! Scope mirrors the PostgreSQL backend: the full [`MemoryStore`] surface
//! plus the ancillary store/recall/hook tables (hook telemetry, the
//! extraction queue, code areas, key/value metadata). The heavier
//! subsystems (memoir graph, transcripts, structured facts, feedback,
//! pattern mining) return [`IcmError::Unsupported`]; they stay fully
//! available on the default SQLite backend.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};

use icm_core::{
    Concept, ConceptLink, Embedder, Fact, FactsStats, FactsStore, Feedback, FeedbackStats,
    FeedbackStore, IcmError, IcmResult, Importance, Label, Memoir, MemoirStats, MemoirStore,
    Memory, MemorySource, MemoryStore, Message, PatternCluster, Relation, Role, Scope, Session,
    StoreStats, TopicHealth, TranscriptHit, TranscriptStats, TranscriptStore,
};

// Shared public row types live in `crate::common` (issue #301) so every
// backend can be compiled into one binary without colliding definitions.
pub use crate::common::{
    CodeArea, ConsolidationJob, HookEvent, HookEventInsert, HookStatsRow, PendingRow,
};

// Index names

const IDX_MEMORIES: &str = "icm_memories";
const IDX_METADATA: &str = "icm_metadata";
const IDX_HOOKS: &str = "icm_hook_events";
const IDX_PENDING: &str = "icm_pending_extractions";
const IDX_CONSOLIDATION_JOBS: &str = "icm_pending_consolidations";
const IDX_CODE_AREAS: &str = "icm_code_areas";

// Store

/// OpenSearch-backed store. Cheap to clone-free share via `&self`; every
/// method is a blocking REST round-trip.
pub struct OpenSearchStore {
    agent: ureq::Agent,
    base: String,
    auth: Option<String>,
    embedding_dims: usize,
    readonly: bool,
}

// Subsystems not yet ported to this backend. They stay fully available on
// the default SQLite backend; here they fail cleanly with `Unsupported`.

fn unsupported<T>(op: &str) -> IcmResult<T> {
    Err(IcmError::Unsupported(format!(
        "{op} (use the default SQLite backend)"
    )))
}

mod connection;
mod facts;
mod feedback;
mod hooks;
mod maintenance;
mod memoir;
mod memory;
mod patterns;
mod rows;
#[cfg(test)]
mod tests;
mod transcript;

pub(crate) use rows::*;
