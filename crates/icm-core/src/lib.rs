pub mod embedder;
pub mod error;
#[cfg(feature = "embeddings")]
pub mod fastembed_embedder;
pub mod feedback;
pub mod feedback_store;
pub mod memoir;
pub mod memoir_store;
pub mod memory;
pub mod store;

/// Default embedding vector dimensions (used when no embedder is configured).
pub const DEFAULT_EMBEDDING_DIMS: usize = 384;

pub use embedder::Embedder;
pub use error::{IcmError, IcmResult};
#[cfg(feature = "embeddings")]
pub use fastembed_embedder::FastEmbedder;
pub use feedback::{Feedback, FeedbackStats};
pub use feedback_store::FeedbackStore;
pub use memoir::{Concept, ConceptLink, Label, Memoir, MemoirStats, Relation};
pub use memoir_store::MemoirStore;
pub use memory::{
    Importance, Memory, MemorySource, PatternCluster, Scope, StoreStats, TopicHealth,
};
pub use store::MemoryStore;

/// Common message for empty search results.
pub const MSG_NO_MEMORIES: &str = "No memories found.";

/// Check if a memory's topic matches a filter (supports prefix with ':').
pub fn topic_matches(memory_topic: &str, filter: &str) -> bool {
    memory_topic == filter || memory_topic.starts_with(&format!("{filter}:"))
}

/// Check if any keyword contains the filter string.
pub fn keyword_matches(keywords: &[String], filter: &str) -> bool {
    keywords.iter().any(|k| k.contains(filter))
}
