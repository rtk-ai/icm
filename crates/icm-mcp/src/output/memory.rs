use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;

use icm_core::{Importance, Memory, MemorySource, Scope, StoreStats};

use super::TypedTool;

pub(crate) struct RecallTool;

impl TypedTool for RecallTool {
    const NAME: &'static str = "icm_memory_recall";
    type Output = RecallOutput;
}

pub(crate) struct ListTopicsTool;

impl TypedTool for ListTopicsTool {
    const NAME: &'static str = "icm_memory_list_topics";
    type Output = ListTopicsOutput;
}

pub(crate) struct StatsTool;

impl TypedTool for StatsTool {
    const NAME: &'static str = "icm_memory_stats";
    type Output = StatsOutput;
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct RecallOutput {
    count: usize,
    memories: Vec<RecallMemoryOutput>,
}

impl RecallOutput {
    pub(crate) fn from_memories(memories: &[(Memory, f32)]) -> Self {
        let memories = memories
            .iter()
            .map(RecallMemoryOutput::from)
            .collect::<Vec<_>>();
        Self {
            count: memories.len(),
            memories,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct RecallMemoryOutput {
    id: String,
    topic: String,
    summary: String,
    raw_excerpt: Option<String>,
    keywords: Vec<String>,
    importance: ImportanceOutput,
    source: MemorySourceOutput,
    scope: ScopeOutput,
    related_ids: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_accessed: DateTime<Utc>,
    access_count: u32,
    weight: f32,
    score: Option<f32>,
}

impl From<&(Memory, f32)> for RecallMemoryOutput {
    fn from((memory, score): &(Memory, f32)) -> Self {
        Self {
            id: memory.id.clone(),
            topic: memory.topic.clone(),
            summary: memory.summary.clone(),
            raw_excerpt: memory.raw_excerpt.clone(),
            keywords: memory.keywords.clone(),
            importance: memory.importance.into(),
            source: (&memory.source).into(),
            scope: memory.scope.into(),
            related_ids: memory.related_ids.clone(),
            created_at: memory.created_at,
            updated_at: memory.updated_at,
            last_accessed: memory.last_accessed,
            access_count: memory.access_count,
            weight: memory.weight,
            score: (*score >= 0.0).then_some(*score),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ImportanceOutput {
    Critical,
    High,
    Medium,
    Low,
}

impl From<Importance> for ImportanceOutput {
    fn from(value: Importance) -> Self {
        match value {
            Importance::Critical => Self::Critical,
            Importance::High => Self::High,
            Importance::Medium => Self::Medium,
            Importance::Low => Self::Low,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(deny_unknown_fields)]
enum MemorySourceOutput {
    ClaudeCode {
        session_id: String,
        file_path: Option<String>,
    },
    Conversation {
        thread_id: String,
    },
    Manual,
}

impl From<&MemorySource> for MemorySourceOutput {
    fn from(value: &MemorySource) -> Self {
        match value {
            MemorySource::ClaudeCode {
                session_id,
                file_path,
            } => Self::ClaudeCode {
                session_id: session_id.clone(),
                file_path: file_path.clone(),
            },
            MemorySource::Conversation { thread_id } => Self::Conversation {
                thread_id: thread_id.clone(),
            },
            MemorySource::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ScopeOutput {
    User,
    Project,
    Org,
}

impl From<Scope> for ScopeOutput {
    fn from(value: Scope) -> Self {
        match value {
            Scope::User => Self::User,
            Scope::Project => Self::Project,
            Scope::Org => Self::Org,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct ListTopicsOutput {
    total_topics: usize,
    total_memories: usize,
    topics: Vec<TopicOutput>,
}

impl ListTopicsOutput {
    pub(crate) fn from_topics(topics: &[(String, usize)]) -> Self {
        Self {
            total_topics: topics.len(),
            total_memories: topics.iter().map(|(_, count)| count).sum(),
            topics: topics
                .iter()
                .map(|(name, count)| TopicOutput {
                    name: name.clone(),
                    count: *count,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct TopicOutput {
    name: String,
    count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct StatsOutput {
    total_memories: usize,
    total_topics: usize,
    average_weight: f32,
    oldest_memory: Option<DateTime<Utc>>,
    newest_memory: Option<DateTime<Utc>>,
}

impl From<&StoreStats> for StatsOutput {
    fn from(stats: &StoreStats) -> Self {
        Self {
            total_memories: stats.total_memories,
            total_topics: stats.total_topics,
            average_weight: stats.avg_weight,
            oldest_memory: stats.oldest_memory,
            newest_memory: stats.newest_memory,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn schemas_are_inline_closed_and_lean() {
        for schema in [
            RecallTool::output_schema(),
            ListTopicsTool::output_schema(),
            StatsTool::output_schema(),
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
    fn recall_schema_preserves_wire_enums_nullability_and_dates() {
        let schema = RecallTool::output_schema();
        let memory = &schema["properties"]["memories"]["items"];

        assert_eq!(memory["additionalProperties"], false);
        assert_eq!(memory["properties"]["createdAt"]["format"], "date-time");
        assert_eq!(
            memory["properties"]["importance"]["enum"],
            serde_json::json!(["critical", "high", "medium", "low"])
        );
        assert!(memory["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|name| name == "score")));
        assert!(jsonschema::draft202012::is_valid(
            &memory["properties"]["score"],
            &Value::Null
        ));
    }
}
