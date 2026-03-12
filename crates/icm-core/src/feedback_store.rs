use crate::error::IcmResult;
use crate::feedback::{Feedback, FeedbackStats};

pub trait FeedbackStore {
    fn store_feedback(&self, feedback: Feedback) -> IcmResult<String>;
    fn search_feedback(
        &self,
        query: &str,
        topic: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<Feedback>>;
    fn list_feedback(&self, topic: Option<&str>, limit: usize) -> IcmResult<Vec<Feedback>>;
    fn increment_applied(&self, id: &str) -> IcmResult<()>;
    fn delete_feedback(&self, id: &str) -> IcmResult<()>;
    fn feedback_stats(&self) -> IcmResult<FeedbackStats>;
}
