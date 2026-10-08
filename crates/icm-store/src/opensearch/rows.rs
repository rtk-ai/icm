//! OpenSearch backend -- split out of the former monolithic opensearch.rs.
//!
//! Row/parse helpers and doc<->Memory mapping shared by every trait-impl
//! submodule here.

use super::*;

/// Percent-encode a value for safe use as a single path segment in a REST
/// URL. Document ids are caller-controlled (`icm forget <id>` CLI, MCP
/// `icm_forget`, etc.) with no format constraint enforced anywhere in the
/// schema — without this, a crafted id containing `/`, `..`, `?`, or `#`
/// could redirect which REST endpoint is actually hit instead of just
/// addressing the intended document (audit finding).
pub(crate) fn url_encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// Pure helpers (self-contained, mirror the other backends)

pub(crate) fn source_type(source: &MemorySource) -> &'static str {
    match source {
        MemorySource::ClaudeCode { .. } => "claude_code",
        MemorySource::Conversation { .. } => "conversation",
        MemorySource::Manual => "manual",
    }
}

pub(crate) fn source_data(source: &MemorySource) -> Option<String> {
    match source {
        MemorySource::Manual => None,
        other => serde_json::to_string(other).ok(),
    }
}

pub(crate) fn parse_source(source_type_str: &str, source_data_str: Option<String>) -> MemorySource {
    match source_type_str {
        "manual" => MemorySource::Manual,
        _ => source_data_str
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or(MemorySource::Manual),
    }
}

pub(crate) fn importance_rank(i: Importance) -> u8 {
    match i {
        Importance::Low => 0,
        Importance::Medium => 1,
        Importance::High => 2,
        Importance::Critical => 3,
    }
}

pub(crate) fn max_importance(a: Importance, b: Importance) -> Importance {
    if importance_rank(a) >= importance_rank(b) {
        a
    } else {
        b
    }
}

/// SHA-256 over the normalized `(topic, summary)` pair, hex-encoded.
/// Normalization: trim + lowercase + collapse whitespace, joined by `\0`.
pub(crate) fn summary_hash(topic: &str, summary: &str) -> String {
    use sha2::{Digest, Sha256};
    let topic_n = topic.trim().to_lowercase();
    let summary_n = summary
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase();
    let mut h = Sha256::new();
    h.update(topic_n.as_bytes());
    h.update(b"\0");
    h.update(summary_n.as_bytes());
    icm_core::to_hex(&h.finalize())
}

/// Validate and normalize a memory before storing (mirror of the other
/// backends): non-empty topic/summary, generate an id if missing, and
/// stamp timestamps.
pub(crate) fn validate_and_normalize(mut memory: Memory) -> IcmResult<Memory> {
    if memory.topic.trim().is_empty() {
        return Err(IcmError::InvalidInput("topic cannot be empty".into()));
    }
    if memory.summary.trim().is_empty() {
        return Err(IcmError::InvalidInput("summary cannot be empty".into()));
    }
    if memory.id.trim().is_empty() {
        memory.id = ulid::Ulid::generate().to_string();
    }
    memory.topic = memory.topic.trim().to_string();
    Ok(memory)
}

pub(crate) fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

impl OpenSearchStore {
    // (de)serialization

    pub(crate) fn memory_to_source(memory: &Memory) -> Value {
        let mut doc = json!({
            "created_at": memory.created_at.to_rfc3339(),
            "updated_at": memory.updated_at.to_rfc3339(),
            "last_accessed": memory.last_accessed.to_rfc3339(),
            "access_count": memory.access_count,
            "weight": memory.weight,
            "topic": memory.topic,
            "summary": memory.summary,
            "raw_excerpt": memory.raw_excerpt,
            "keywords": memory.keywords,
            "importance": memory.importance.to_string(),
            "source_type": source_type(&memory.source),
            "source_data": source_data(&memory.source),
            "related_ids": memory.related_ids,
            "summary_hash": summary_hash(&memory.topic, &memory.summary),
        });
        if let Some(emb) = memory.embedding.as_ref() {
            doc["embedding"] = json!(emb);
        }
        doc
    }

    pub(crate) fn source_to_memory(id: &str, src: &Value) -> Memory {
        let get_str = |k: &str| {
            src.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let opt_str = |k: &str| {
            src.get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        };
        let arr = |k: &str| {
            src.get(k)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default()
        };
        let importance = get_str("importance").parse().unwrap_or(Importance::Medium);
        let source = parse_source(&get_str("source_type"), opt_str("source_data"));
        let embedding = src.get("embedding").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>()
        });
        Memory {
            id: id.to_string(),
            created_at: parse_dt(&get_str("created_at")),
            updated_at: parse_dt(&get_str("updated_at")),
            last_accessed: parse_dt(&get_str("last_accessed")),
            access_count: src
                .get("access_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            weight: src.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            topic: get_str("topic"),
            summary: get_str("summary"),
            raw_excerpt: opt_str("raw_excerpt"),
            keywords: arr("keywords"),
            importance,
            source,
            related_ids: arr("related_ids"),
            embedding,
            scope: Scope::default(),
        }
    }

    /// Map a `_search` response's hits to memories paired with `_score`.
    pub(crate) fn hits_to_scored(resp: &Value) -> Vec<(Memory, f32)> {
        resp.get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let id = h.get("_id")?.as_str()?;
                        let src = h.get("_source")?;
                        let score = h.get("_score").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                        Some((Self::source_to_memory(id, src), score))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn hits_to_memories(resp: &Value) -> Vec<Memory> {
        Self::hits_to_scored(resp)
            .into_iter()
            .map(|(m, _)| m)
            .collect()
    }
}
