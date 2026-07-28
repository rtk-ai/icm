use crate::error::IcmResult;
use crate::feedback::{Feedback, FeedbackStats};

pub trait FeedbackStore {
    fn store_feedback(&self, feedback: Feedback) -> IcmResult<String>;
    /// `query_embedding`: when the caller has an embedder available, pass
    /// the query's embedding to blend semantic similarity (70%) with FTS
    /// (30%) — same weights as `MemoryStore::search_hybrid`. `None` falls
    /// back to FTS-only, still ordered by relevance (not just recency).
    fn search_feedback(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        topic: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<Feedback>>;
    fn list_feedback(&self, topic: Option<&str>, limit: usize) -> IcmResult<Vec<Feedback>>;
    fn increment_applied(&self, id: &str) -> IcmResult<()>;
    fn delete_feedback(&self, id: &str) -> IcmResult<()>;
    fn feedback_stats(&self) -> IcmResult<FeedbackStats>;
}
