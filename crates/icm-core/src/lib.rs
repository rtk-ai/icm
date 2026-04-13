pub mod auto_link;
pub mod embedder;
pub mod error;
#[cfg(feature = "embeddings")]
pub mod fastembed_embedder;
pub mod feedback;
pub mod feedback_store;
pub mod learn;
pub mod transcript;
pub mod transcript_store;
pub mod memoir;
pub mod memoir_store;
pub mod memory;
pub mod store;
pub mod wake_up;

/// Default embedding vector dimensions (used when no embedder is configured).
pub const DEFAULT_EMBEDDING_DIMS: usize = 384;

pub use auto_link::{add_backrefs, auto_link_memory, AutoLinkOptions};
pub use embedder::Embedder;
pub use error::{IcmError, IcmResult};
#[cfg(feature = "embeddings")]
pub use fastembed_embedder::FastEmbedder;
pub use feedback::{Feedback, FeedbackStats};
pub use feedback_store::FeedbackStore;
pub use memoir::{Concept, ConceptLink, Label, Memoir, MemoirStats, Relation};
pub use memoir_store::MemoirStore;
pub use transcript::{Message, Role, Session, TranscriptHit, TranscriptStats};
pub use transcript_store::TranscriptStore;
pub use memory::{
    Importance, Memory, MemorySource, PatternCluster, Scope, StoreStats, TopicHealth,
};
pub use store::MemoryStore;
pub use wake_up::{
    build_wake_up, build_wake_up_from_memories, WakeUpFormat, WakeUpOptions, EMPTY_PACK_HEADER,
};

pub use learn::{learn_project, LearnResult};

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
