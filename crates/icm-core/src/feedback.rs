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
    /// Manual-testing finding: `feedback search` had no semantic fallback
    /// at all — pure FTS5 with implicit AND, so a query missing even one
    /// exact token (no stemming) returned nothing, even with an obviously
    /// relevant entry present. Mirrors `Memory::embedding`: attached by
    /// the caller (CLI/MCP) via an `Embedder` before `store_feedback`.
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
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
            embedding: None,
        }
    }

    /// Text used to compute this feedback's embedding — mirrors
    /// `Memory::embed_text`. `context` carries the situation, `predicted`/
    /// `corrected` carry the actual correction content a future query is
    /// most likely to be phrased against.
    pub fn embed_text(&self) -> String {
        format!("{} {} {}", self.context, self.predicted, self.corrected)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackStats {
    pub total: usize,
    pub by_topic: Vec<(String, usize)>,
    pub most_applied: Vec<(String, u32)>,
}
