use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Serialize, Serializer};

use icm_core::{Message, Role, Session, TranscriptHit, TranscriptStats};

use super::TypedTool;

// Preserve the original structured-output builders' explicit UTC offset.
#[derive(Debug, JsonSchema)]
#[serde(transparent)]
struct Rfc3339Timestamp(DateTime<Utc>);

impl From<DateTime<Utc>> for Rfc3339Timestamp {
    fn from(value: DateTime<Utc>) -> Self {
        Self(value)
    }
}

impl Serialize for Rfc3339Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_rfc3339())
    }
}

pub(crate) struct TranscriptStartSessionTool;

impl TypedTool for TranscriptStartSessionTool {
    const NAME: &'static str = "icm_transcript_start_session";
    type Output = TranscriptStartSessionOutput;
}

pub(crate) struct TranscriptRecordTool;

impl TypedTool for TranscriptRecordTool {
    const NAME: &'static str = "icm_transcript_record";
    type Output = TranscriptRecordOutput;
}

pub(crate) struct TranscriptSearchTool;

impl TypedTool for TranscriptSearchTool {
    const NAME: &'static str = "icm_transcript_search";
    type Output = TranscriptSearchOutput;
}

pub(crate) struct TranscriptShowTool;

impl TypedTool for TranscriptShowTool {
    const NAME: &'static str = "icm_transcript_show";
    type Output = TranscriptShowOutput;
}

pub(crate) struct TranscriptStatsTool;

impl TypedTool for TranscriptStatsTool {
    const NAME: &'static str = "icm_transcript_stats";
    type Output = TranscriptStatsOutput;
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct TranscriptStartSessionOutput {
    session_id: String,
}

impl TranscriptStartSessionOutput {
    pub(crate) fn new(session_id: String) -> Self {
        Self { session_id }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct TranscriptRecordOutput {
    message_id: String,
    session_id: String,
}

impl TranscriptRecordOutput {
    pub(crate) fn new(message_id: String, session_id: String) -> Self {
        Self {
            message_id,
            session_id,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct TranscriptSearchOutput {
    count: usize,
    hits: Vec<TranscriptHitOutput>,
}

impl TranscriptSearchOutput {
    pub(crate) fn from_hits(hits: &[TranscriptHit]) -> Self {
        Self {
            count: hits.len(),
            hits: hits.iter().map(TranscriptHitOutput::from).collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct TranscriptHitOutput {
    message: TranscriptMessageOutput,
    session: TranscriptSessionOutput,
    score: f64,
}

impl From<&TranscriptHit> for TranscriptHitOutput {
    fn from(hit: &TranscriptHit) -> Self {
        Self {
            message: (&hit.message).into(),
            session: (&hit.session).into(),
            score: hit.score,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct TranscriptShowOutput {
    session: TranscriptSessionOutput,
    message_count: usize,
    messages: Vec<TranscriptMessageOutput>,
}

impl TranscriptShowOutput {
    pub(crate) fn new(session: &Session, messages: &[Message]) -> Self {
        Self {
            session: session.into(),
            message_count: messages.len(),
            messages: messages.iter().map(TranscriptMessageOutput::from).collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct TranscriptSessionOutput {
    id: String,
    agent: String,
    project: Option<String>,
    started_at: Rfc3339Timestamp,
    updated_at: Rfc3339Timestamp,
    metadata: String,
}

impl From<&Session> for TranscriptSessionOutput {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            agent: session.agent.clone(),
            project: session.project.clone(),
            started_at: session.started_at.into(),
            updated_at: session.updated_at.into(),
            metadata: session.metadata.clone(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct TranscriptMessageOutput {
    id: String,
    session_id: String,
    role: TranscriptRoleOutput,
    content: String,
    tool_name: Option<String>,
    tokens: Option<i64>,
    timestamp: Rfc3339Timestamp,
    metadata: String,
}

impl From<&Message> for TranscriptMessageOutput {
    fn from(message: &Message) -> Self {
        Self {
            id: message.id.clone(),
            session_id: message.session_id.clone(),
            role: message.role.into(),
            content: message.content.clone(),
            tool_name: message.tool_name.clone(),
            tokens: message.tokens,
            timestamp: message.ts.into(),
            metadata: message.metadata.clone(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum TranscriptRoleOutput {
    User,
    Assistant,
    System,
    Tool,
}

impl From<Role> for TranscriptRoleOutput {
    fn from(role: Role) -> Self {
        match role {
            Role::User => Self::User,
            Role::Assistant => Self::Assistant,
            Role::System => Self::System,
            Role::Tool => Self::Tool,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct TranscriptStatsOutput {
    total_sessions: usize,
    total_messages: usize,
    total_bytes: u64,
    by_role: Vec<RoleCountOutput>,
    by_agent: Vec<AgentCountOutput>,
    top_sessions: Vec<SessionCountOutput>,
    oldest: Option<Rfc3339Timestamp>,
    newest: Option<Rfc3339Timestamp>,
}

impl From<&TranscriptStats> for TranscriptStatsOutput {
    fn from(stats: &TranscriptStats) -> Self {
        Self {
            total_sessions: stats.total_sessions,
            total_messages: stats.total_messages,
            total_bytes: stats.total_bytes,
            by_role: stats.by_role.iter().map(RoleCountOutput::from).collect(),
            by_agent: stats.by_agent.iter().map(AgentCountOutput::from).collect(),
            top_sessions: stats
                .top_sessions
                .iter()
                .map(SessionCountOutput::from)
                .collect(),
            oldest: stats.oldest.map(Into::into),
            newest: stats.newest.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct RoleCountOutput {
    role: String,
    count: usize,
}

impl From<&(String, usize)> for RoleCountOutput {
    fn from((role, count): &(String, usize)) -> Self {
        Self {
            role: role.clone(),
            count: *count,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct AgentCountOutput {
    agent: String,
    count: usize,
}

impl From<&(String, usize)> for AgentCountOutput {
    fn from((agent, count): &(String, usize)) -> Self {
        Self {
            agent: agent.clone(),
            count: *count,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct SessionCountOutput {
    session_id: String,
    count: usize,
}

impl From<&(String, usize)> for SessionCountOutput {
    fn from((session_id, count): &(String, usize)) -> Self {
        Self {
            session_id: session_id.clone(),
            count: *count,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schemas_are_inline_closed_and_valid() {
        for schema in [
            TranscriptStartSessionTool::output_schema(),
            TranscriptRecordTool::output_schema(),
            TranscriptSearchTool::output_schema(),
            TranscriptShowTool::output_schema(),
            TranscriptStatsTool::output_schema(),
        ] {
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false);
            assert!(schema.get("$schema").is_none());
            assert!(schema.get("$defs").is_none());
            assert!(schema.get("title").is_none());
            assert!(jsonschema::draft202012::meta::is_valid(&schema));
        }
    }

    #[test]
    fn nested_schemas_preserve_wire_names_roles_and_dates() {
        let search = TranscriptSearchTool::output_schema();
        let message = &search["properties"]["hits"]["items"]["properties"]["message"];
        let session = &search["properties"]["hits"]["items"]["properties"]["session"];

        assert_eq!(message["additionalProperties"], false);
        assert_eq!(session["additionalProperties"], false);
        assert_eq!(message["properties"]["timestamp"]["format"], "date-time");
        assert_eq!(session["properties"]["startedAt"]["format"], "date-time");
        assert_eq!(
            message["properties"]["role"]["enum"],
            json!(["user", "assistant", "system", "tool"])
        );

        let stats = TranscriptStatsTool::output_schema();
        assert_eq!(
            stats["properties"]["topSessions"]["items"]["required"],
            json!(["sessionId", "count"])
        );
        assert_eq!(stats["properties"]["oldest"]["format"], "date-time");
        assert_eq!(stats["properties"]["newest"]["format"], "date-time");
    }

    #[test]
    fn typed_outputs_preserve_prior_timestamp_wire_shape() {
        let timestamp = "2026-08-04T18:30:00Z".parse::<DateTime<Utc>>().unwrap();
        let timestamp_text = timestamp.to_rfc3339();
        let session = Session {
            id: "session-1".into(),
            agent: "codex".into(),
            project: Some("icm".into()),
            started_at: timestamp,
            updated_at: timestamp,
            metadata: "{}".into(),
        };
        let message = Message {
            id: "message-1".into(),
            session_id: session.id.clone(),
            role: Role::Assistant,
            content: "completed".into(),
            tool_name: Some("icm_memory_store".into()),
            tokens: Some(12),
            ts: timestamp,
            metadata: "{}".into(),
        };
        let session_value = json!({
            "id": session.id,
            "agent": session.agent,
            "project": session.project,
            "startedAt": timestamp_text,
            "updatedAt": timestamp_text,
            "metadata": session.metadata
        });
        let message_value = json!({
            "id": message.id,
            "sessionId": message.session_id,
            "role": message.role.as_str(),
            "content": message.content,
            "toolName": message.tool_name,
            "tokens": message.tokens,
            "timestamp": timestamp_text,
            "metadata": message.metadata
        });
        let hit = TranscriptHit {
            message: message.clone(),
            session: session.clone(),
            score: 0.75,
        };

        assert_eq!(
            serde_json::to_value(TranscriptSearchOutput::from_hits(&[hit])).unwrap(),
            json!({
                "count": 1,
                "hits": [{
                    "message": message_value,
                    "session": session_value,
                    "score": 0.75
                }]
            })
        );
        assert_eq!(
            serde_json::to_value(TranscriptShowOutput::new(&session, &[message])).unwrap(),
            json!({
                "session": session_value,
                "messageCount": 1,
                "messages": [message_value]
            })
        );

        let stats = TranscriptStats {
            total_sessions: 1,
            total_messages: 1,
            total_bytes: 9,
            by_role: vec![("assistant".into(), 1)],
            by_agent: vec![("codex".into(), 1)],
            top_sessions: vec![("session-1".into(), 1)],
            oldest: Some(timestamp),
            newest: Some(timestamp),
        };
        assert_eq!(
            serde_json::to_value(TranscriptStatsOutput::from(&stats)).unwrap(),
            json!({
                "totalSessions": 1,
                "totalMessages": 1,
                "totalBytes": 9,
                "byRole": [{"role": "assistant", "count": 1}],
                "byAgent": [{"agent": "codex", "count": 1}],
                "topSessions": [{"sessionId": "session-1", "count": 1}],
                "oldest": timestamp_text,
                "newest": timestamp_text
            })
        );
        assert!(timestamp_text.ends_with("+00:00"));
    }
}
