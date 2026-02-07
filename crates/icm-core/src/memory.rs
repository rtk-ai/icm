use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub access_count: u32,
    pub weight: f32,

    pub topic: String,
    pub summary: String,
    pub raw_excerpt: Option<String>,
    pub keywords: Vec<String>,

    pub importance: Importance,
    pub source: MemorySource,

    pub related_ids: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

impl Memory {
    pub fn new(topic: String, summary: String, importance: Importance) -> Self {
        let now = Utc::now();
        Self {
            id: ulid::Ulid::new().to_string(),
            created_at: now,
            last_accessed: now,
            access_count: 0,
            weight: 1.0,
            topic,
            summary,
            raw_excerpt: None,
            keywords: Vec::new(),
            importance,
            source: MemorySource::Manual,
            related_ids: Vec::new(),
            embedding: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importance {
    Critical,
    High,
    Medium,
    Low,
}

impl fmt::Display for Importance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

impl std::str::FromStr for Importance {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "critical" => Ok(Self::Critical),
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            _ => Err(format!("invalid importance: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemorySource {
    ClaudeCode {
        session_id: String,
        file_path: Option<String>,
    },
    Conversation {
        thread_id: String,
    },
    Manual,
}

impl fmt::Display for MemorySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClaudeCode { session_id, .. } => write!(f, "claude-code:{session_id}"),
            Self::Conversation { thread_id } => write!(f, "conversation:{thread_id}"),
            Self::Manual => write!(f, "manual"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub total_memories: usize,
    pub total_topics: usize,
    pub avg_weight: f32,
    pub oldest_memory: Option<DateTime<Utc>>,
    pub newest_memory: Option<DateTime<Utc>>,
}
