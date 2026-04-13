use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "system" => Some(Role::System),
            "tool" => Some(Role::Tool),
            _ => None,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub project: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: String, // JSON
}

impl Session {
    pub fn new(agent: String, project: Option<String>, metadata: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: ulid::Ulid::new().to_string(),
            agent,
            project,
            started_at: now,
            updated_at: now,
            metadata: metadata.unwrap_or_else(|| "{}".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: Role,
    pub content: String,
    pub tool_name: Option<String>,
    pub tokens: Option<i64>,
    pub ts: DateTime<Utc>,
    pub metadata: String, // JSON
}

impl Message {
    pub fn new(
        session_id: String,
        role: Role,
        content: String,
        tool_name: Option<String>,
        tokens: Option<i64>,
        metadata: Option<String>,
    ) -> Self {
        Self {
            id: ulid::Ulid::new().to_string(),
            session_id,
            role,
            content,
            tool_name,
            tokens,
            ts: Utc::now(),
            metadata: metadata.unwrap_or_else(|| "{}".into()),
        }
    }
}

/// A search hit with the message, its parent session, and a relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptHit {
    pub message: Message,
    pub session: Session,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptStats {
    pub total_sessions: usize,
    pub total_messages: usize,
    pub total_bytes: u64,
    pub by_role: Vec<(String, usize)>,
    pub by_agent: Vec<(String, usize)>,
    pub top_sessions: Vec<(String, usize)>, // (session_id, message_count)
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
}
