mod memory;
mod transcript;

use schemars::{generate::SchemaSettings, JsonSchema};
use serde::Serialize;
use serde_json::Value;

use crate::protocol::ToolResult;

pub(crate) use memory::{
    ListTopicsOutput, ListTopicsTool, RecallOutput, RecallTool, StatsOutput, StatsTool,
};
pub(crate) use transcript::{
    TranscriptRecordOutput, TranscriptRecordTool, TranscriptSearchOutput, TranscriptSearchTool,
    TranscriptShowOutput, TranscriptShowTool, TranscriptStartSessionOutput,
    TranscriptStartSessionTool, TranscriptStatsOutput, TranscriptStatsTool,
};

pub(crate) trait ToolOutput: Serialize + JsonSchema {}

impl<T> ToolOutput for T where T: Serialize + JsonSchema {}

/// Associates one MCP tool name with the only structured output it may emit.
pub(crate) trait TypedTool {
    const NAME: &'static str;
    type Output: ToolOutput;

    fn output_schema() -> Value {
        output_schema::<Self::Output>()
    }

    fn result(text: String, output: Self::Output, summary: String) -> ToolResult
    where
        Self: Sized,
    {
        ToolResult::structured::<Self>(text, output, summary)
    }
}

fn output_schema<T: ToolOutput>() -> Value {
    let settings = SchemaSettings::draft2020_12()
        .for_serialize()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        });
    let mut schema = settings.into_generator().into_root_schema_for::<T>();

    // MCP embeds this inside a tool definition, where root document metadata
    // adds bytes without helping clients interpret the output contract.
    schema.remove("title");
    schema.to_value()
}
