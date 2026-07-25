use crate::error::IcmResult;
use crate::memory::{Memory, StoreStats, TopicHealth};

/// Similarity score above which a new memory is considered a duplicate of an existing one.
pub const DEDUP_SIMILARITY_THRESHOLD: f32 = 0.85;

/// Find an existing memory that is similar enough to be considered a duplicate.
///
/// Returns the closest match and its similarity score if the score exceeds `threshold`
/// and the match belongs to the same topic. Returns `None` otherwise.
///
/// Uses `search_by_embedding` (pure cosine similarity) rather than
/// `search_hybrid`. Dedup is fundamentally a "is this the same content"
/// question, which cosine similarity answers directly. The hybrid blend
/// (`0.3*fts + 0.7*cosine`) is tuned for human-facing recall RANKING, not
/// for a duplicate/not-duplicate decision — diluting an exact semantic
/// match (cosine=1.0) with an FTS component made the 0.85 threshold nearly
/// unreachable even for byte-identical content once the FTS side scored
/// low (e.g. AND-joined tokens not matching), and a purely vector-side
/// match (no shared keywords, cosine=1.0) could never exceed
/// `0.3*0 + 0.7*1.0 = 0.70` (audit finding).
pub fn find_similar_memory(
    store: &dyn MemoryStore,
    _embed_text: &str,
    embedding: &[f32],
    topic: &str,
    threshold: f32,
) -> IcmResult<Option<(Memory, f32)>> {
    let similar = store.search_by_embedding(embedding, 1)?;
    Ok(similar
        .into_iter()
        .find(|(m, score)| *score > threshold && m.topic == topic))
}

pub trait MemoryStore {
    // CRUD
    fn store(&self, memory: Memory) -> IcmResult<String>;
    fn get(&self, id: &str) -> IcmResult<Option<Memory>>;
    fn update(&self, memory: &Memory) -> IcmResult<()>;
    fn delete(&self, id: &str) -> IcmResult<()>;

    // Search
    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>>;
    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>>;
    fn search_by_embedding(&self, embedding: &[f32], limit: usize)
        -> IcmResult<Vec<(Memory, f32)>>;
    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>>;

    // Lifecycle
    fn update_access(&self, id: &str) -> IcmResult<()>;
    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize>;
    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize>;
    fn prune(&self, weight_threshold: f32) -> IcmResult<usize>;

    // Organization
    fn list_all(&self) -> IcmResult<Vec<Memory>>;
    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>>;
    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>>;
    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()>;

    // Stats
    fn count(&self) -> IcmResult<usize>;
    fn count_by_topic(&self, topic: &str) -> IcmResult<usize>;
    fn stats(&self) -> IcmResult<StoreStats>;
    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth>;
}
