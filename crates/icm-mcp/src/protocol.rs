use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 message types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcMessage {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
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
            error: Some(JsonRpcError { code, message }),
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
        }
    }

    pub fn error(text: String) -> Self {
        Self {
            content: vec![TextContent {
                content_type: "text".into(),
                text,
            }],
            is_error: true,
        }
    }

    /// Append a hint to the last text content block.
    pub fn append_hint(&mut self, hint: &str) {
        if let Some(last) = self.content.last_mut() {
            last.text.push_str(hint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_result_text() {
        let result = ToolResult::text("hello".into());
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text, "hello");
        assert_eq!(result.content[0].content_type, "text");
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("boom".into());
        assert!(result.is_error);
        assert_eq!(result.content[0].text, "boom");
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
        // isError should be absent when false (skip_serializing_if)
        assert!(json.get("isError").is_none());
    }

    #[test]
    fn test_error_result_serializes_is_error() {
        let result = ToolResult::error("fail".into());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["isError"], true);
    }
}
