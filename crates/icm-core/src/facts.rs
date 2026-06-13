//! Structured facts layer (issue #273).
//!
//! Flat `(entity, key, value)` tuples for **exact** lookup, distinct
//! from the curated `Memory` (semantic recall) and the verbatim
//! `Transcript` (archived conversation) layers.
//!
//! Why a third layer:
//! - Semantic recall is probabilistic — vector + BM25 ranking can
//!   miss or down-rank the right answer when the query is sharp
//!   ("which GCP secret for X", "what host is Z on").
//! - `facts` answers those questions with a primary-key lookup,
//!   under 1ms even at 10k+ rows.
//!
//! Supersession: each fact carries `superseded_at`. An update
//! marks the previous row superseded and inserts a new active row
//! under the same `(entity, key)`. This gives ICM a first-class
//! place for "fact changed" (vs. dedup, which is "same fact stated
//! twice") — the issue calls this out as a bonus motivator.
//!
//! Scope (per issue): this PR ships schema + CRUD + CLI. Auto-
//! population from extraction and entity-resolution heuristics are
//! deferred to follow-up RFCs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single structured fact.
///
/// The composite identity is `(entity, key)` — `id` is just a row
/// handle for the CLI / API to reference. Active facts have
/// `superseded_at = None`; supersession-history rows carry the
/// timestamp of the override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub entity: String,
    pub key: String,
    pub value: String,
    /// Free-form provenance label (e.g. "user", "extraction:hook",
    /// "import:foo.yaml"). Surfaces in `icm facts get` so the user
    /// can see where a value came from.
    pub source: String,
    pub created_at: DateTime<Utc>,
    /// `None` while active; set to the override timestamp when a
    /// newer fact takes over the same `(entity, key)`.
    pub superseded_at: Option<DateTime<Utc>>,
}

impl Fact {
    /// Construct an active fact (`superseded_at = None`).
    #[must_use]
    pub fn new(entity: String, key: String, value: String, source: String) -> Self {
        Self {
            id: ulid::Ulid::new().to_string(),
            entity,
            key,
            value,
            source,
            created_at: Utc::now(),
            superseded_at: None,
        }
    }

    /// True iff the fact is currently the active value for its
    /// `(entity, key)` slot.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.superseded_at.is_none()
    }
}

/// Aggregate stats for the facts store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactsStats {
    /// Distinct `(entity, key)` pairs with an active row.
    pub active_count: usize,
    /// Total rows including superseded history.
    pub total_count: usize,
    /// Distinct entities with at least one active fact.
    pub distinct_entities: usize,
    /// Top entities by active fact count, descending.
    pub top_entities: Vec<(String, usize)>,
}
