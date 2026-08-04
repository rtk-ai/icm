use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::output::TypedTool;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 message types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcMessage {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Audit finding: plain `Option<Value>` can't distinguish an *absent*
    /// `id` field (a Notification — no response expected) from an id that's
    /// *explicitly* `null` (a legal, if discouraged, Request per JSON-RPC
    /// 2.0 — the server must still reply, with `id: null`). Both used to
    /// deserialize to `None` and get silently dropped as if they were
    /// Notifications, leaving a client that sent `"id": null` hanging
    /// forever with no response. `deserialize_some` + `#[serde(default)]`
    /// is the standard "double Option" idiom: missing → `None` (via
    /// default), present-as-null → `Some(Value::Null)`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub id: Option<Value>,
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
}

fn deserialize_some<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }

    pub fn err_with_data(id: Value, code: i64, message: String, data: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: Some(data),
            }),
        }
    }

    pub fn method_not_found(id: Value, method: &str) -> Self {
        Self::err(id, -32601, format!("method not found: {method}"))
    }
}

// ---------------------------------------------------------------------------
// MCP tool result
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ToolResult {
    pub content: Vec<TextContent>,
    #[serde(rename = "isError", skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    /// Short text fallback used only when structured output is emitted.
    /// Legacy clients still receive the full human-readable `content`.
    #[serde(skip)]
    pub structured_summary: Option<Box<String>>,
}

#[derive(Debug, Serialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl ToolResult {
    pub fn text(text: String) -> Self {
        Self {
            content: vec![TextContent {
                content_type: "text".into(),
                text,
            }],
            is_error: false,
            structured_content: None,
            structured_summary: None,
        }
    }

    pub fn error(text: String) -> Self {
        Self {
            content: vec![TextContent {
                content_type: "text".into(),
                text,
            }],
            is_error: true,
            structured_content: None,
            structured_summary: None,
        }
    }

    pub(crate) fn structured<T: TypedTool>(
        text: String,
        output: T::Output,
        structured_summary: String,
    ) -> Self {
        match serde_json::to_value(output) {
            Ok(data) => Self {
                content: vec![TextContent {
                    content_type: "text".into(),
                    text,
                }],
                is_error: false,
                structured_content: Some(data),
                structured_summary: Some(Box::new(structured_summary)),
            },
            Err(error) => {
                tracing::error!(tool = T::NAME, %error, "failed to serialize MCP structured output");
                Self::error(format!(
                    "failed to serialize structured output for {}",
                    T::NAME
                ))
            }
        }
    }

    /// Append a hint to the last text content block.
    pub fn append_hint(&mut self, hint: &str) {
        if let Some(last) = self.content.last_mut() {
            last.text.push_str(hint);
        }
        if let Some(summary) = self.structured_summary.as_mut() {
            summary.push_str(hint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde_json::json;

    #[derive(JsonSchema, Serialize)]
    struct TestOutput {
        count: usize,
    }

    struct TestTool;

    impl TypedTool for TestTool {
        const NAME: &'static str = "test_tool";
        type Output = TestOutput;
    }

    /// Audit regression: a Request with `"id": null` (legal per JSON-RPC
    /// 2.0, if discouraged) must still be recognized as a Request needing a
    /// response — not conflated with a Notification (missing `id`
    /// entirely), which gets no response at all.
    #[test]
    fn json_rpc_message_distinguishes_explicit_null_id_from_missing_id() {
        let with_null_id: JsonRpcMessage =
            serde_json::from_value(json!({"jsonrpc": "2.0", "id": null, "method": "ping"}))
                .unwrap();
        assert_eq!(
            with_null_id.id,
            Some(Value::Null),
            "explicit `\"id\": null` must deserialize to Some(Null), not None, \
             or the server silently drops the request as a Notification"
        );

        let without_id: JsonRpcMessage =
            serde_json::from_value(json!({"jsonrpc": "2.0", "method": "ping"})).unwrap();
        assert_eq!(
            without_id.id, None,
            "a genuinely absent id field must still deserialize to None (Notification)"
        );

        let with_real_id: JsonRpcMessage =
            serde_json::from_value(json!({"jsonrpc": "2.0", "id": 7, "method": "ping"})).unwrap();
        assert_eq!(with_real_id.id, Some(json!(7)));
    }

    #[test]
    fn test_tool_result_text() {
        let result = ToolResult::text("hello".into());
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text, "hello");
        assert_eq!(result.content[0].content_type, "text");
        assert!(result.structured_content.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("boom".into());
        assert!(result.is_error);
        assert_eq!(result.content[0].text, "boom");
        assert!(result.structured_content.is_none());
    }

    #[test]
    fn test_append_hint() {
        let mut result = ToolResult::text("original".into());
        result.append_hint("\n[nudge]");
        assert_eq!(result.content[0].text, "original\n[nudge]");
    }

    #[test]
    fn test_append_hint_empty_content() {
        let mut result = ToolResult {
            content: vec![],
            is_error: false,
            structured_content: None,
            structured_summary: None,
        };
        result.append_hint("[hint]");
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_jsonrpc_ok() {
        let resp = JsonRpcResponse::ok(json!(1), json!({"status": "ok"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, json!(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_jsonrpc_err() {
        let resp = JsonRpcResponse::err(json!(2), -32600, "bad request".into());
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "bad request");
    }

    #[test]
    fn test_jsonrpc_method_not_found() {
        let resp = JsonRpcResponse::method_not_found(json!(3), "tools/execute");
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("tools/execute"));
    }

    #[test]
    fn test_jsonrpc_parse_valid() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":null}"#;
        let msg: JsonRpcMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(msg.method.as_deref(), Some("ping"));
        assert_eq!(msg.id, Some(json!(1)));
    }

    #[test]
    fn test_jsonrpc_parse_missing_method() {
        let raw = r#"{"jsonrpc":"2.0","id":1}"#;
        let msg: JsonRpcMessage = serde_json::from_str(raw).unwrap();
        assert!(msg.method.is_none());
    }

    #[test]
    fn test_jsonrpc_parse_invalid_json() {
        let raw = r#"not json at all"#;
        let result: Result<JsonRpcMessage, _> = serde_json::from_str(raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_result_serializes_correctly() {
        let result = ToolResult::text("hello".into());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["content"][0]["type"], "text");
        assert_eq!(json["content"][0]["text"], "hello");
        assert!(json.get("structuredContent").is_none());
        // isError should be absent when false (skip_serializing_if)
        assert!(json.get("isError").is_none());
    }

    #[test]
    fn structured_summary_is_internal_and_preserves_hints() {
        let mut result = TestTool::result(
            "full legacy text".into(),
            TestOutput { count: 1 },
            "1 structured result".into(),
        );
        result.append_hint("\n[nudge]");
        let json = serde_json::to_value(&result).unwrap();

        assert_eq!(result.content[0].text, "full legacy text\n[nudge]");
        assert_eq!(
            result.structured_summary.as_deref().map(String::as_str),
            Some("1 structured result\n[nudge]")
        );
        assert!(json.get("structured_summary").is_none());
    }

    #[test]
    fn structured_serialization_errors_become_tool_errors() {
        #[derive(JsonSchema)]
        struct FailingOutput;

        impl Serialize for FailingOutput {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(<S::Error as serde::ser::Error>::custom("fixture failure"))
            }
        }

        struct FailingTool;

        impl TypedTool for FailingTool {
            const NAME: &'static str = "failing_tool";
            type Output = FailingOutput;
        }

        let result = FailingTool::result(
            "legacy text".into(),
            FailingOutput,
            "structured summary".into(),
        );

        assert!(result.is_error);
        assert!(result.structured_content.is_none());
        assert!(result.structured_summary.is_none());
        assert_eq!(
            result.content[0].text,
            "failed to serialize structured output for failing_tool"
        );
    }

    #[test]
    fn test_error_result_serializes_is_error() {
        let result = ToolResult::error("fail".into());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["isError"], true);
        assert!(json.get("structuredContent").is_none());
    }
}
