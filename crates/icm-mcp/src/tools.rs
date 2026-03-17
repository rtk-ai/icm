use chrono::Utc;
use serde_json::{json, Value};

use icm_core::{
    Concept, ConceptLink, Embedder, Feedback, FeedbackStore, Label, Memoir, MemoirStore, Memory,
    MemoryStore, Relation,
};
use icm_store::SqliteStore;

use crate::protocol::ToolResult;

/// Default threshold for auto-consolidation (can be overridden by config).
const AUTO_CONSOLIDATE_THRESHOLD: usize = 10;

/// Try to auto-consolidate a topic if it exceeds the threshold.
/// Returns a human-readable message if consolidation happened, or empty string.
fn try_auto_consolidate(store: &SqliteStore, topic: &str, threshold: usize) -> String {
    match store.auto_consolidate(topic, threshold) {
        Ok(true) => format!("Auto-consolidated topic '{topic}' (exceeded {threshold} entries)."),
        Ok(false) => String::new(),
        Err(e) => {
            tracing::warn!("auto-consolidation failed for topic '{topic}': {e}");
            String::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Tool schemas for tools/list
// ---------------------------------------------------------------------------

pub fn tool_definitions(has_embedder: bool) -> Value {
    let mut tools = vec![
        // --- Memory tools ---
        json!({
            "name": "icm_memory_store",
            "description": "Store important information in ICM long-term memory. Use to save decisions, preferences, project context, resolved errors — anything that should persist between sessions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Category/namespace (e.g. 'projet-kexa', 'preferences', 'decisions-architecture', 'erreurs-resolues')"
                    },
                    "content": {
                        "type": "string",
                        "description": "Information to memorize — be concise but complete"
                    },
                    "importance": {
                        "type": "string",
                        "enum": ["critical", "high", "medium", "low"],
                        "default": "medium",
                        "description": "critical=never forgotten, high=slow decay, medium=normal, low=fast decay"
                    },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Keywords to improve search"
                    },
                    "raw_excerpt": {
                        "type": "string",
                        "description": "Optional verbatim (code, exact error message, etc.)"
                    }
                },
                "required": ["topic", "content"]
            }
        }),
        json!({
            "name": "icm_memory_recall",
            "description": "Search ICM long-term memory. Use to find past decisions, project context, preferences, or solutions to previously encountered problems.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query"
                    },
                    "topic": {
                        "type": "string",
                        "description": "Filter by specific topic (optional)"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Max number of results"
                    },
                    "keyword": {
                        "type": "string",
                        "description": "Filter results by keyword (exact match on memory keywords)"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "icm_memory_forget",
            "description": "Delete a specific memory by its ID. Use when information is obsolete or incorrect.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Memory ID to delete"
                    }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "icm_memory_consolidate",
            "description": "Consolidate all memories of a topic into a single summary. Useful when a topic accumulates too many entries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Topic to consolidate"
                    },
                    "summary": {
                        "type": "string",
                        "description": "Consolidated summary to replace all memories in the topic"
                    }
                },
                "required": ["topic", "summary"]
            }
        }),
        json!({
            "name": "icm_memory_list_topics",
            "description": "List all available topics in memory with their counts.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "icm_memory_stats",
            "description": "Get global ICM memory statistics.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "icm_memory_update",
            "description": "Update an existing memory in-place. Use to correct, refresh, or extend a memory without creating a duplicate.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Memory ID to update"
                    },
                    "content": {
                        "type": "string",
                        "description": "New content (replaces existing summary)"
                    },
                    "importance": {
                        "type": "string",
                        "enum": ["critical", "high", "medium", "low"],
                        "description": "New importance level (optional, keeps existing if not set)"
                    },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New keywords (optional, keeps existing if not set)"
                    }
                },
                "required": ["id", "content"]
            }
        }),
        json!({
            "name": "icm_memory_health",
            "description": "Get health stats for all topics: entry count, staleness, consolidation needs. Use to audit memory hygiene.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Check a specific topic (optional — checks all if omitted)"
                    }
                }
            }
        }),
        // --- Memoir tools ---
        json!({
            "name": "icm_memoir_create",
            "description": "Create a new memoir — a permanent knowledge container. Memoirs hold concepts that never decay.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Unique human-readable name for the memoir"
                    },
                    "description": {
                        "type": "string",
                        "description": "Description of what this memoir is for"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "icm_memoir_list",
            "description": "List all memoirs with their concept counts.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "icm_memoir_show",
            "description": "Show a memoir's stats, labels, and all its concepts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Memoir name"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "icm_memoir_add_concept",
            "description": "Add a permanent concept to a memoir. Concepts are knowledge nodes that get refined, never decayed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memoir": {
                        "type": "string",
                        "description": "Memoir name"
                    },
                    "name": {
                        "type": "string",
                        "description": "Concept name (unique within memoir)"
                    },
                    "definition": {
                        "type": "string",
                        "description": "Dense description of the concept"
                    },
                    "labels": {
                        "type": "string",
                        "description": "Comma-separated labels (namespace:value or plain tag). E.g. 'domain:arch,type:decision'"
                    }
                },
                "required": ["memoir", "name", "definition"]
            }
        }),
        json!({
            "name": "icm_memoir_refine",
            "description": "Refine an existing concept with a new, improved definition. Bumps revision and boosts confidence.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memoir": {
                        "type": "string",
                        "description": "Memoir name"
                    },
                    "name": {
                        "type": "string",
                        "description": "Concept name"
                    },
                    "definition": {
                        "type": "string",
                        "description": "New, refined definition"
                    }
                },
                "required": ["memoir", "name", "definition"]
            }
        }),
        json!({
            "name": "icm_memoir_search",
            "description": "Full-text search concepts within a memoir.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memoir": {
                        "type": "string",
                        "description": "Memoir name"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "label": {
                        "type": "string",
                        "description": "Filter by label (e.g. 'domain:tech')"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "description": "Max results"
                    }
                },
                "required": ["memoir", "query"]
            }
        }),
        json!({
            "name": "icm_memoir_link",
            "description": "Create a directed, typed edge between two concepts in the same memoir.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memoir": {
                        "type": "string",
                        "description": "Memoir name"
                    },
                    "from": {
                        "type": "string",
                        "description": "Source concept name"
                    },
                    "to": {
                        "type": "string",
                        "description": "Target concept name"
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["part_of", "depends_on", "related_to", "contradicts", "refines", "alternative_to", "caused_by", "instance_of", "superseded_by"],
                        "description": "Relation type"
                    }
                },
                "required": ["memoir", "from", "to", "relation"]
            }
        }),
        json!({
            "name": "icm_memoir_inspect",
            "description": "Inspect a concept and its graph neighborhood (BFS).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memoir": {
                        "type": "string",
                        "description": "Memoir name"
                    },
                    "name": {
                        "type": "string",
                        "description": "Concept name"
                    },
                    "depth": {
                        "type": "integer",
                        "default": 1,
                        "description": "BFS depth"
                    }
                },
                "required": ["memoir", "name"]
            }
        }),
        json!({
            "name": "icm_memoir_export",
            "description": "Export a memoir's full concept graph. Formats: json (structured), dot (Graphviz), ascii (visual), ai (compact markdown for LLM context).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Memoir name"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["json", "dot", "ascii", "ai"],
                        "default": "json",
                        "description": "Output format: json (structured), dot (Graphviz), ascii (visual graph), ai (compact markdown for LLM)"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "icm_memory_extract_patterns",
            "description": "Detect recurring patterns in a topic by keyword similarity. Optionally create concepts in a memoir from detected patterns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Topic to analyze for patterns"
                    },
                    "memoir": {
                        "type": "string",
                        "description": "Memoir name — if provided, creates concepts from detected patterns"
                    },
                    "min_cluster_size": {
                        "type": "integer",
                        "default": 3,
                        "minimum": 2,
                        "description": "Minimum number of similar memories to form a pattern (default: 3)"
                    }
                },
                "required": ["topic"]
            }
        }),
        json!({
            "name": "icm_memoir_search_all",
            "description": "Full-text search concepts across all memoirs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "description": "Max results"
                    }
                },
                "required": ["query"]
            }
        }),
        // --- Feedback tools ---
        json!({
            "name": "icm_feedback_record",
            "description": "Record a correction/feedback when an AI prediction was wrong. Helps improve future predictions by learning from mistakes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Category/namespace for this feedback (e.g. 'triage-owner/repo', 'pr-analysis')"
                    },
                    "context": {
                        "type": "string",
                        "description": "What was the situation / input that led to the prediction"
                    },
                    "predicted": {
                        "type": "string",
                        "description": "What the AI predicted or did"
                    },
                    "corrected": {
                        "type": "string",
                        "description": "What the correct answer/action should have been"
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why the correction was made (optional)"
                    },
                    "source": {
                        "type": "string",
                        "description": "Which tool/pipeline generated the prediction (optional)"
                    }
                },
                "required": ["topic", "context", "predicted", "corrected"]
            }
        }),
        json!({
            "name": "icm_feedback_search",
            "description": "Search past feedback/corrections to inform current predictions. Use before making predictions to learn from past mistakes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query to find relevant past corrections"
                    },
                    "topic": {
                        "type": "string",
                        "description": "Filter by topic (optional)"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Max number of results"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "icm_feedback_stats",
            "description": "Get feedback statistics: total count, breakdown by topic, most applied corrections.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
    ];

    if has_embedder {
        tools.push(json!({
            "name": "icm_memory_embed_all",
            "description": "Generate embeddings for all memories that don't have one yet. Use this to backfill vector search capability.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Only embed memories in this topic (optional)"
                    }
                }
            }
        }));
    }

    json!({ "tools": tools })
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

pub fn call_tool(
    store: &SqliteStore,
    embedder: Option<&dyn Embedder>,
    name: &str,
    args: &Value,
    compact: bool,
) -> ToolResult {
    match name {
        // Memory tools
        "icm_memory_store" => tool_store(store, embedder, args, compact),
        "icm_memory_recall" => tool_recall(store, embedder, args, compact),
        "icm_memory_forget" => tool_forget(store, args),
        "icm_memory_update" => tool_update(store, embedder, args),
        "icm_memory_consolidate" => tool_consolidate(store, args),
        "icm_memory_list_topics" => tool_list_topics(store),
        "icm_memory_stats" => tool_stats(store),
        "icm_memory_health" => tool_health(store, args),
        "icm_memory_extract_patterns" => tool_extract_patterns(store, args),
        "icm_memory_embed_all" => tool_embed_all(store, embedder, args),
        // Memoir tools
        "icm_memoir_create" => tool_memoir_create(store, args),
        "icm_memoir_list" => tool_memoir_list(store),
        "icm_memoir_show" => tool_memoir_show(store, args),
        "icm_memoir_add_concept" => tool_memoir_add_concept(store, args),
        "icm_memoir_refine" => tool_memoir_refine(store, args),
        "icm_memoir_search" => tool_memoir_search(store, args),
        "icm_memoir_search_all" => tool_memoir_search_all(store, args),
        "icm_memoir_link" => tool_memoir_link(store, args),
        "icm_memoir_inspect" => tool_memoir_inspect(store, args),
        "icm_memoir_export" => tool_memoir_export(store, args),
        // Feedback tools
        "icm_feedback_record" => tool_feedback_record(store, args, compact),
        "icm_feedback_search" => tool_feedback_search(store, args),
        "icm_feedback_stats" => tool_feedback_stats(store),
        _ => ToolResult::error(format!("unknown tool: {name}")),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn get_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn resolve_memoir(store: &SqliteStore, name: &str) -> Result<Memoir, ToolResult> {
    store
        .get_memoir_by_name(name)
        .map_err(|e| ToolResult::error(format!("db error: {e}")))?
        .ok_or_else(|| ToolResult::error(format!("memoir not found: {name}")))
}

// ---------------------------------------------------------------------------
// Memory tool handlers
// ---------------------------------------------------------------------------

fn tool_store(
    store: &SqliteStore,
    embedder: Option<&dyn Embedder>,
    args: &Value,
    compact: bool,
) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t,
        None => return ToolResult::error("missing required field: topic".into()),
    };
    let content = match get_str(args, "content") {
        Some(c) => c,
        None => return ToolResult::error("missing required field: content".into()),
    };
    let importance_str = get_str(args, "importance").unwrap_or("medium");
    let importance = importance_str
        .parse()
        .unwrap_or(icm_core::Importance::Medium);

    let mut memory = Memory::new(topic.into(), content.into(), importance);

    if let Some(keywords) = args.get("keywords").and_then(|v| v.as_array()) {
        memory.keywords = keywords
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    if let Some(raw) = get_str(args, "raw_excerpt") {
        memory.raw_excerpt = Some(raw.into());
    }

    // Auto-embed if embedder is available
    let embed_text = format!("{topic} {content}");
    let embed_vec = if let Some(emb) = embedder {
        match emb.embed(&embed_text) {
            Ok(vec) => Some(vec),
            Err(e) => {
                tracing::warn!("embedding failed: {e}");
                None
            }
        }
    } else {
        None
    };

    if let Some(ref vec) = embed_vec {
        memory.embedding = Some(vec.clone());
    }

    // Dedup check: if a very similar memory exists in the same topic, update it instead
    if let Some(ref query_emb) = embed_vec {
        if let Ok(similar) = store.search_hybrid(&embed_text, query_emb, 1) {
            if let Some((existing, score)) = similar.first() {
                if score > &0.85 && existing.topic == topic {
                    // Very similar content in same topic — update instead of duplicate
                    let mut updated = existing.clone();
                    updated.summary = content.to_string();
                    updated.updated_at = Utc::now();
                    updated.weight = 1.0; // Reset weight on update
                    if let Some(raw) = get_str(args, "raw_excerpt") {
                        updated.raw_excerpt = Some(raw.into());
                    }
                    if let Some(keywords_arr) = args.get("keywords").and_then(|v| v.as_array()) {
                        updated.keywords = keywords_arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                    }
                    updated.importance = importance;
                    updated.embedding = Some(query_emb.clone());
                    if let Err(e) = store.update(&updated) {
                        return ToolResult::error(format!("failed to update: {e}"));
                    }
                    return if compact {
                        ToolResult::text(format!("ok:{}", updated.id))
                    } else {
                        ToolResult::text(format!(
                            "Updated existing memory (similarity {score:.2}): {}",
                            updated.id
                        ))
                    };
                }
            }
        }
    }

    match store.store(memory) {
        Ok(id) => {
            if compact {
                // Try auto-consolidation even in compact mode
                let consolidation_msg =
                    try_auto_consolidate(store, topic, AUTO_CONSOLIDATE_THRESHOLD);
                if consolidation_msg.is_empty() {
                    ToolResult::text(format!("ok:{id}"))
                } else {
                    ToolResult::text(format!("ok:{id}\n{consolidation_msg}"))
                }
            } else {
                let consolidation_msg =
                    try_auto_consolidate(store, topic, AUTO_CONSOLIDATE_THRESHOLD);
                if consolidation_msg.is_empty() {
                    // Still show a nudge if approaching threshold
                    let hint = if let Ok(count) = store.count_by_topic(topic) {
                        if count > 7 {
                            format!(
                                "\nNote: Topic '{topic}' has {count} entries — consider consolidating with icm_memory_consolidate."
                            )
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };
                    ToolResult::text(format!("Stored memory: {id}{hint}"))
                } else {
                    ToolResult::text(format!("Stored memory: {id}\n{consolidation_msg}"))
                }
            }
        }
        Err(e) => ToolResult::error(format!("failed to store: {e}")),
    }
}

fn format_memory_output(memories: &[(Memory, f32)], compact: bool) -> String {
    let mut output = String::new();
    if compact {
        for (mem, _) in memories {
            output.push_str(&format!("[{}] {}\n", mem.topic, mem.summary));
        }
    } else {
        for (mem, score) in memories {
            if *score >= 0.0 {
                output.push_str(&format!(
                    "--- {} [score: {:.3}] ---\n  topic: {}\n  importance: {}\n  weight: {:.3}\n  summary: {}\n",
                    mem.id, score, mem.topic, mem.importance, mem.weight, mem.summary
                ));
            } else {
                output.push_str(&format!(
                    "--- {} ---\n  topic: {}\n  importance: {}\n  weight: {:.3}\n  summary: {}\n",
                    mem.id, mem.topic, mem.importance, mem.weight, mem.summary
                ));
            }
            if !mem.keywords.is_empty() {
                output.push_str(&format!("  keywords: {}\n", mem.keywords.join(", ")));
            }
            if let Some(ref raw) = mem.raw_excerpt {
                output.push_str(&format!("  raw: {raw}\n"));
            }
            output.push('\n');
        }
    }
    output
}

fn tool_recall(
    store: &SqliteStore,
    embedder: Option<&dyn Embedder>,
    args: &Value,
    compact: bool,
) -> ToolResult {
    // Auto-decay if >24h since last decay
    let _ = store.maybe_auto_decay();

    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    let limit = get_i64(args, "limit", 5).clamp(1, 100) as usize;
    let topic = get_str(args, "topic");
    let keyword = get_str(args, "keyword");

    // Try hybrid search if embedder is available
    if let Some(emb) = embedder {
        if let Ok(query_emb) = emb.embed(query) {
            if let Ok(results) = store.search_hybrid(query, &query_emb, limit) {
                let mut scored_results = results;
                if let Some(t) = topic {
                    scored_results
                        .retain(|(m, _)| m.topic == t || m.topic.starts_with(&format!("{t}:")));
                }
                if let Some(kw) = keyword {
                    scored_results.retain(|(m, _)| m.keywords.iter().any(|k| k.contains(kw)));
                }

                // Batch update access counts
                let ids: Vec<&str> = scored_results.iter().map(|(m, _)| m.id.as_str()).collect();
                let _ = store.batch_update_access(&ids);

                if scored_results.is_empty() {
                    return ToolResult::text("No memories found.".into());
                }

                return ToolResult::text(format_memory_output(&scored_results, compact));
            }
        }
    }

    // Fallback: FTS then keywords
    let mut results = match store.search_fts(query, limit) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("search error: {e}")),
    };

    if results.is_empty() {
        let keywords: Vec<&str> = query.split_whitespace().collect();
        results = match store.search_by_keywords(&keywords, limit) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("search error: {e}")),
        };
    }

    if let Some(t) = topic {
        results.retain(|m| m.topic == t || m.topic.starts_with(&format!("{t}:")));
    }
    if let Some(kw) = keyword {
        results.retain(|m| m.keywords.iter().any(|k| k.contains(kw)));
    }

    // Batch update access counts
    let ids: Vec<&str> = results.iter().map(|m| m.id.as_str()).collect();
    let _ = store.batch_update_access(&ids);

    if results.is_empty() {
        return ToolResult::text("No memories found.".into());
    }

    // Convert to scored format (no score = -1.0)
    let scored: Vec<(Memory, f32)> = results.into_iter().map(|m| (m, -1.0)).collect();
    ToolResult::text(format_memory_output(&scored, compact))
}

fn tool_forget(store: &SqliteStore, args: &Value) -> ToolResult {
    let id = match get_str(args, "id") {
        Some(id) => id,
        None => return ToolResult::error("missing required field: id".into()),
    };

    match store.delete(id) {
        Ok(()) => ToolResult::text(format!("Deleted memory: {id}")),
        Err(e) => ToolResult::error(format!("failed to delete: {e}")),
    }
}

fn tool_consolidate(store: &SqliteStore, args: &Value) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t,
        None => return ToolResult::error("missing required field: topic".into()),
    };
    let summary = match get_str(args, "summary") {
        Some(s) => s,
        None => return ToolResult::error("missing required field: summary".into()),
    };

    let consolidated = Memory::new(topic.into(), summary.into(), icm_core::Importance::High);

    match store.consolidate_topic(topic, consolidated) {
        Ok(()) => ToolResult::text(format!("Consolidated topic: {topic}")),
        Err(e) => ToolResult::error(format!("failed to consolidate: {e}")),
    }
}

fn tool_list_topics(store: &SqliteStore) -> ToolResult {
    match store.list_topics() {
        Ok(topics) => {
            if topics.is_empty() {
                return ToolResult::text("No topics yet.".into());
            }

            // Group topics by scope prefix (before ':')
            let mut scoped: std::collections::BTreeMap<String, Vec<(String, usize)>> =
                std::collections::BTreeMap::new();
            let mut unscoped: Vec<(String, usize)> = Vec::new();

            for (topic, count) in &topics {
                if let Some((prefix, _rest)) = topic.split_once(':') {
                    scoped
                        .entry(prefix.to_string())
                        .or_default()
                        .push((topic.clone(), *count));
                } else {
                    unscoped.push((topic.clone(), *count));
                }
            }

            let mut output = String::from("Topics:\n");

            // Show unscoped topics first
            for (topic, count) in &unscoped {
                output.push_str(&format!("  {topic}: {count} memories\n"));
            }

            // Show scoped topics grouped by prefix
            for (prefix, sub_topics) in &scoped {
                let total: usize = sub_topics.iter().map(|(_, c)| c).sum();
                output.push_str(&format!("  [{prefix}] ({total} total):\n"));
                for (topic, count) in sub_topics {
                    output.push_str(&format!("    {topic}: {count} memories\n"));
                }
            }

            ToolResult::text(output)
        }
        Err(e) => ToolResult::error(format!("failed to list topics: {e}")),
    }
}

fn tool_stats(store: &SqliteStore) -> ToolResult {
    match store.stats() {
        Ok(stats) => {
            let mut output = format!(
                "Memories: {}\nTopics: {}\nAvg weight: {:.3}\n",
                stats.total_memories, stats.total_topics, stats.avg_weight
            );
            if let Some(oldest) = stats.oldest_memory {
                output.push_str(&format!("Oldest: {}\n", oldest.format("%Y-%m-%d %H:%M")));
            }
            if let Some(newest) = stats.newest_memory {
                output.push_str(&format!("Newest: {}\n", newest.format("%Y-%m-%d %H:%M")));
            }
            ToolResult::text(output)
        }
        Err(e) => ToolResult::error(format!("failed to get stats: {e}")),
    }
}

fn tool_update(store: &SqliteStore, embedder: Option<&dyn Embedder>, args: &Value) -> ToolResult {
    let id = match get_str(args, "id") {
        Some(id) => id,
        None => return ToolResult::error("missing required field: id".into()),
    };
    let content = match get_str(args, "content") {
        Some(c) => c,
        None => return ToolResult::error("missing required field: content".into()),
    };

    let mut memory = match store.get(id) {
        Ok(Some(m)) => m,
        Ok(None) => return ToolResult::error(format!("memory not found: {id}")),
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    memory.summary = content.to_string();
    memory.updated_at = Utc::now();
    memory.weight = 1.0; // Reset weight on update (refreshed content)

    if let Some(imp_str) = get_str(args, "importance") {
        if let Ok(imp) = imp_str.parse() {
            memory.importance = imp;
        }
    }

    if let Some(keywords_arr) = args.get("keywords").and_then(|v| v.as_array()) {
        memory.keywords = keywords_arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    // Re-embed if embedder available
    if let Some(emb) = embedder {
        let text = format!("{} {}", memory.topic, content);
        if let Ok(vec) = emb.embed(&text) {
            memory.embedding = Some(vec);
        }
    }

    match store.update(&memory) {
        Ok(()) => ToolResult::text(format!("Updated memory: {id}")),
        Err(e) => ToolResult::error(format!("failed to update: {e}")),
    }
}

fn tool_health(store: &SqliteStore, args: &Value) -> ToolResult {
    let specific_topic = get_str(args, "topic");

    let topics = if let Some(t) = specific_topic {
        vec![(t.to_string(), 0usize)]
    } else {
        match store.list_topics() {
            Ok(t) => t,
            Err(e) => return ToolResult::error(format!("failed to list topics: {e}")),
        }
    };

    if topics.is_empty() {
        return ToolResult::text("No topics yet.".into());
    }

    let mut output = String::from("Memory Health Report:\n\n");
    let mut total_stale = 0usize;
    let mut topics_needing_consolidation = 0usize;

    for (topic, _) in &topics {
        match store.topic_health(topic) {
            Ok(health) => {
                let status = health.status();

                output.push_str(&format!(
                    "  {topic}: {status}\n    entries: {}  avg_weight: {:.2}  stale: {}  avg_access: {:.1}\n",
                    health.entry_count, health.avg_weight, health.stale_count, health.avg_access_count
                ));

                if health.needs_consolidation {
                    topics_needing_consolidation += 1;
                }
                total_stale += health.stale_count;
            }
            Err(_) => {
                output.push_str(&format!("  {topic}: (error reading)\n"));
            }
        }
    }

    output.push_str(&format!(
        "\nSummary: {} topics, {} need consolidation, {} stale entries total\n",
        topics.len(),
        topics_needing_consolidation,
        total_stale
    ));

    ToolResult::text(output)
}

fn tool_extract_patterns(store: &SqliteStore, args: &Value) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t,
        None => return ToolResult::error("missing required field: topic".into()),
    };
    let min_cluster_size = get_i64(args, "min_cluster_size", 3).clamp(2, 50) as usize;
    let memoir_name = get_str(args, "memoir");

    let patterns = match store.detect_patterns(topic, min_cluster_size) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("pattern detection failed: {e}")),
    };

    if patterns.is_empty() {
        return ToolResult::text(format!(
            "No patterns detected in topic '{topic}' (min cluster size: {min_cluster_size})."
        ));
    }

    let mut output = format!(
        "Detected {} pattern(s) in topic '{topic}':\n\n",
        patterns.len()
    );

    // If memoir is provided, resolve it and create concepts
    let memoir_id = if let Some(mname) = memoir_name {
        match resolve_memoir(store, mname) {
            Ok(m) => Some(m.id),
            Err(e) => return e,
        }
    } else {
        None
    };

    for (i, cluster) in patterns.iter().enumerate() {
        output.push_str(&format!(
            "Pattern {}: {} memories\n  Keywords: {}\n  Representative: {}\n",
            i + 1,
            cluster.count,
            cluster.keywords.join(", "),
            cluster.representative_summary,
        ));

        if let Some(ref mid) = memoir_id {
            match store.extract_pattern_as_concept(cluster, mid) {
                Ok(concept_id) => {
                    output.push_str(&format!("  -> Created concept: {concept_id}\n"));
                }
                Err(e) => {
                    output.push_str(&format!("  -> Failed to create concept: {e}\n"));
                }
            }
        }

        output.push('\n');
    }

    if memoir_id.is_some() {
        output.push_str(&format!(
            "Created {} concept(s) in memoir '{}'.\n",
            patterns.len(),
            memoir_name.unwrap_or("?")
        ));
    }

    ToolResult::text(output)
}

fn tool_embed_all(
    store: &SqliteStore,
    embedder: Option<&dyn Embedder>,
    args: &Value,
) -> ToolResult {
    let embedder = match embedder {
        Some(e) => e,
        None => return ToolResult::error("embeddings not available".into()),
    };

    let topic_filter = get_str(args, "topic");

    // Get all memories in a single query
    let memories = if let Some(t) = topic_filter {
        match store.get_by_topic(t) {
            Ok(m) => m,
            Err(e) => return ToolResult::error(format!("failed to list memories: {e}")),
        }
    } else {
        match store.list_all() {
            Ok(m) => m,
            Err(e) => return ToolResult::error(format!("failed to list memories: {e}")),
        }
    };

    // Filter to only those without embeddings
    let to_embed: Vec<&Memory> = memories.iter().filter(|m| m.embedding.is_none()).collect();

    if to_embed.is_empty() {
        return ToolResult::text("All memories already have embeddings.".into());
    }

    let total = to_embed.len();

    // Batch embed all texts at once
    let texts: Vec<String> = to_embed.iter().map(|m| m.embed_text()).collect();
    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

    let embeddings = match embedder.embed_batch(&text_refs) {
        Ok(vecs) => vecs,
        Err(e) => return ToolResult::error(format!("batch embedding failed: {e}")),
    };

    let mut embedded = 0;
    let mut errors = 0;

    for (mem, vec) in to_embed.iter().zip(embeddings) {
        let mut updated = (*mem).clone();
        updated.embedding = Some(vec);
        if store.update(&updated).is_ok() {
            embedded += 1;
        } else {
            errors += 1;
        }
    }

    ToolResult::text(format!(
        "Embedded {embedded}/{total} memories ({errors} errors)"
    ))
}

// ---------------------------------------------------------------------------
// Memoir tool handlers
// ---------------------------------------------------------------------------

fn tool_memoir_create(store: &SqliteStore, args: &Value) -> ToolResult {
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    let description = get_str(args, "description").unwrap_or("");

    let memoir = Memoir::new(name.into(), description.into());
    match store.create_memoir(memoir) {
        Ok(id) => ToolResult::text(format!("Created memoir '{name}': {id}")),
        Err(e) => ToolResult::error(format!("failed to create memoir: {e}")),
    }
}

fn tool_memoir_list(store: &SqliteStore) -> ToolResult {
    let memoirs = match store.list_memoirs() {
        Ok(m) => m,
        Err(e) => return ToolResult::error(format!("failed to list memoirs: {e}")),
    };

    if memoirs.is_empty() {
        return ToolResult::text("No memoirs yet.".into());
    }

    let counts = store.batch_memoir_concept_counts().unwrap_or_default();
    let mut output = String::from("Memoirs:\n");
    for m in &memoirs {
        let concept_count = counts.get(&m.id).copied().unwrap_or(0);
        output.push_str(&format!(
            "  {} ({} concepts) — {}\n",
            m.name, concept_count, m.description
        ));
    }
    ToolResult::text(output)
}

fn tool_memoir_show(store: &SqliteStore, args: &Value) -> ToolResult {
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };

    let memoir = match resolve_memoir(store, name) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let stats = match store.memoir_stats(&memoir.id) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("failed to get stats: {e}")),
    };
    let concepts = match store.list_concepts(&memoir.id) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("failed to list concepts: {e}")),
    };

    let mut output = format!(
        "Memoir: {}\nDescription: {}\nConcepts: {}\nLinks: {}\nAvg confidence: {:.2}\n",
        memoir.name,
        memoir.description,
        stats.total_concepts,
        stats.total_links,
        stats.avg_confidence
    );

    if !stats.label_counts.is_empty() {
        output.push_str("Labels:\n");
        for (label, count) in &stats.label_counts {
            output.push_str(&format!("  {label} ({count})\n"));
        }
    }

    if !concepts.is_empty() {
        output.push_str("\nConcepts:\n");
        for c in &concepts {
            let labels_str = c.format_labels();
            output.push_str(&format!(
                "  {} [r{} c{:.2}]{}\n    {}\n",
                c.name,
                c.revision,
                c.confidence,
                if labels_str.is_empty() {
                    String::new()
                } else {
                    format!(" ({labels_str})")
                },
                c.definition
            ));
        }
    }

    ToolResult::text(output)
}

fn tool_memoir_add_concept(store: &SqliteStore, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "memoir") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: memoir".into()),
    };
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    let definition = match get_str(args, "definition") {
        Some(d) => d,
        None => return ToolResult::error("missing required field: definition".into()),
    };

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let mut concept = Concept::new(memoir.id, name.into(), definition.into());

    if let Some(labels_str) = get_str(args, "labels") {
        concept.labels = labels_str
            .split(',')
            .filter_map(|s| s.trim().parse::<Label>().ok())
            .collect();
    }

    match store.add_concept(concept) {
        Ok(id) => ToolResult::text(format!(
            "Added concept '{name}' to memoir '{memoir_name}': {id}"
        )),
        Err(e) => ToolResult::error(format!("failed to add concept: {e}")),
    }
}

fn tool_memoir_refine(store: &SqliteStore, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "memoir") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: memoir".into()),
    };
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    let definition = match get_str(args, "definition") {
        Some(d) => d,
        None => return ToolResult::error("missing required field: definition".into()),
    };

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let concept = match store.get_concept_by_name(&memoir.id, name) {
        Ok(Some(c)) => c,
        Ok(None) => return ToolResult::error(format!("concept not found: {name}")),
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    if let Err(e) = store.refine_concept(&concept.id, definition, &[]) {
        return ToolResult::error(format!("failed to refine: {e}"));
    }

    let updated = match store.get_concept(&concept.id) {
        Ok(Some(c)) => c,
        _ => return ToolResult::text(format!("Refined concept '{name}'")),
    };

    ToolResult::text(format!(
        "Refined '{name}' (r{}, confidence={:.2})",
        updated.revision, updated.confidence
    ))
}

fn tool_memoir_search(store: &SqliteStore, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "memoir") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: memoir".into()),
    };
    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    let limit = get_i64(args, "limit", 10).clamp(1, 100) as usize;
    let label_str = get_str(args, "label");

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let results = if let Some(lbl) = label_str {
        let parsed: Label = match lbl.parse() {
            Ok(l) => l,
            Err(e) => return ToolResult::error(format!("invalid label: {e}")),
        };
        let mut by_label = match store.search_concepts_by_label(&memoir.id, &parsed, limit) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("search error: {e}")),
        };
        if !query.is_empty() {
            let q = query.to_lowercase();
            by_label.retain(|c| {
                c.name.to_lowercase().contains(&q) || c.definition.to_lowercase().contains(&q)
            });
        }
        by_label
    } else {
        match store.search_concepts_fts(&memoir.id, query, limit) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("search error: {e}")),
        }
    };

    if results.is_empty() {
        return ToolResult::text("No concepts found.".into());
    }

    let mut output = String::new();
    for c in &results {
        let labels_str = c.format_labels();
        output.push_str(&format!(
            "--- {} [r{} c{:.2}] ---\n  {}\n",
            c.name, c.revision, c.confidence, c.definition
        ));
        if !labels_str.is_empty() {
            output.push_str(&format!("  labels: {labels_str}\n"));
        }
        output.push('\n');
    }

    ToolResult::text(output)
}

fn tool_memoir_search_all(store: &SqliteStore, args: &Value) -> ToolResult {
    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    let limit = get_i64(args, "limit", 10).clamp(1, 100) as usize;

    let results = match store.search_all_concepts_fts(query, limit) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("search error: {e}")),
    };

    if results.is_empty() {
        return ToolResult::text("No concepts found.".into());
    }

    // Group by memoir for readable output
    let memoirs: std::collections::HashMap<String, String> = store
        .list_memoirs()
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.id.clone(), m.name))
        .collect();

    let mut output = String::new();
    for c in &results {
        let memoir_name = memoirs.get(&c.memoir_id).map(|s| s.as_str()).unwrap_or("?");
        let labels_str = c.format_labels();
        output.push_str(&format!(
            "--- {} ({}) [r{} c{:.2}] ---\n  {}\n",
            c.name, memoir_name, c.revision, c.confidence, c.definition
        ));
        if !labels_str.is_empty() {
            output.push_str(&format!("  labels: {labels_str}\n"));
        }
        output.push('\n');
    }

    ToolResult::text(output)
}

fn tool_memoir_link(store: &SqliteStore, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "memoir") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: memoir".into()),
    };
    let from_name = match get_str(args, "from") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: from".into()),
    };
    let to_name = match get_str(args, "to") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: to".into()),
    };
    let relation_str = match get_str(args, "relation") {
        Some(r) => r,
        None => return ToolResult::error("missing required field: relation".into()),
    };

    let relation: Relation = match relation_str.parse() {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("invalid relation: {e}")),
    };

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let from = match store.get_concept_by_name(&memoir.id, from_name) {
        Ok(Some(c)) => c,
        Ok(None) => return ToolResult::error(format!("concept not found: {from_name}")),
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };
    let to = match store.get_concept_by_name(&memoir.id, to_name) {
        Ok(Some(c)) => c,
        Ok(None) => return ToolResult::error(format!("concept not found: {to_name}")),
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    let link = ConceptLink::new(from.id, to.id, relation);
    match store.add_link(link) {
        Ok(id) => ToolResult::text(format!(
            "Linked: {from_name} --{relation}--> {to_name} ({id})"
        )),
        Err(e) => ToolResult::error(format!("failed to link: {e}")),
    }
}

fn tool_memoir_inspect(store: &SqliteStore, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "memoir") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: memoir".into()),
    };
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    let depth = get_i64(args, "depth", 1).clamp(1, 3) as usize;

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let concept = match store.get_concept_by_name(&memoir.id, name) {
        Ok(Some(c)) => c,
        Ok(None) => return ToolResult::error(format!("concept not found: {name}")),
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    let labels_str = concept.format_labels();

    let mut output = format!(
        "Concept: {}\n  id: {}\n  definition: {}\n  confidence: {:.2}\n  revision: {}\n",
        concept.name, concept.id, concept.definition, concept.confidence, concept.revision
    );
    if !labels_str.is_empty() {
        output.push_str(&format!("  labels: {labels_str}\n"));
    }

    let (neighbors, links) = match store.get_neighborhood(&concept.id, depth) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("graph error: {e}")),
    };

    if links.is_empty() {
        output.push_str("\n(no links)\n");
    } else {
        let name_map: std::collections::HashMap<&str, &str> = neighbors
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();
        output.push_str(&format!("\nGraph (depth={depth}):\n"));
        for link in &links {
            let src = name_map.get(link.source_id.as_str()).unwrap_or(&"?");
            let tgt = name_map.get(link.target_id.as_str()).unwrap_or(&"?");
            output.push_str(&format!("  {src} --{}--> {tgt}\n", link.relation));
        }
    }

    ToolResult::text(output)
}

// confidence_color and confidence_bar are now methods on Concept in icm-core

fn tool_memoir_export(store: &SqliteStore, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    let format = get_str(args, "format").unwrap_or("json");

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let concepts = match store.list_concepts(&memoir.id) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    // Batch load all links for this memoir (single query)
    let links = match store.get_links_for_memoir(&memoir.id) {
        Ok(l) => l,
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    let id_to_name: std::collections::HashMap<&str, &str> = concepts
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();

    match format {
        "json" => {
            let json_concepts: Vec<serde_json::Value> = concepts
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "name": c.name,
                        "definition": c.definition,
                        "labels": c.labels.iter().map(|l| l.to_string()).collect::<Vec<_>>(),
                        "confidence": c.confidence,
                        "revision": c.revision,
                    })
                })
                .collect();

            let json_links: Vec<serde_json::Value> = links
                .iter()
                .filter_map(|l| {
                    let src = id_to_name.get(l.source_id.as_str())?;
                    let tgt = id_to_name.get(l.target_id.as_str())?;
                    Some(serde_json::json!({
                        "source": src,
                        "target": tgt,
                        "relation": l.relation.to_string(),
                        "weight": l.weight,
                    }))
                })
                .collect();

            let output = serde_json::json!({
                "memoir": { "name": memoir.name, "description": memoir.description },
                "concepts": json_concepts,
                "links": json_links,
            });

            ToolResult::text(
                serde_json::to_string_pretty(&output)
                    .unwrap_or_else(|e| format!("json error: {e}")),
            )
        }
        "dot" => {
            let mut out = format!(
                "digraph \"{}\" {{\n  rankdir=LR;\n  node [shape=box, style=\"rounded,filled\", fillcolor=white];\n\n",
                memoir.name
            );
            for c in &concepts {
                let escaped = c.definition.replace('"', "\\\"");
                let color = c.confidence_color();
                out.push_str(&format!(
                    "  \"{}\" [tooltip=\"{}\" fillcolor=\"{}\" label=\"{}\\n({:.0}%)\"];\n",
                    c.name,
                    escaped,
                    color,
                    c.name,
                    c.confidence * 100.0
                ));
            }
            out.push('\n');
            for l in &links {
                if let (Some(src), Some(tgt)) = (
                    id_to_name.get(l.source_id.as_str()),
                    id_to_name.get(l.target_id.as_str()),
                ) {
                    let pw = 0.5 + l.weight * 2.0;
                    out.push_str(&format!(
                        "  \"{}\" -> \"{}\" [label=\"{}\" penwidth={:.1}];\n",
                        src, tgt, l.relation, pw
                    ));
                }
            }
            out.push_str("}\n");
            ToolResult::text(out)
        }
        "ascii" => {
            let mut out = format!("╔══ {} ══╗\n", memoir.name);
            if !memoir.description.is_empty() {
                out.push_str(&format!("║ {}\n", memoir.description));
            }
            out.push_str(&format!(
                "║ {} concepts, {} links\n",
                concepts.len(),
                links.len()
            ));
            out.push_str(&format!("╚{}╝\n\n", "═".repeat(memoir.name.len() + 6)));

            let mut outgoing: std::collections::HashMap<&str, Vec<(String, &str)>> =
                std::collections::HashMap::new();
            let mut incoming: std::collections::HashMap<&str, Vec<(String, &str)>> =
                std::collections::HashMap::new();
            for l in &links {
                if let (Some(&src), Some(&tgt)) = (
                    id_to_name.get(l.source_id.as_str()),
                    id_to_name.get(l.target_id.as_str()),
                ) {
                    outgoing
                        .entry(src)
                        .or_default()
                        .push((l.relation.to_string(), tgt));
                    incoming
                        .entry(tgt)
                        .or_default()
                        .push((l.relation.to_string(), src));
                }
            }

            for c in &concepts {
                let labels_str = if c.labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", c.format_labels())
                };
                out.push_str(&format!(
                    "┌─ {}{} {}\n",
                    c.name,
                    labels_str,
                    c.confidence_bar()
                ));
                out.push_str(&format!("│  {}\n", c.definition));
                if let Some(outs) = outgoing.get(c.name.as_str()) {
                    for (rel, tgt) in outs {
                        out.push_str(&format!("│  ──{}──> {}\n", rel, tgt));
                    }
                }
                if let Some(ins) = incoming.get(c.name.as_str()) {
                    for (rel, src) in ins {
                        out.push_str(&format!("│  <──{}── {}\n", rel, src));
                    }
                }
                out.push_str("└─\n");
            }
            ToolResult::text(out)
        }
        "ai" => {
            let mut out = format!("# Memoir: {} — {}\n\n", memoir.name, memoir.description);
            out.push_str(&format!("## Concepts ({})\n", concepts.len()));
            for c in &concepts {
                let labels_str = if c.labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", c.format_labels())
                };
                out.push_str(&format!(
                    "- **{}**{} (confidence: {:.0}%): {}\n",
                    c.name,
                    labels_str,
                    c.confidence * 100.0,
                    c.definition
                ));
            }
            if !links.is_empty() {
                out.push_str(&format!("\n## Relations ({})\n", links.len()));
                for l in &links {
                    if let (Some(src), Some(tgt)) = (
                        id_to_name.get(l.source_id.as_str()),
                        id_to_name.get(l.target_id.as_str()),
                    ) {
                        out.push_str(&format!(
                            "- {} ──{}──> {} (w:{:.1})\n",
                            src, l.relation, tgt, l.weight
                        ));
                    }
                }
            }
            ToolResult::text(out)
        }
        _ => ToolResult::error(format!(
            "unsupported format: {format} (use 'json', 'dot', 'ascii', or 'ai')"
        )),
    }
}

fn tool_feedback_record(store: &SqliteStore, args: &Value, compact: bool) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t,
        None => return ToolResult::error("missing required field: topic".into()),
    };
    let context = match get_str(args, "context") {
        Some(c) => c,
        None => return ToolResult::error("missing required field: context".into()),
    };
    let predicted = match get_str(args, "predicted") {
        Some(p) => p,
        None => return ToolResult::error("missing required field: predicted".into()),
    };
    let corrected = match get_str(args, "corrected") {
        Some(c) => c,
        None => return ToolResult::error("missing required field: corrected".into()),
    };
    let reason = get_str(args, "reason").map(|s| s.to_string());
    let source = get_str(args, "source").unwrap_or("").to_string();

    let feedback = Feedback::new(
        topic.into(),
        context.into(),
        predicted.into(),
        corrected.into(),
        reason,
        source,
    );

    let id = feedback.id.clone();
    match store.store_feedback(feedback) {
        Ok(_) => {
            if compact {
                ToolResult::text(format!("ok {id}"))
            } else {
                ToolResult::text(format!("Feedback recorded: {id}\n  topic: {topic}\n  predicted: {predicted}\n  corrected: {corrected}"))
            }
        }
        Err(e) => ToolResult::error(format!("failed to store feedback: {e}")),
    }
}

fn tool_feedback_search(store: &SqliteStore, args: &Value) -> ToolResult {
    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    let topic = get_str(args, "topic");
    let limit = get_i64(args, "limit", 5).clamp(1, 100) as usize;

    match store.search_feedback(query, topic, limit) {
        Ok(results) => {
            if results.is_empty() {
                return ToolResult::text("No feedback found.".into());
            }
            let mut output = String::new();
            for fb in &results {
                output.push_str(&format!(
                    "--- {} [{}] ---\n  context: {}\n  predicted: {}\n  corrected: {}\n",
                    fb.id, fb.topic, fb.context, fb.predicted, fb.corrected
                ));
                if let Some(ref reason) = fb.reason {
                    output.push_str(&format!("  reason: {reason}\n"));
                }
                if !fb.source.is_empty() {
                    output.push_str(&format!("  source: {}\n", fb.source));
                }
                if fb.applied_count > 0 {
                    output.push_str(&format!("  applied: {} times\n", fb.applied_count));
                }
            }
            ToolResult::text(output)
        }
        Err(e) => ToolResult::error(format!("failed to search feedback: {e}")),
    }
}

fn tool_feedback_stats(store: &SqliteStore) -> ToolResult {
    match store.feedback_stats() {
        Ok(stats) => {
            let mut output = format!("Feedback total: {}\n", stats.total);
            if !stats.by_topic.is_empty() {
                output.push_str("\nBy topic:\n");
                for (topic, count) in &stats.by_topic {
                    output.push_str(&format!("  {topic}: {count}\n"));
                }
            }
            if !stats.most_applied.is_empty() {
                output.push_str("\nMost applied:\n");
                for (id, count) in &stats.most_applied {
                    output.push_str(&format!("  {id}: {count} times\n"));
                }
            }
            ToolResult::text(output)
        }
        Err(e) => ToolResult::error(format!("failed to get feedback stats: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SqliteStore {
        SqliteStore::in_memory().unwrap()
    }

    #[test]
    fn test_unknown_tool_returns_error() {
        let store = test_store();
        let result = call_tool(&store, None, "nonexistent_tool", &json!({}), false);
        assert!(result.is_error);
        assert!(result.content[0].text.contains("unknown tool"));
    }

    #[test]
    fn test_store_missing_topic() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"content": "hello"}),
            false,
        );
        assert!(result.is_error);
        assert!(result.content[0].text.contains("topic"));
    }

    #[test]
    fn test_store_missing_content() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "test"}),
            false,
        );
        assert!(result.is_error);
        assert!(result.content[0].text.contains("content"));
    }

    #[test]
    fn test_recall_missing_query() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_memory_recall", &json!({}), false);
        assert!(result.is_error);
        assert!(result.content[0].text.contains("query"));
    }

    #[test]
    fn test_recall_empty_store() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "anything"}),
            false,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("No memories"));
    }

    #[test]
    fn test_forget_missing_id() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_memory_forget", &json!({}), false);
        assert!(result.is_error);
        assert!(result.content[0].text.contains("id"));
    }

    #[test]
    fn test_forget_nonexistent_id() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_forget",
            &json!({"id": "does-not-exist"}),
            false,
        );
        assert!(result.is_error);
    }

    #[test]
    fn test_store_and_recall_roundtrip() {
        let store = test_store();
        let store_result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "test-project", "content": "Uses Rust and SQLite"}),
            false,
        );
        assert!(!store_result.is_error);
        assert!(store_result.content[0].text.contains("Stored memory"));

        let recall_result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "Rust SQLite"}),
            false,
        );
        assert!(!recall_result.is_error);
        assert!(recall_result.content[0].text.contains("Rust"));
    }

    #[test]
    fn test_compact_store_output() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "t", "content": "c"}),
            true,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.starts_with("ok:"));
    }

    #[test]
    fn test_compact_recall_output() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "proj", "content": "Rust memory system"}),
            false,
        );
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "Rust memory"}),
            true,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("[proj]"));
    }

    #[test]
    fn test_stats_empty() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("Memories: 0"));
    }

    #[test]
    fn test_list_topics_empty() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_memory_list_topics", &json!({}), false);
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("No topics"));
    }

    #[test]
    fn test_health_empty() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_memory_health", &json!({}), false);
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("No topics"));
    }

    #[test]
    fn test_update_missing_fields() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_update",
            &json!({"id": "x"}),
            false,
        );
        assert!(result.is_error);
        assert!(result.content[0].text.contains("content"));
    }

    #[test]
    fn test_update_nonexistent() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_update",
            &json!({"id": "fake", "content": "new"}),
            false,
        );
        assert!(result.is_error);
        assert!(result.content[0].text.contains("not found"));
    }

    #[test]
    fn test_store_sql_injection_topic() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "'; DROP TABLE memories;--", "content": "pwned"}),
            false,
        );
        assert!(!result.is_error);
        let stats = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(stats.content[0].text.contains("Memories: 1"));
    }

    #[test]
    fn test_recall_injection_query() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "safe", "content": "normal data"}),
            false,
        );
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "') OR 1=1 --"}),
            false,
        );
        assert!(!result.is_error);
    }

    #[test]
    fn test_store_xss_in_content() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "xss",
                "content": "<script>alert('xss')</script>"
            }),
            false,
        );
        assert!(!result.is_error);
        let recall = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "script alert"}),
            false,
        );
        assert!(recall.content[0].text.contains("<script>"));
    }

    #[test]
    fn test_store_very_large_content() {
        let store = test_store();
        let huge = "x".repeat(500_000);
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "big", "content": huge}),
            false,
        );
        assert!(!result.is_error);
    }

    #[test]
    fn test_memoir_create_injection() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memoir_create",
            &json!({"name": "'; DROP TABLE memoirs;--", "description": "test"}),
            false,
        );
        assert!(!result.is_error);
        let list = call_tool(&store, None, "icm_memoir_list", &json!({}), false);
        assert!(!list.is_error);
        assert!(list.content[0].text.contains("DROP TABLE"));
    }

    #[test]
    fn test_store_many_via_mcp() {
        let store = test_store();
        // Use different topics to avoid auto-consolidation (threshold=10)
        for i in 0..50 {
            let topic = format!("perf-{}", i / 9); // max 9 per topic, under threshold
            let result = call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": topic, "content": format!("item {i}")}),
                true,
            );
            assert!(!result.is_error);
        }
        let stats = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(stats.content[0].text.contains("Memories: 50"));
    }

    #[test]
    fn test_recall_with_topic_filter() {
        let store = test_store();
        for topic in &["alpha", "beta", "gamma"] {
            call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": topic, "content": format!("data for {topic}")}),
                false,
            );
        }
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "data", "topic": "beta"}),
            false,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("beta"));
        assert!(!result.content[0].text.contains("alpha"));
    }

    #[test]
    fn test_consolidate_via_mcp() {
        let store = test_store();
        for i in 0..10 {
            call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": "consolidate-me", "content": format!("detail {i}")}),
                false,
            );
        }
        let result = call_tool(
            &store,
            None,
            "icm_memory_consolidate",
            &json!({"topic": "consolidate-me", "summary": "All 10 details merged"}),
            false,
        );
        assert!(!result.is_error);
        let stats = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(stats.content[0].text.contains("Memories: 1"));
    }

    // === Security tests ===

    #[test]
    fn test_path_traversal_in_topic() {
        let store = test_store();
        let malicious_topics = [
            "../../../etc/passwd",
            "..\\..\\windows\\system32",
            "/etc/shadow",
            "topic/../../secret",
            "....//....//etc/passwd",
        ];
        for topic in &malicious_topics {
            let result = call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": topic, "content": "path traversal attempt"}),
                false,
            );
            // Should either store safely (topic is just a string label) or reject
            // but must NOT crash or access filesystem
            assert!(!result.content.is_empty());
        }
        let stats = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(!stats.is_error);
    }

    #[test]
    fn test_extremely_long_content_over_1mb() {
        let store = test_store();
        let huge_content = "A".repeat(1_100_000); // ~1.1MB
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "huge", "content": huge_content}),
            false,
        );
        // Should either store or reject gracefully, never panic
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_null_bytes_in_topic() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "before\0after", "content": "null byte topic"}),
            false,
        );
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_null_bytes_in_content() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "test", "content": "start\0middle\0end"}),
            false,
        );
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_null_bytes_in_query() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "safe", "content": "normal data"}),
            false,
        );
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "normal\0injected"}),
            false,
        );
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_unicode_rtl_and_zero_width_chars() {
        let store = test_store();
        // Right-to-left override, zero-width joiners, bidi markers
        let tricky_strings = [
            "\u{202E}reversed\u{202C}",                   // RTL override
            "normal\u{200B}zero\u{200B}width",            // zero-width space
            "\u{FEFF}bom_prefix",                         // BOM
            "a\u{0300}\u{0301}\u{0302}\u{0303}combining", // stacked combining marks
            "\u{200D}\u{200D}\u{200D}",                   // zero-width joiners only
        ];
        for s in &tricky_strings {
            let result = call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": s, "content": format!("content with {s}")}),
                false,
            );
            assert!(!result.is_error, "Failed on unicode string: {:?}", s);
        }
        let stats = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(!stats.is_error);
    }

    #[test]
    fn test_json_injection_in_params() {
        let store = test_store();
        // Attempt to inject extra JSON fields
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "test",
                "content": "legit",
                "__proto__": {"admin": true},
                "constructor": {"prototype": {"isAdmin": true}},
                "extra_unknown_field": "should be ignored"
            }),
            false,
        );
        // Should store normally, ignoring unknown fields
        assert!(!result.is_error);
        let recall = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "legit"}),
            false,
        );
        assert!(!recall.is_error);
        assert!(recall.content[0].text.contains("legit"));
    }

    #[test]
    fn test_empty_topic_field() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "", "content": "empty topic"}),
            false,
        );
        // Should either reject or store; must not panic
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_whitespace_only_fields() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "   \t\n  ", "content": "   \n\t  "}),
            false,
        );
        // Should either reject or store; must not panic
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_whitespace_only_recall_query() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "   \t\n  "}),
            false,
        );
        // Should return empty or error, not crash
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_memoir_create_path_traversal_name() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memoir_create",
            &json!({"name": "../../../etc/passwd", "description": "traversal"}),
            false,
        );
        // Should store as a label, not access filesystem
        assert!(!result.content.is_empty());
        if !result.is_error {
            let list = call_tool(&store, None, "icm_memoir_list", &json!({}), false);
            assert!(!list.is_error);
        }
    }

    // === Feedback tool tests ===

    #[test]
    fn test_feedback_record_missing_fields() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({"topic": "test"}),
            false,
        );
        assert!(result.is_error);
        assert!(result.content[0].text.contains("context"));
    }

    #[test]
    fn test_feedback_record_and_search_roundtrip() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({
                "topic": "triage",
                "context": "issue about memory leak in connection pool",
                "predicted": "low priority",
                "corrected": "high priority",
                "reason": "memory leaks are always high priority"
            }),
            false,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("Feedback recorded"));

        let search = call_tool(
            &store,
            None,
            "icm_feedback_search",
            &json!({"query": "memory leak"}),
            false,
        );
        assert!(!search.is_error);
        assert!(search.content[0].text.contains("memory leak"));
        assert!(search.content[0].text.contains("high priority"));
    }

    #[test]
    fn test_feedback_record_compact_mode() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({
                "topic": "test",
                "context": "ctx",
                "predicted": "a",
                "corrected": "b"
            }),
            true,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.starts_with("ok "));
    }

    #[test]
    fn test_feedback_search_missing_query() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_feedback_search", &json!({}), false);
        assert!(result.is_error);
        assert!(result.content[0].text.contains("query"));
    }

    #[test]
    fn test_feedback_search_empty_results() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_feedback_search",
            &json!({"query": "nonexistent"}),
            false,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("No feedback found"));
    }

    #[test]
    fn test_feedback_stats_empty() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_feedback_stats", &json!({}), false);
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("Feedback total: 0"));
    }

    #[test]
    fn test_feedback_stats_with_data() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({
                "topic": "triage",
                "context": "ctx1",
                "predicted": "a",
                "corrected": "b"
            }),
            false,
        );
        call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({
                "topic": "pr-review",
                "context": "ctx2",
                "predicted": "c",
                "corrected": "d"
            }),
            false,
        );

        let result = call_tool(&store, None, "icm_feedback_stats", &json!({}), false);
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("Feedback total: 2"));
        assert!(result.content[0].text.contains("triage"));
        assert!(result.content[0].text.contains("pr-review"));
    }
}
