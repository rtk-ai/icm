use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub id: String,
    pub topic: String,
    pub context: String,
    pub predicted: String,
    pub corrected: String,
    pub reason: Option<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub applied_count: u32,
}

impl Feedback {
    pub fn new(
        topic: String,
        context: String,
        predicted: String,
        corrected: String,
        reason: Option<String>,
        source: String,
    ) -> Self {
        Self {
            id: ulid::Ulid::new().to_string(),
            topic,
            context,
            predicted,
            corrected,
            reason,
            source,
            created_at: Utc::now(),
            applied_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackStats {
    pub total: usize,
    pub by_topic: Vec<(String, usize)>,
    pub most_applied: Vec<(String, u32)>,
}
