use serde_json::{json, Value};

use icm_core::{Concept, ConceptLink, Label, Memoir, MemoirStore, Memory, MemoryStore, Relation};
use icm_store::SqliteStore;

use crate::protocol::ToolResult;

// ---------------------------------------------------------------------------
// Tool schemas for tools/list
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            // --- Memory tools ---
            {
                "name": "icm_store",
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
            },
            {
                "name": "icm_recall",
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
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "icm_forget",
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
            },
            {
                "name": "icm_consolidate",
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
            },
            {
                "name": "icm_list_topics",
                "description": "List all available topics in memory with their counts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "icm_stats",
                "description": "Get global ICM memory statistics.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            // --- Memoir tools ---
            {
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
            },
            {
                "name": "icm_memoir_list",
                "description": "List all memoirs with their concept counts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
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
            },
            {
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
            },
            {
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
            },
            {
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
                        "limit": {
                            "type": "integer",
                            "default": 10,
                            "description": "Max results"
                        }
                    },
                    "required": ["memoir", "query"]
                }
            },
            {
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
                            "enum": ["part_of", "depends_on", "related_to", "contradicts", "refines", "alternative_to", "caused_by", "instance_of"],
                            "description": "Relation type"
                        }
                    },
                    "required": ["memoir", "from", "to", "relation"]
                }
            },
            {
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
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

pub fn call_tool(store: &SqliteStore, name: &str, args: &Value) -> ToolResult {
    match name {
        // Memory tools
        "icm_store" => tool_store(store, args),
        "icm_recall" => tool_recall(store, args),
        "icm_forget" => tool_forget(store, args),
        "icm_consolidate" => tool_consolidate(store, args),
        "icm_list_topics" => tool_list_topics(store),
        "icm_stats" => tool_stats(store),
        // Memoir tools
        "icm_memoir_create" => tool_memoir_create(store, args),
        "icm_memoir_list" => tool_memoir_list(store),
        "icm_memoir_show" => tool_memoir_show(store, args),
        "icm_memoir_add_concept" => tool_memoir_add_concept(store, args),
        "icm_memoir_refine" => tool_memoir_refine(store, args),
        "icm_memoir_search" => tool_memoir_search(store, args),
        "icm_memoir_link" => tool_memoir_link(store, args),
        "icm_memoir_inspect" => tool_memoir_inspect(store, args),
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

fn tool_store(store: &SqliteStore, args: &Value) -> ToolResult {
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

    match store.store(memory) {
        Ok(id) => ToolResult::text(format!("Stored memory: {id}")),
        Err(e) => ToolResult::error(format!("failed to store: {e}")),
    }
}

fn tool_recall(store: &SqliteStore, args: &Value) -> ToolResult {
    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    let limit = get_i64(args, "limit", 5) as usize;
    let topic = get_str(args, "topic");

    // Try FTS first, fallback to keywords
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
        results.retain(|m| m.topic == t);
    }

    // Update access counts
    for mem in &results {
        let _ = store.update_access(&mem.id);
    }

    if results.is_empty() {
        return ToolResult::text("No memories found.".into());
    }

    let mut output = String::new();
    for mem in &results {
        output.push_str(&format!(
            "--- {} ---\n  topic: {}\n  importance: {}\n  weight: {:.3}\n  summary: {}\n",
            mem.id, mem.topic, mem.importance, mem.weight, mem.summary
        ));
        if !mem.keywords.is_empty() {
            output.push_str(&format!("  keywords: {}\n", mem.keywords.join(", ")));
        }
        if let Some(ref raw) = mem.raw_excerpt {
            output.push_str(&format!("  raw: {raw}\n"));
        }
        output.push('\n');
    }

    ToolResult::text(output)
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
            let mut output = String::from("Topics:\n");
            for (topic, count) in &topics {
                output.push_str(&format!("  {topic}: {count} memories\n"));
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

    let mut output = String::from("Memoirs:\n");
    for m in &memoirs {
        let stats = store.memoir_stats(&m.id).ok();
        let concept_count = stats.map(|s| s.total_concepts).unwrap_or(0);
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
            let labels_str = c
                .labels
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(", ");
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
    let limit = get_i64(args, "limit", 10) as usize;

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let results = match store.search_concepts_fts(&memoir.id, query, limit) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("search error: {e}")),
    };

    if results.is_empty() {
        return ToolResult::text("No concepts found.".into());
    }

    let mut output = String::new();
    for c in &results {
        let labels_str = c
            .labels
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(", ");
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
    let depth = get_i64(args, "depth", 1) as usize;

    let memoir = match resolve_memoir(store, memoir_name) {
        Ok(m) => m,
        Err(e) => return e,
    };

    let concept = match store.get_concept_by_name(&memoir.id, name) {
        Ok(Some(c)) => c,
        Ok(None) => return ToolResult::error(format!("concept not found: {name}")),
        Err(e) => return ToolResult::error(format!("db error: {e}")),
    };

    let labels_str = concept
        .labels
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join(", ");

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
        output.push_str(&format!("\nGraph (depth={depth}):\n"));
        for link in &links {
            let src = neighbors
                .iter()
                .find(|c| c.id == link.source_id)
                .map(|c| c.name.as_str())
                .unwrap_or("?");
            let tgt = neighbors
                .iter()
                .find(|c| c.id == link.target_id)
                .map(|c| c.name.as_str())
                .unwrap_or("?");
            output.push_str(&format!("  {src} --{}--> {tgt}\n", link.relation));
        }
    }

    ToolResult::text(output)
}
