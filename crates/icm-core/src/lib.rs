// The `embeddings` feature carries only the embedder code + model download; an
// onnxruntime backend must be selected explicitly (issue #345). Enabling bare
// `embeddings` would build `fastembed` with no ort backend — a confusing link
// error or a runtime with no way to load onnxruntime — so fail loudly here.
#[cfg(all(
    feature = "embeddings",
    not(any(feature = "embeddings-static", feature = "embeddings-dynamic"))
))]
compile_error!(
    "the `embeddings` feature needs an onnxruntime backend: enable \
     `embeddings-static` (build-time download) or `embeddings-dynamic` \
     (runtime load-dynamic), not bare `embeddings`. See issue #345."
);

pub mod auto_link;
pub mod context_snapshot;
pub mod embedder;
pub mod error;
pub mod facts;
pub mod facts_store;
#[cfg(feature = "embeddings")]
pub mod fastembed_embedder;
pub mod feedback;
pub mod feedback_store;
pub mod learn;
pub mod memoir;
pub mod memoir_store;
pub mod memory;
pub mod project;
pub mod store;
pub mod transcript;
pub mod transcript_store;
pub mod wake_up;

/// Default embedding vector dimensions (used when no embedder is configured).
pub const DEFAULT_EMBEDDING_DIMS: usize = 384;

pub use auto_link::{AutoLinkOptions, add_backrefs, auto_link_memory};
pub use context_snapshot::{
    ContextSnapshot, ContextSnapshotOptions, SNAPSHOT_HEADER, SnapshotFormat, SnapshotSection,
    build_context_snapshot, build_context_snapshot_from_memories,
};
pub use embedder::Embedder;
pub use error::{IcmError, IcmResult};
pub use facts::{Fact, FactsStats};
pub use facts_store::FactsStore;
#[cfg(feature = "embeddings")]
pub use fastembed_embedder::{DEFAULT_MODEL as DEFAULT_EMBEDDING_MODEL, FastEmbedder};
pub use feedback::{Feedback, FeedbackStats};
pub use feedback_store::FeedbackStore;
pub use memoir::{Concept, ConceptLink, Label, Memoir, MemoirStats, Relation};
pub use memoir_store::MemoirStore;
pub use memory::{
    Importance, Memory, MemorySource, PatternCluster, Scope, StoreStats, TopicHealth,
    max_importance,
};
pub use store::{
    DEDUP_SIMILARITY_THRESHOLD, MemoryStore, find_similar_memory, merge_summaries, union_keywords,
};
pub use transcript::{Message, Role, Session, TranscriptHit, TranscriptStats};
pub use transcript_store::TranscriptStore;
pub use wake_up::{
    EMPTY_PACK_HEADER, WakeUpFormat, WakeUpOptions, build_wake_up, build_wake_up_from_memories,
    is_preference_topic, project_matches,
};

pub use learn::{LearnResult, learn_project};

pub mod time_fmt;
pub use time_fmt::format_local;

/// Common message for empty search results.
pub const MSG_NO_MEMORIES: &str = "No memories found.";

/// Check if a memory's topic matches a filter.
/// Matching is case-insensitive and bidirectional: the filter can be a
/// substring of the topic or vice-versa. This allows `"pi-api"` to match
/// `"context-pi-api"` and `"context-pi-api"` to match `"pi-api"`.
pub fn topic_matches(memory_topic: &str, filter: &str) -> bool {
    let topic = memory_topic.to_lowercase();
    let f = filter.to_lowercase();
    topic == f || topic.contains(&f) || f.contains(&topic)
}

/// Check if any keyword contains the filter string.
pub fn keyword_matches(keywords: &[String], filter: &str) -> bool {
    keywords.iter().any(|k| k.contains(filter))
}
