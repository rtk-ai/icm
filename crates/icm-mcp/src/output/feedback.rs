use schemars::JsonSchema;
use serde::Serialize;

use icm_core::{Feedback, FeedbackStats};

use super::TypedTool;

pub(crate) struct FeedbackRecordTool;

impl TypedTool for FeedbackRecordTool {
    const NAME: &'static str = "icm_feedback_record";
    type Output = FeedbackRecordOutput;
}

pub(crate) struct FeedbackSearchTool;

impl TypedTool for FeedbackSearchTool {
    const NAME: &'static str = "icm_feedback_search";
    type Output = FeedbackSearchOutput;
}

pub(crate) struct FeedbackStatsTool;

impl TypedTool for FeedbackStatsTool {
    const NAME: &'static str = "icm_feedback_stats";
    type Output = FeedbackStatsOutput;
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct FeedbackRecordOutput {
    feedback_id: String,
    topic: String,
}

impl FeedbackRecordOutput {
    pub(crate) fn new(feedback_id: String, topic: String) -> Self {
        Self { feedback_id, topic }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct FeedbackSearchOutput {
    count: usize,
    feedback: Vec<FeedbackOutput>,
}

impl FeedbackSearchOutput {
    pub(crate) fn from_feedback(feedback: &[Feedback]) -> Self {
        Self {
            count: feedback.len(),
            feedback: feedback.iter().map(FeedbackOutput::from).collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct FeedbackOutput {
    id: String,
    topic: String,
    context: String,
    predicted: String,
    corrected: String,
    reason: Option<String>,
    source: String,
    created_at: String,
    applied_count: u32,
}

impl From<&Feedback> for FeedbackOutput {
    fn from(feedback: &Feedback) -> Self {
        Self {
            id: feedback.id.clone(),
            topic: feedback.topic.clone(),
            context: feedback.context.clone(),
            predicted: feedback.predicted.clone(),
            corrected: feedback.corrected.clone(),
            reason: feedback.reason.clone(),
            source: feedback.source.clone(),
            created_at: feedback.created_at.to_rfc3339(),
            applied_count: feedback.applied_count,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub(crate) struct FeedbackStatsOutput {
    total: usize,
    by_topic: Vec<TopicCountOutput>,
    most_applied: Vec<FeedbackCountOutput>,
}

impl From<&FeedbackStats> for FeedbackStatsOutput {
    fn from(stats: &FeedbackStats) -> Self {
        Self {
            total: stats.total,
            by_topic: stats.by_topic.iter().map(TopicCountOutput::from).collect(),
            most_applied: stats
                .most_applied
                .iter()
                .map(FeedbackCountOutput::from)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct TopicCountOutput {
    topic: String,
    count: usize,
}

impl From<&(String, usize)> for TopicCountOutput {
    fn from((topic, count): &(String, usize)) -> Self {
        Self {
            topic: topic.clone(),
            count: *count,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
struct FeedbackCountOutput {
    feedback_id: String,
    count: u32,
}

impl From<&(String, u32)> for FeedbackCountOutput {
    fn from((feedback_id, count): &(String, u32)) -> Self {
        Self {
            feedback_id: feedback_id.clone(),
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
            FeedbackRecordTool::output_schema(),
            FeedbackSearchTool::output_schema(),
            FeedbackStatsTool::output_schema(),
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
    fn search_schema_preserves_wire_fields_and_excludes_embeddings() {
        let schema = FeedbackSearchTool::output_schema();
        let item = &schema["properties"]["feedback"]["items"];

        assert_eq!(item["additionalProperties"], false);
        assert_eq!(item["properties"]["createdAt"]["type"], "string");
        assert!(item["properties"].get("embedding").is_none());
        assert!(item["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|name| name == "reason")));
        assert!(jsonschema::draft202012::is_valid(
            &item["properties"]["reason"],
            &serde_json::Value::Null
        ));
    }

    #[test]
    fn conversion_preserves_the_existing_feedback_wire_shape() {
        let mut feedback = Feedback::new(
            "review".into(),
            "context".into(),
            "old".into(),
            "new".into(),
            Some("reason".into()),
            "mcp".into(),
        );
        feedback.embedding = Some(vec![0.1, 0.2]);
        let created_at = feedback.created_at.to_rfc3339();

        let value = serde_json::to_value(FeedbackSearchOutput::from_feedback(&[feedback])).unwrap();
        let item = &value["feedback"][0];

        assert_eq!(value["count"], 1);
        assert_eq!(item["createdAt"], created_at);
        assert_eq!(item["reason"], json!("reason"));
        assert!(item.get("embedding").is_none());
    }
}
