use chrono::Utc;
use serde_json::{Value, json};

use icm_core::{
    AutoLinkOptions, Concept, ConceptLink, DEDUP_SIMILARITY_THRESHOLD, Embedder, Feedback,
    FeedbackStore, Label, MSG_NO_MEMORIES, Memoir, MemoirStore, Memory, MemoryStore, Relation,
    WakeUpFormat, WakeUpOptions, add_backrefs, auto_link_memory, build_wake_up,
    find_similar_memory, format_local, is_preference_topic, keyword_matches, project_matches,
    topic_matches,
};
use icm_store::{RecallEngine, RecallRequest, Store};

use crate::protocol::ToolResult;

/// Historical default threshold for auto-consolidation. The live value comes
/// from [`AutoConsolidate`] (issue #318); this constant is only the fallback
/// for callers that don't pass a policy.
const AUTO_CONSOLIDATE_THRESHOLD: usize = 10;

/// Auto-consolidation policy for the MCP store path (issue #318).
///
/// Previously the MCP `icm_memory_store` handler consolidated a topic past a
/// hardcoded 10 entries **unconditionally**, ignoring `[memory]
/// auto_consolidate_enabled` / `auto_consolidate_threshold` — so an explicit
/// `enabled = false` still destructively rolled up (and deleted) a topic's
/// memories. `icm serve` now threads the loaded config through as one of
/// these, and the handler honors it.
#[derive(Clone, Copy, Debug)]
pub struct AutoConsolidate {
    pub enabled: bool,
    pub threshold: usize,
    /// An LLM summarizer is configured (`[consolidate.summarizer]` provider
    /// other than `none`): a topic past the threshold is queued for
    /// `icm consolidate-pending` instead of being rolled up on the spot.
    /// The rollup keeps the 3 heaviest memories and deletes the rest; with
    /// a summarizer configured the user asked for a real summary, and the
    /// CLI store path already queues.
    pub queue: bool,
}

impl Default for AutoConsolidate {
    /// The historical always-on behavior (threshold 10). Used only by callers
    /// that don't supply a policy — e.g. tests via [`call_tool`]. The
    /// `icm serve` path passes the user's real config through
    /// [`call_tool_with_config`] instead.
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: AUTO_CONSOLIDATE_THRESHOLD,
            queue: false,
        }
    }
}

/// Maximum allowed length for topic names. Must stay <= the store
/// layer's `MAX_TOPIC_BYTES` so the MCP-level rejection happens
/// *before* the store's lower-level validation does.
const MAX_TOPIC_LEN: usize = 255;

/// Maximum allowed length for content/summary text. Aligned with the
/// store layer's `MAX_SUMMARY_BYTES` (64 KB). Letting MCP accept
/// larger inputs only to have the store reject them would be
/// confusing — fail fast at the API surface.
const MAX_CONTENT_LEN: usize = 64 * 1024;

/// `icm_feedback_record`'s context/predicted/corrected/reason had no length
/// cap at all, unlike icm_memory_store's MAX_CONTENT_LEN (audit finding).
const MAX_FEEDBACK_FIELD_LEN: usize = 20_000;

/// Parse a JSON keywords array from tool arguments.
fn parse_keywords(args: &Value) -> Vec<String> {
    args.get("keywords")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Try to auto-consolidate a topic if the policy is enabled and the topic
/// exceeds the configured threshold (issue #318). Returns a human-readable
/// message if consolidation happened, or an empty string (including when the
/// policy is disabled — a no-op).
///
/// Routes through `auto_consolidate_with_embedder` so the consolidated
/// memory is embedded inline (closes audit M2/AC2: previously the
/// rolled-up memory had `embedding = None` and was invisible to hybrid
/// recall until a manual `icm embed` rebuilt it).
fn try_auto_consolidate(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    topic: &str,
    auto: AutoConsolidate,
) -> String {
    if !auto.enabled {
        return String::new();
    }
    if auto.queue {
        // Same decision as `maybe_auto_consolidate` on the CLI store path.
        return match store.count_by_topic(topic) {
            Ok(n) if n > auto.threshold => match store.enqueue_pending_consolidation(topic, "") {
                Ok(_) => format!(
                    "Topic '{topic}' queued for consolidation (exceeded {} entries).",
                    auto.threshold
                ),
                Err(e) => {
                    tracing::warn!("enqueue consolidation failed for topic '{topic}': {e}");
                    String::new()
                }
            },
            Ok(_) => String::new(),
            Err(e) => {
                tracing::warn!("count_by_topic failed for '{topic}': {e}");
                String::new()
            }
        };
    }
    match store.auto_consolidate_with_embedder(topic, auto.threshold, embedder) {
        Ok(true) => format!(
            "Auto-consolidated topic '{topic}' (exceeded {} entries).",
            auto.threshold
        ),
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
                        "description": "Category/namespace. Use the canonical topics from the server instructions: 'decisions-{project}', 'preferences', 'errors-resolved', 'context-{project}' — mixed-language topic names fragment the memory."
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
                    "max_tokens": {
                        "type": "integer",
                        "minimum": 100,
                        "maximum": 32000,
                        "description": "Token budget (~4 chars/token): return as many of the best matches as fit, instead of `limit` results"
                    },
                    "keyword": {
                        "type": "string",
                        "description": "Filter results by keyword (exact match on memory keywords)"
                    },
                    "project": {
                        "type": "string",
                        "description": "Project filter (segment-aware). Defaults to the server's cwd directory name. Pass an empty string to disable the filter and search across all projects."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "json"],
                        "default": "text",
                        "description": "Output shape. `json` returns an array of records with `id`, `topic`, `summary`, `importance`, `weight`, `keywords`, `related_ids` and timestamps, for clients that parse the result"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "icm_memory_related",
            "description": "List the memories linked to one memory, following the links ICM stores between related memories. Use after a recall with `format: \"json\"`, which gives the ids.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "ID of the memory to start from"
                    },
                    "depth": {
                        "type": "integer",
                        "default": 1,
                        "minimum": 1,
                        "maximum": 3,
                        "description": "How many links to follow from the start memory"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "minimum": 1,
                        "maximum": 50,
                        "description": "Max number of results, nearest first"
                    },
                    "project": {
                        "type": "string",
                        "description": "Project filter, as in icm_memory_recall. Linked memories of another project are left out and not followed. Pass an empty string to follow links across projects."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "json"],
                        "default": "text",
                        "description": "Output shape. `json` returns the same records as icm_memory_recall, with a `hops` field"
                    }
                },
                "required": ["id"]
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
            "name": "icm_memory_forget_topic",
            "description": "Delete ALL memories in a topic. Use to clear an entire topic at once.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Topic whose memories should all be deleted"
                    }
                },
                "required": ["topic"]
            }
        }),
        json!({
            "name": "icm_learn",
            "description": "Scan a project directory and create a Memoir knowledge graph with its structure, dependencies, modules, and config files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "Project directory to scan (default: current working directory)"
                    },
                    "name": {
                        "type": "string",
                        "description": "Memoir name (default: directory name)"
                    }
                }
            }
        }),
        json!({
            "name": "icm_memory_consolidate",
            "description": "Replace memories of a topic with one summary you wrote from them. Two steps: call with only `topic` to list the memories and their ids, then call with `summary` and the `ids` it covers. Only the listed memories are replaced; critical ones never are.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "Topic to consolidate"
                    },
                    "summary": {
                        "type": "string",
                        "description": "Your summary of the memories listed in `ids`"
                    },
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Ids of the memories the summary covers: only these are replaced. Call once WITHOUT ids (and without summary) to get the topic's memories with their ids; nothing is replaced by that call."
                    }
                },
                "required": ["topic"]
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
        // --- Transcript tools (verbatim session replay) ---
        json!({
            "name": "icm_transcript_start_session",
            "description": "Create a new transcript session for verbatim message capture. Returns the session_id used by subsequent icm_transcript_record calls. Use once per conversation or debugging session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Agent identifier (e.g. 'claude-code', 'cursor', 'gemini-cli'). Default: 'mcp'."
                    },
                    "project": {
                        "type": "string",
                        "description": "Project name (optional; usually cwd basename or repo slug)"
                    },
                    "metadata": {
                        "type": "string",
                        "description": "Arbitrary JSON metadata (optional)"
                    }
                }
            }
        }),
        json!({
            "name": "icm_transcript_record",
            "description": "Append a verbatim message to a transcript session. Stores the raw content with no summarization. Use once per user turn, assistant reply, or tool call for full replay fidelity.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session id from icm_transcript_start_session"
                    },
                    "role": {
                        "type": "string",
                        "enum": ["user", "assistant", "system", "tool"],
                        "description": "Message role"
                    },
                    "content": {
                        "type": "string",
                        "description": "Raw message content (stored verbatim)"
                    },
                    "tool_name": {
                        "type": "string",
                        "description": "Tool name if role=tool (optional)"
                    },
                    "tokens": {
                        "type": "integer",
                        "description": "Token count for billing / stats (optional)"
                    },
                    "metadata": {
                        "type": "string",
                        "description": "Arbitrary JSON metadata (optional)"
                    }
                },
                "required": ["session_id", "role", "content"]
            }
        }),
        json!({
            "name": "icm_transcript_search",
            "description": "Full-text search across recorded transcript messages (FTS5 BM25). Supports boolean operators, phrase matches, and prefix queries. Use to recall exact quotes or debug past decisions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "FTS5 query: 'postgres OR mysql', '\"exact phrase\"', 'auth*'"
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Restrict to one session (optional)"
                    },
                    "project": {
                        "type": "string",
                        "description": "Restrict to one project (optional)"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 10,
                        "minimum": 1,
                        "maximum": 50
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "icm_transcript_show",
            "description": "Replay the full message thread of a transcript session, chronologically. Returns up to `limit` messages with role, content, tool name, timestamp.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "limit": { "type": "integer", "default": 200, "minimum": 1, "maximum": 2000 }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "icm_transcript_stats",
            "description": "Global transcript statistics: session count, message count, total bytes, breakdown by role and agent, top sessions by message count.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "icm_wake_up",
            "description": "Build a compact critical-facts pack for LLM system-prompt injection. Selects critical/high memories (and preferences) optionally scoped by project, ranks by importance × recency × weight, and truncates to a token budget. Use at session start to hydrate an agent with the most load-bearing context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "Project name filter (substring match against topic). Preferences/identity memories are always included."
                    },
                    "max_tokens": {
                        "type": "integer",
                        "default": 200,
                        "minimum": 20,
                        "maximum": 4000,
                        "description": "Approximate token budget (1 token ≈ 4 characters)"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "plain"],
                        "default": "markdown",
                        "description": "Output format"
                    },
                    "include_preferences": {
                        "type": "boolean",
                        "default": true,
                        "description": "Include global preferences/identity memories regardless of the project filter"
                    }
                }
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
    store: &Store,
    embedder: Option<&dyn Embedder>,
    name: &str,
    args: &Value,
    compact: bool,
) -> ToolResult {
    call_tool_with_config(
        store,
        embedder,
        name,
        args,
        compact,
        AutoConsolidate::default(),
    )
}

/// Like [`call_tool`] but with an explicit auto-consolidation policy
/// (issue #318). `icm serve` calls this with the user's loaded config so an
/// `auto_consolidate_enabled = false` is honored on the MCP store path.
pub fn call_tool_with_config(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    name: &str,
    args: &Value,
    compact: bool,
    auto_consolidate: AutoConsolidate,
) -> ToolResult {
    match name {
        // Memory tools
        "icm_memory_store" => tool_store(store, embedder, args, compact, auto_consolidate),
        "icm_memory_recall" => tool_recall(store, embedder, args, compact),
        "icm_memory_related" => tool_related(store, args, compact),
        "icm_memory_forget" => tool_forget(store, args),
        "icm_memory_forget_topic" => tool_forget_topic(store, args),
        "icm_memory_update" => tool_update(store, embedder, args),
        "icm_memory_consolidate" => tool_consolidate(store, embedder, args),
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
        // Learn tool
        "icm_learn" => tool_learn(store, args),
        // Feedback tools
        "icm_feedback_record" => tool_feedback_record(store, embedder, args, compact),
        "icm_feedback_search" => tool_feedback_search(store, embedder, args),
        "icm_feedback_stats" => tool_feedback_stats(store),
        // Transcript tools
        "icm_transcript_start_session" => tool_transcript_start_session(store, args),
        "icm_transcript_record" => tool_transcript_record(store, args),
        "icm_transcript_search" => tool_transcript_search(store, args),
        "icm_transcript_show" => tool_transcript_show(store, args),
        "icm_transcript_stats" => tool_transcript_stats(store),
        // Wake-up tool
        "icm_wake_up" => tool_wake_up(store, args),
        _ => ToolResult::error(format!("unknown tool: {name}")),
    }
}

// ---------------------------------------------------------------------------
// Transcript tool handlers
// ---------------------------------------------------------------------------

fn tool_transcript_start_session(store: &Store, args: &Value) -> ToolResult {
    use icm_core::TranscriptStore;
    let agent = args.get("agent").and_then(|v| v.as_str()).unwrap_or("mcp");
    let project = args.get("project").and_then(|v| v.as_str());
    let metadata = args.get("metadata").and_then(|v| v.as_str());
    match store.create_session(agent, project, metadata) {
        Ok(id) => ToolResult::text(format!("{{\"session_id\":\"{id}\"}}")),
        Err(e) => ToolResult::error(format!("start_session failed: {e}")),
    }
}

fn tool_transcript_record(store: &Store, args: &Value) -> ToolResult {
    use icm_core::{Role, TranscriptStore};
    let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("session_id is required".into()),
    };
    let role_str = match args.get("role").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("role is required".into()),
    };
    let role = match Role::parse(role_str) {
        Some(r) => r,
        None => {
            return ToolResult::error(format!(
                "invalid role '{role_str}'; must be user|assistant|system|tool"
            ));
        }
    };
    let content = match args.get("content").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("content is required".into()),
    };
    let tool_name = args.get("tool_name").and_then(|v| v.as_str());
    let tokens = args.get("tokens").and_then(|v| v.as_i64());
    let metadata = args.get("metadata").and_then(|v| v.as_str());
    match store.record_message(session_id, role, content, tool_name, tokens, metadata) {
        Ok(id) => ToolResult::text(format!("{{\"message_id\":\"{id}\"}}")),
        Err(e) => ToolResult::error(format!("record failed: {e}")),
    }
}

fn tool_transcript_search(store: &Store, args: &Value) -> ToolResult {
    use icm_core::TranscriptStore;
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("query is required".into()),
    };
    let session_id = args.get("session_id").and_then(|v| v.as_str());
    let project = args.get("project").and_then(|v| v.as_str());
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .min(50) as usize;
    match store.search_transcripts(query, session_id, project, limit) {
        Ok(hits) => {
            let json = serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into());
            ToolResult::text(json)
        }
        Err(e) => ToolResult::error(format!("search failed: {e}")),
    }
}

fn tool_transcript_show(store: &Store, args: &Value) -> ToolResult {
    use icm_core::TranscriptStore;
    let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ToolResult::error("session_id is required".into()),
    };
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
        .min(2000) as usize;
    let sess = match store.get_session(session_id) {
        Ok(Some(s)) => s,
        Ok(None) => return ToolResult::error(format!("session {session_id} not found")),
        Err(e) => return ToolResult::error(format!("get_session failed: {e}")),
    };
    let msgs = match store.list_session_messages(session_id, limit, 0) {
        Ok(m) => m,
        Err(e) => return ToolResult::error(format!("list_messages failed: {e}")),
    };
    let body = json!({ "session": sess, "messages": msgs });
    ToolResult::text(body.to_string())
}

fn tool_transcript_stats(store: &Store) -> ToolResult {
    use icm_core::TranscriptStore;
    match store.transcript_stats() {
        Ok(s) => ToolResult::text(serde_json::to_string(&s).unwrap_or_else(|_| "{}".into())),
        Err(e) => ToolResult::error(format!("stats failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Wake-up tool handler
// ---------------------------------------------------------------------------

fn tool_wake_up(store: &Store, args: &Value) -> ToolResult {
    // Normalize the project filter: empty string or "-" both mean "disabled",
    // mirroring the CLI convention.
    let project = match get_str(args, "project") {
        Some("") | Some("-") => None,
        other => other,
    };
    // Clamp token budget to [20, 4000] to guard against accidental blowups.
    let max_tokens = get_i64(args, "max_tokens", 200).clamp(20, 4000) as usize;
    let format = match get_str(args, "format").unwrap_or("markdown") {
        "plain" => WakeUpFormat::Plain,
        _ => WakeUpFormat::Markdown,
    };
    let include_preferences = args
        .get("include_preferences")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let opts = WakeUpOptions {
        project,
        max_tokens,
        format,
        include_preferences,
    };

    match build_wake_up(store, &opts) {
        Ok(pack) => ToolResult::text(pack),
        Err(e) => ToolResult::error(format!("wake_up failed: {e}")),
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

/// The project a recall is scoped to. The explicit `project` argument wins
/// (an empty string disables the filter); otherwise it is derived from the
/// server's cwd through the shared icm-core detection (git remote first) —
/// the CLI hooks store under that name, so a raw cwd basename would
/// silently miss on renamed checkouts (audit finding).
fn project_scope(args: &Value) -> Option<String> {
    match get_str(args, "project") {
        Some("") => None,
        Some(p) => Some(p.to_string()),
        None => std::env::current_dir()
            .ok()
            .and_then(|p| icm_core::project::project_from_path(&p.to_string_lossy())),
    }
}

fn resolve_memoir(store: &Store, name: &str) -> Result<Memoir, ToolResult> {
    store
        .get_memoir_by_name(name)
        .map_err(|e| ToolResult::error(format!("db error: {e}")))?
        .ok_or_else(|| ToolResult::error(format!("memoir not found: {name}")))
}

// ---------------------------------------------------------------------------
// Memory tool handlers
// ---------------------------------------------------------------------------

fn tool_store(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    args: &Value,
    compact: bool,
    auto_consolidate: AutoConsolidate,
) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t,
        None => return ToolResult::error("missing required field: topic".into()),
    };
    let content = match get_str(args, "content") {
        Some(c) => c,
        None => return ToolResult::error("missing required field: content".into()),
    };

    // Empty-string validation: the inputSchema marks `topic` and
    // `content` as required, but JSON allows passing `""` which slips
    // past the structural check. Reject explicitly so callers don't
    // silently end up with a memory under a blank topic that they
    // can't meaningfully recall.
    if topic.trim().is_empty() {
        return ToolResult::error("topic must not be empty".into());
    }
    if content.trim().is_empty() {
        return ToolResult::error("content must not be empty".into());
    }

    // Input length validation
    if topic.len() > MAX_TOPIC_LEN {
        return ToolResult::error(format!(
            "topic exceeds maximum length ({} > {MAX_TOPIC_LEN} chars)",
            topic.len()
        ));
    }
    if content.len() > MAX_CONTENT_LEN {
        return ToolResult::error(format!(
            "content exceeds maximum length ({} > {MAX_CONTENT_LEN} chars)",
            content.len()
        ));
    }

    let importance_str = get_str(args, "importance").unwrap_or("medium");
    let importance = importance_str
        .parse()
        .unwrap_or(icm_core::Importance::Medium);

    let mut memory = Memory::new(topic.into(), content.into(), importance);

    let kw = parse_keywords(args);
    if !kw.is_empty() {
        memory.keywords = kw;
    }

    if let Some(raw) = get_str(args, "raw_excerpt") {
        memory.raw_excerpt = Some(raw.into());
    }

    // Auto-embed if embedder is available
    let embed_text = memory.embed_text();
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
        if let Ok(Some((existing, score))) = find_similar_memory(
            store,
            &embed_text,
            query_emb,
            topic,
            DEDUP_SIMILARITY_THRESHOLD,
        ) {
            let updated = Memory {
                id: existing.id.clone(),
                created_at: existing.created_at,
                last_accessed: existing.last_accessed,
                access_count: existing.access_count,
                weight: 1.0,
                topic: existing.topic.clone(),
                // Never wholesale-replace: `existing` and the incoming
                // content are only known to be semantically close (cosine
                // similarity), not the same statement — see
                // `merge_summaries`'s docs for a measured case (two distinct
                // LoCoMo greeting turns scored 0.98) where that destroyed
                // the earlier memory's content.
                summary: icm_core::merge_summaries(&existing.summary, content),
                raw_excerpt: get_str(args, "raw_excerpt")
                    .map(|r| r.into())
                    .or_else(|| existing.raw_excerpt.clone()),
                keywords: icm_core::union_keywords(&existing.keywords, &parse_keywords(args)),
                embedding: Some(query_emb.clone()),
                // Never let a near-dup merge downgrade importance: an MCP
                // caller that omits `importance` defaults to Medium, which
                // would otherwise silently demote an existing Critical
                // memory into decay/prune eligibility (audit finding).
                importance: icm_core::max_importance(existing.importance, importance),
                source: existing.source.clone(),
                related_ids: existing.related_ids.clone(),
                updated_at: Utc::now(),
                scope: existing.scope,
            };
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

    // Auto-link: populate `related_ids` with similar existing memories BEFORE
    // storing, so the new memory lands in the DB with its forward edges
    // already set. Back-refs are added AFTER storing so the linked memories
    // point to an id that exists in the DB.
    let auto_link_opts = AutoLinkOptions::default();
    let linked_ids = if memory.embedding.is_some() {
        auto_link_memory(store, &mut memory, &auto_link_opts).unwrap_or_else(|e| {
            tracing::warn!("auto-link failed: {e}");
            Vec::new()
        })
    } else {
        Vec::new()
    };

    match store.store(memory) {
        Ok(id) => {
            // Best-effort back-ref update. Failure here leaves an asymmetric
            // edge (forward-only) but does not fail the store call.
            if !linked_ids.is_empty() {
                if let Err(e) = add_backrefs(store, &id, &linked_ids) {
                    tracing::warn!("auto-link back-ref update failed: {e}");
                }
            }

            let link_suffix = if linked_ids.is_empty() {
                String::new()
            } else {
                format!(
                    " (+{} link{})",
                    linked_ids.len(),
                    if linked_ids.len() == 1 { "" } else { "s" }
                )
            };

            if compact {
                // Try auto-consolidation even in compact mode
                let consolidation_msg =
                    try_auto_consolidate(store, embedder, topic, auto_consolidate);
                if consolidation_msg.is_empty() {
                    ToolResult::text(format!("ok:{id}{link_suffix}"))
                } else {
                    ToolResult::text(format!("ok:{id}{link_suffix}\n{consolidation_msg}"))
                }
            } else {
                let consolidation_msg =
                    try_auto_consolidate(store, embedder, topic, auto_consolidate);
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
                    ToolResult::text(format!("Stored memory: {id}{link_suffix}{hint}"))
                } else {
                    ToolResult::text(format!(
                        "Stored memory: {id}{link_suffix}\n{consolidation_msg}"
                    ))
                }
            }
        }
        Err(e) => ToolResult::error(format!("failed to store: {e}")),
    }
}

fn format_memory_output(memories: &[(Memory, f32)], compact: bool) -> String {
    let mut output = String::new();
    for (mem, score) in memories {
        push_memory_output(&mut output, mem, *score, compact);
    }
    output
}

/// Output shape of `icm_memory_recall` and `icm_memory_related` (#476).
/// `Text` is what the tools always printed; `Json` is for clients that
/// parse the result and need each record's id and links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
}

/// The `format` argument. Absent means `Text`; an unknown value is refused
/// rather than answered in a shape the caller did not ask for.
fn output_format(args: &Value) -> Result<OutputFormat, String> {
    match args.get("format") {
        None | Some(Value::Null) => Ok(OutputFormat::Text),
        Some(Value::String(s)) if s == "text" => Ok(OutputFormat::Text),
        Some(Value::String(s)) if s == "json" => Ok(OutputFormat::Json),
        Some(other) => Err(format!(
            "invalid format {other}: expected \"text\" or \"json\""
        )),
    }
}

/// Longest `raw_excerpt` shown per memory. The excerpt can hold up to 64 KB;
/// dumping it in full for every hit floods the client LLM's context (audit
/// finding). The full excerpt stays in the store.
const MAX_RAW_IN_RECALL: usize = 2048;

/// `raw` cut to [`MAX_RAW_IN_RECALL`] bytes on a character boundary.
fn capped_raw(raw: &str) -> &str {
    if raw.len() <= MAX_RAW_IN_RECALL {
        return raw;
    }
    let mut cut = MAX_RAW_IN_RECALL;
    while !raw.is_char_boundary(cut) {
        cut -= 1;
    }
    &raw[..cut]
}

/// One memory as a JSON record (#476). `score` is present only when a
/// similarity was computed (negative means none, as in the text view);
/// `hops` only in `icm_memory_related`. JSON escapes the text itself, so
/// nothing is flattened here.
fn memory_json(mem: &Memory, score: f32, hops: Option<usize>) -> Value {
    // Three decimals, as in the text view: an f32 printed as a JSON number
    // would otherwise read 0.9750000238418579.
    let rounded = |v: f32| (f64::from(v) * 1000.0).round() / 1000.0;
    let mut record = json!({
        "id": mem.id,
        "topic": mem.topic,
        "summary": mem.summary,
        "importance": mem.importance.to_string(),
        "weight": rounded(mem.weight),
        "keywords": mem.keywords,
        "related_ids": mem.related_ids,
        "created_at": mem.created_at.to_rfc3339(),
        "updated_at": mem.updated_at.to_rfc3339(),
    });
    if score >= 0.0 {
        record["score"] = json!(rounded(score));
    }
    if let Some(hops) = hops {
        record["hops"] = json!(hops);
    }
    if let Some(raw) = mem.raw_excerpt.as_deref() {
        let shown = capped_raw(raw);
        record["raw_excerpt"] = json!(shown);
        if shown.len() < raw.len() {
            record["raw_excerpt_bytes"] = json!(raw.len());
        }
    }
    record
}

/// The answer of a recall in the requested shape. In `Json` the server's
/// compact setting does not apply: the caller asked for the records.
fn render_memories(memories: &[(Memory, f32)], compact: bool, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => format_memory_output(memories, compact),
        OutputFormat::Json => Value::Array(
            memories
                .iter()
                .map(|(mem, score)| memory_json(mem, *score, None))
                .collect(),
        )
        .to_string(),
    }
}

/// What a recall with no match answers: the usual sentence, or an empty
/// array for a caller that parses JSON.
fn no_memories(format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => MSG_NO_MEMORIES.into(),
        OutputFormat::Json => "[]".into(),
    }
}

/// Append one recalled memory to `output`. Kept separate from the loop so
/// the v2 token budget can charge a hit exactly what is printed for it.
fn push_memory_output(output: &mut String, mem: &Memory, score: f32, compact: bool) {
    // Audit finding: `summary` has no newline/CR validation at the store
    // layer (only `topic` is checked — see `validate_fields`), and it can
    // be LLM/tool-extracted from untrusted content. Written verbatim, a
    // stored summary could forge a fake `--- <id> [score: ...] ---`
    // delimiter indistinguishable from a real entry, or (compact mode) a
    // fake `[topic] ...` line. `keywords` has no validation at all. Flatten
    // both, same fix already applied to recall_context/render_detail.
    let flatten = |s: &str| s.replace(['\n', '\r'], " ");
    if compact {
        output.push_str(&format!("[{}] {}\n", mem.topic, flatten(&mem.summary)));
        return;
    }
    let summary = flatten(&mem.summary);
    if score >= 0.0 {
        output.push_str(&format!(
            "--- {} [score: {:.3}] ---\n  topic: {}\n  importance: {}\n  weight: {:.3}\n  summary: {}\n",
            mem.id, score, mem.topic, mem.importance, mem.weight, summary
        ));
    } else {
        output.push_str(&format!(
            "--- {} ---\n  topic: {}\n  importance: {}\n  weight: {:.3}\n  summary: {}\n",
            mem.id, mem.topic, mem.importance, mem.weight, summary
        ));
    }
    if !mem.keywords.is_empty() {
        let flattened_keywords: Vec<String> = mem.keywords.iter().map(|k| flatten(k)).collect();
        output.push_str(&format!("  keywords: {}\n", flattened_keywords.join(", ")));
    }
    if !mem.related_ids.is_empty() {
        // Ids are generated by ICM, but `related_ids` is a stored field
        // like the others: flattened for the same reason.
        let related: Vec<String> = mem.related_ids.iter().map(|id| flatten(id)).collect();
        output.push_str(&format!("  related: {}\n", related.join(", ")));
    }
    if let Some(ref raw) = mem.raw_excerpt {
        let shown = capped_raw(raw);
        if shown.len() < raw.len() {
            output.push_str(&format!(
                "  raw: {shown}… [truncated, {} bytes total]\n",
                raw.len()
            ));
        } else {
            output.push_str(&format!("  raw: {raw}\n"));
        }
    }
    output.push('\n');
}

/// The `max_tokens` argument of `icm_memory_recall`, clamped to the
/// schema's 100..=32000. Clients do not all send a JSON integer: a whole
/// float (`2000.0`) or a numeric string (`"2000"`) means the same thing
/// and is accepted. Anything else is refused rather than ignored, which
/// would silently fall back to a count-based recall.
fn recall_max_tokens(args: &Value) -> Result<Option<usize>, String> {
    let value = match args.get("max_tokens") {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    match whole_number(value) {
        Some(n) => Ok(Some(n.clamp(100, 32_000) as usize)),
        None => Err(format!(
            "invalid max_tokens {value}: expected a whole number of tokens (100 to 32000)"
        )),
    }
}

/// A whole number from a tool argument: a JSON integer, a whole float
/// (`3.0`, a valid JSON Schema `integer`) or a numeric string (`"3"`).
/// `None` for anything else.
fn whole_number(value: &Value) -> Option<i64> {
    let whole = |f: f64| (f.is_finite() && f.fract() == 0.0).then_some(f as i64);
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().and_then(whole)),
        Value::String(s) => {
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().and_then(whole))
        }
        _ => None,
    }
}

/// A count argument (`depth`, `limit`) of `icm_memory_related`, clamped to
/// `1..=max`. A value that is not a whole number is refused rather than
/// replaced by the default: `depth: "three"` answered at depth 1 would read
/// as "there are no deeper links".
fn count_arg(args: &Value, key: &str, default: i64, max: i64) -> Result<usize, String> {
    let value = match args.get(key) {
        None | Some(Value::Null) => return Ok(default as usize),
        Some(v) => v,
    };
    match whole_number(value) {
        Some(n) => Ok(n.clamp(1, max) as usize),
        None => Err(format!(
            "invalid {key} {value}: expected a whole number (1 to {max})"
        )),
    }
}

fn tool_recall(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    args: &Value,
    compact: bool,
) -> ToolResult {
    // The v2 engine (rank fusion, token budget) is the default. The engine
    // is not a tool parameter: `ICM_RECALL_ENGINE=legacy` in the server's
    // environment brings back the previous one.
    let max_tokens = match recall_max_tokens(args) {
        Ok(v) => v,
        Err(msg) => return ToolResult::error(msg),
    };
    let engine = match RecallEngine::resolve(None, max_tokens.is_some()) {
        Ok(engine) => engine,
        Err(e) => return ToolResult::error(format!("{e}")),
    };
    tool_recall_on(store, embedder, args, compact, engine, max_tokens)
}

/// `icm_memory_recall` on a given engine. Below the `V2` dispatch, the
/// legacy path is the pre-v2 code, unchanged.
fn tool_recall_on(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    args: &Value,
    compact: bool,
    engine: RecallEngine,
    max_tokens: Option<usize>,
) -> ToolResult {
    // Auto-decay if >24h since last decay. The v2 pipeline runs its own.
    if engine == RecallEngine::Legacy {
        if let Err(e) = store.maybe_auto_decay() {
            tracing::warn!(error = %e, "auto-decay failed during recall");
        }
    }

    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    // Clamp to the schema's advertised maximum (20) — the code previously
    // accepted up to 100, silently diverging from the published contract.
    let limit = get_i64(args, "limit", 5).clamp(1, 20) as usize;
    let topic = get_str(args, "topic");
    let keyword = get_str(args, "keyword");

    // Project filter: same hard segment-aware filter applied to the CLI
    // `recall_context` path (extract.rs) so MCP-side recall can't leak
    // memories from other projects. Caller can override via the explicit
    // `project` arg (empty string disables the filter); otherwise we
    // derive it from the server's cwd via the shared icm-core detection
    // (git remote first) — the CLI hooks store under that name, so a raw
    // cwd basename would silently miss on renamed checkouts (audit finding).
    let project = project_scope(args);
    let format = match output_format(args) {
        Ok(f) => f,
        Err(msg) => return ToolResult::error(msg),
    };

    if engine == RecallEngine::V2 {
        let req = RecallRequest {
            query,
            limit: v2_limit(args.get("limit").and_then(|v| v.as_i64()), max_tokens),
            max_tokens,
            topic,
            keyword,
            project: project.as_deref(),
            now: None,
            expand_neighbors: true,
        };
        return tool_recall_v2(store, embedder, &req, compact, format);
    }

    let project_filter = |m: &Memory| -> bool {
        match project.as_deref() {
            None => true,
            Some(p) => is_preference_topic(&m.topic) || project_matches(&m.topic, Some(p)),
        }
    };

    // Audit finding: filters were applied AFTER the store already truncated
    // to `limit` — if the top-`limit` global hits all belonged to other
    // projects/topics, filtering left nothing and recall reported "no
    // memories" even though relevant matches existed further down the
    // ranked list. When any filter is active, request a much larger
    // candidate pool so filtering has enough to work with, then truncate to
    // the caller's requested `limit` at the very end (capped — this is a
    // memory-scoped search, not a paginated export).
    let filters_active = project.is_some() || topic.is_some() || keyword.is_some();
    let query_limit = if filters_active {
        (limit * 10).min(200)
    } else {
        limit
    };

    // Try hybrid search if embedder is available
    if let Some(emb) = embedder {
        if let Ok(query_emb) = emb.embed_query(query) {
            if let Ok(results) = store.search_hybrid(query, &query_emb, query_limit) {
                let mut scored_results = results;
                scored_results.retain(|(m, _)| project_filter(m));
                if let Some(t) = topic {
                    scored_results.retain(|(m, _)| topic_matches(&m.topic, t));
                }
                if let Some(kw) = keyword {
                    scored_results.retain(|(m, _)| keyword_matches(&m.keywords, kw));
                }

                // Graph-aware expansion: follow `related_ids` one hop from
                // each primary hit and fold neighbors into the result set.
                // Neighbors carry a discounted score so they rank below
                // direct matches but can displace weak primary results.
                //
                // Audit R13b: neighbors are fetched by id without going
                // through the project / topic / keyword filters above,
                // so a project-A primary hit can pull in a project-B
                // neighbor via auto-linked `related_ids`. Re-apply the
                // filters to `expanded` so the caller's scope is honored.
                let max_neighbors = (query_limit / 3).max(1);
                let mut expanded = store
                    .expand_with_neighbors(&scored_results, max_neighbors, 0.5, query_limit)
                    .unwrap_or(scored_results);
                expanded.retain(|(m, _)| project_filter(m));
                if let Some(t) = topic {
                    expanded.retain(|(m, _)| topic_matches(&m.topic, t));
                }
                if let Some(kw) = keyword {
                    expanded.retain(|(m, _)| keyword_matches(&m.keywords, kw));
                }
                expanded.truncate(limit);

                // Batch update access counts (includes expanded neighbors)
                let ids: Vec<&str> = expanded.iter().map(|(m, _)| m.id.as_str()).collect();
                let _ = store.batch_update_access(&ids);

                if expanded.is_empty() {
                    return ToolResult::text(no_memories(format));
                }

                return ToolResult::text(render_memories(&expanded, compact, format));
            }
        }
    }

    // Fallback: FTS then keywords
    let mut results = match store.search_fts(query, query_limit) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("search error: {e}")),
    };

    if results.is_empty() {
        let keywords: Vec<&str> = query.split_whitespace().collect();
        results = match store.search_by_keywords(&keywords, query_limit) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("search error: {e}")),
        };
    }

    results.retain(|m| project_filter(m));
    if let Some(t) = topic {
        results.retain(|m| topic_matches(&m.topic, t));
    }
    if let Some(kw) = keyword {
        results.retain(|m| keyword_matches(&m.keywords, kw));
    }
    results.truncate(limit);

    // Convert to scored format with a sentinel score of 1.0 (FTS fallback
    // doesn't expose a real similarity score, but we still want the graph
    // expansion to score neighbors relative to their primary parent).
    let scored: Vec<(Memory, f32)> = results.into_iter().map(|m| (m, 1.0)).collect();

    // Graph-aware expansion also applies in the fallback path so that
    // keyword-only deployments benefit from auto-linked memories.
    // Same R13b re-filter as the hybrid path.
    let max_neighbors = (limit / 3).max(1);
    let mut expanded = store
        .expand_with_neighbors(&scored, max_neighbors, 0.5, limit)
        .unwrap_or(scored);
    expanded.retain(|(m, _)| project_filter(m));
    if let Some(t) = topic {
        expanded.retain(|(m, _)| topic_matches(&m.topic, t));
    }
    if let Some(kw) = keyword {
        expanded.retain(|(m, _)| keyword_matches(&m.keywords, kw));
    }

    // Batch update access counts (includes expanded neighbors)
    let ids: Vec<&str> = expanded.iter().map(|(m, _)| m.id.as_str()).collect();
    let _ = store.batch_update_access(&ids);

    if expanded.is_empty() {
        return ToolResult::text(no_memories(format));
    }

    // FTS-path results have synthetic scores — reset to -1.0 for display
    // so we don't claim a hybrid-search confidence we didn't compute.
    let for_display: Vec<(Memory, f32)> = expanded.into_iter().map(|(m, _)| (m, -1.0)).collect();
    ToolResult::text(render_memories(&for_display, compact, format))
}

/// Result cap for the v2 engine. Without a budget the published 1..=20
/// range applies, as on the legacy path. With a token budget the budget
/// does the cutting, so the count is only a guard: up to 200, and 200 when
/// the caller gave none (the schema default of 5 would make the budget
/// pointless).
fn v2_limit(requested: Option<i64>, max_tokens: Option<usize>) -> usize {
    let (default, max) = if max_tokens.is_some() {
        (200, 200)
    } else {
        (5, 20)
    };
    requested.unwrap_or(default).clamp(1, max) as usize
}

/// `icm_memory_recall` through the shared v2 pipeline. Filtering, neighbor
/// expansion, the budget cut and the access-count update all happen inside
/// the pipeline; the rendering is MCP's own, and each hit is charged
/// against the budget for the text rendered here, in this mode.
///
/// Same output shape as the legacy path: a score is shown only when an
/// embedder took part in the ranking. A keyword-only ranking has no
/// similarity to report, and the legacy path showed none either.
fn tool_recall_v2(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    req: &RecallRequest<'_>,
    compact: bool,
    format: OutputFormat,
) -> ToolResult {
    // The legacy path answered a blank query with "no memories", not with
    // an error.
    if req.query.trim().is_empty() {
        return ToolResult::text(no_memories(format));
    }
    let shown = |score: f32| if embedder.is_some() { score } else { -1.0 };
    let rendered_tokens = |mem: &Memory, score: f32| {
        let text = match format {
            OutputFormat::Text => {
                let mut text = String::new();
                push_memory_output(&mut text, mem, shown(score), compact);
                text
            }
            OutputFormat::Json => memory_json(mem, shown(score), None).to_string(),
        };
        icm_core::estimate_tokens(&text)
    };
    let outcome = match req.run_with_cost(store, embedder, &rendered_tokens) {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("search error: {e}")),
    };
    if outcome.hits.is_empty() {
        return ToolResult::text(no_memories(format));
    }
    let scored: Vec<(Memory, f32)> = outcome
        .hits
        .into_iter()
        .map(|h| (h.memory, shown(h.score)))
        .collect();
    ToolResult::text(render_memories(&scored, compact, format))
}

/// Result cap of `icm_memory_related`, as published in its schema.
const RELATED_MAX_LIMIT: i64 = 50;
/// Deepest walk `icm_memory_related` does, as published in its schema.
const RELATED_MAX_DEPTH: i64 = 3;

/// `icm_memory_related`: the memories linked to one memory (#476).
///
/// Walks `related_ids` breadth-first from `id`, nearest first, up to
/// `depth` links away. The project scope is the one of
/// `icm_memory_recall`: a linked memory of another project is left out and
/// its own links are not followed, so the walk cannot leave the project
/// through it. A start memory outside the scope is an error, not an empty
/// answer that would read as "no links". Ids that no longer exist are
/// skipped.
fn tool_related(store: &Store, args: &Value, compact: bool) -> ToolResult {
    let format = match output_format(args) {
        Ok(f) => f,
        Err(msg) => return ToolResult::error(msg),
    };
    let id = match get_str(args, "id").map(str::trim) {
        Some(id) if !id.is_empty() => id,
        _ => return ToolResult::error("missing required field: id".into()),
    };
    let depth = match count_arg(args, "depth", 1, RELATED_MAX_DEPTH) {
        Ok(n) => n,
        Err(msg) => return ToolResult::error(msg),
    };
    let limit = match count_arg(args, "limit", 10, RELATED_MAX_LIMIT) {
        Ok(n) => n,
        Err(msg) => return ToolResult::error(msg),
    };
    let project = project_scope(args);
    let in_scope = |m: &Memory| match project.as_deref() {
        None => true,
        Some(p) => is_preference_topic(&m.topic) || project_matches(&m.topic, Some(p)),
    };

    let start = match store.get(id) {
        Ok(Some(m)) => m,
        Ok(None) => return ToolResult::error(format!("memory not found: {id}")),
        Err(e) => return ToolResult::error(format!("failed to get memory {id}: {e}")),
    };
    if !in_scope(&start) {
        return ToolResult::error(format!(
            "memory {id} (topic {:?}) is outside project {:?}. Pass `project: \"\"` to follow \
             links across projects.",
            start.topic,
            project.as_deref().unwrap_or_default()
        ));
    }

    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::from([start.id.clone()]);
    let mut frontier = vec![start];
    let mut found: Vec<(Memory, usize)> = Vec::new();
    'walk: for hop in 1..=depth {
        let mut ids: Vec<String> = Vec::new();
        for mem in &frontier {
            for related in &mem.related_ids {
                if seen.insert(related.clone()) {
                    ids.push(related.clone());
                }
            }
        }
        if ids.is_empty() {
            break;
        }
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut fetched = match store.get_many(&refs) {
            Ok(f) => f,
            Err(e) => return ToolResult::error(format!("failed to get linked memories: {e}")),
        };
        frontier = Vec::new();
        for related in &ids {
            let Some(mem) = fetched.remove(related) else {
                continue;
            };
            if !in_scope(&mem) {
                continue;
            }
            if found.len() >= limit {
                break 'walk;
            }
            found.push((mem.clone(), hop));
            frontier.push(mem);
        }
    }

    if found.is_empty() {
        return ToolResult::text(match format {
            OutputFormat::Text => "No related memories.".into(),
            OutputFormat::Json => "[]".into(),
        });
    }

    // Following a link is a read of the memory, as in recall.
    let found_ids: Vec<&str> = found.iter().map(|(m, _)| m.id.as_str()).collect();
    let _ = store.batch_update_access(&found_ids);

    ToolResult::text(match format {
        OutputFormat::Json => Value::Array(
            found
                .iter()
                .map(|(mem, hops)| memory_json(mem, -1.0, Some(*hops)))
                .collect(),
        )
        .to_string(),
        OutputFormat::Text => {
            let mut output = String::new();
            for (mem, _) in &found {
                push_memory_output(&mut output, mem, -1.0, compact);
            }
            output
        }
    })
}

fn tool_forget(store: &Store, args: &Value) -> ToolResult {
    let id = match get_str(args, "id") {
        Some(id) => id,
        None => return ToolResult::error("missing required field: id".into()),
    };

    match store.delete(id) {
        Ok(()) => ToolResult::text(format!("Deleted memory: {id}")),
        Err(e) => ToolResult::error(format!("failed to delete: {e}")),
    }
}

fn tool_forget_topic(store: &Store, args: &Value) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t,
        None => return ToolResult::error("missing required field: topic".into()),
    };

    let memories = match store.get_by_topic(topic) {
        Ok(m) => m,
        Err(e) => return ToolResult::error(format!("failed to get memories: {e}")),
    };

    let count = memories.len();
    for m in &memories {
        if let Err(e) = store.delete(&m.id) {
            return ToolResult::error(format!("failed to delete memory {}: {e}", m.id));
        }
    }

    ToolResult::text(format!("Deleted {count} memories from topic: {topic}"))
}

fn tool_learn(store: &Store, args: &Value) -> ToolResult {
    let dir_str = get_str(args, "directory").unwrap_or(".");
    let dir = std::path::PathBuf::from(dir_str);

    if !dir.exists() || !dir.is_dir() {
        return ToolResult::error(format!("directory not found: {}", dir.display()));
    }

    let name = get_str(args, "name");

    match icm_core::learn_project(store, &dir, name) {
        Ok(result) => ToolResult::text(result.to_string()),
        Err(e) => ToolResult::error(format!("learn failed: {e}")),
    }
}

/// How many memories the listing answer of `icm_memory_consolidate` shows.
const CONSOLIDATE_LISTING: usize = 40;

/// `icm_memory_consolidate`: replace the memories the caller summarized.
///
/// The caller wrote `summary` from memories it read; `ids` says which. The
/// tool never guesses: it used to replace everything the topic held, and in
/// the default (compact) mode no tool output shows ids, so an agent that
/// had seen the 5 to 20 memories a recall returns wiped out a topic of
/// hundreds. Two choices were possible — show ids in recall output, or stop
/// replacing blind. This does the second without touching recall: called
/// without `ids`, the tool writes nothing and answers with the topic's
/// memories and their ids, which is also what makes `ids` usable at all.
fn tool_consolidate(store: &Store, embedder: Option<&dyn Embedder>, args: &Value) -> ToolResult {
    let topic = match get_str(args, "topic") {
        Some(t) => t.trim(),
        None => return ToolResult::error("missing required field: topic".into()),
    };

    let ids: Vec<String> = match args.get("ids") {
        // Absent: answer with what there is to consolidate.
        None | Some(Value::Null) => return consolidate_listing(store, topic),
        Some(Value::Array(listed)) => {
            // Empty is not "everything": at the store, an empty list
            // replaces nothing, and here it must not mean the opposite.
            if listed.is_empty() {
                return ToolResult::error(
                    "ids is empty: nothing was replaced. Pass the ids of the memories the \
                     summary covers; call without `ids` to list them."
                        .into(),
                );
            }
            let mut ids = Vec::with_capacity(listed.len());
            for id in listed {
                match id.as_str() {
                    Some(id) => ids.push(id.to_string()),
                    None => return ToolResult::error("ids must be an array of strings".into()),
                }
            }
            ids
        }
        Some(_) => return ToolResult::error("ids must be an array of strings".into()),
    };
    let summary = match get_str(args, "summary") {
        Some(s) => s,
        None => return ToolResult::error("missing required field: summary".into()),
    };

    // The listed memories must be in this topic — a topic that differs by
    // case or a blank used to match nothing and leave a stray summary
    // behind, reported as a success. They also set the summary's
    // importance: the highest of what it replaces, as on every other path.
    let mut not_here: Vec<&str> = Vec::new();
    let mut importance: Option<icm_core::Importance> = None;
    let mut critical = 0usize;
    for id in &ids {
        match store.get(id) {
            Ok(Some(m)) if m.topic == topic => {
                if m.importance == icm_core::Importance::Critical {
                    critical += 1;
                } else {
                    importance = Some(match importance {
                        Some(best) => icm_core::max_importance(best, m.importance),
                        None => m.importance,
                    });
                }
            }
            Ok(_) => not_here.push(id),
            Err(e) => return ToolResult::error(format!("failed to get memory {id}: {e}")),
        }
    }
    if !not_here.is_empty() {
        return ToolResult::error(format!(
            "nothing was replaced: {} of the listed ids are not in topic {topic:?} ({}). Check \
             the topic's exact spelling, or call without `ids` to list its memories.",
            not_here.len(),
            not_here
                .iter()
                .take(5)
                .copied()
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let Some(importance) = importance else {
        return ToolResult::error(
            "nothing was replaced: every listed memory is critical, and critical memories are \
             never consolidated."
                .into(),
        );
    };

    let mut consolidated = Memory::new(topic.into(), summary.into(), importance);
    // Same bug class as #394/#395/cmd_consolidate: this tool never attached
    // an embedding to the merged memory it creates.
    if let Some(emb) = embedder {
        if let Ok(vec) = emb.embed(&consolidated.embed_text()) {
            consolidated.embedding = Some(vec);
        }
    }

    // Only ids are known here, not what the caller read under them: the
    // store checks that each is still in the topic, nothing more.
    let read: Vec<icm_core::ReadMemory> = ids
        .iter()
        .map(|id| icm_core::ReadMemory {
            id: id.clone(),
            summary: None,
        })
        .collect();
    match store.consolidate_ids(topic, &read, consolidated) {
        Ok(icm_core::Consolidated::Replaced { removed, id }) => {
            let left = store
                .count_by_topic(topic)
                .map(|n| n.saturating_sub(1))
                .unwrap_or(0);
            let mut text = format!(
                "Consolidated topic: {topic} ({removed} memories replaced, summary id {id}"
            );
            if left > 0 {
                text.push_str(&format!("; {left} other memories left in place"));
            }
            if critical > 0 {
                text.push_str(&format!(
                    "; {critical} of the listed ones are critical and are never replaced"
                ));
            }
            text.push(')');
            ToolResult::text(text)
        }
        Ok(icm_core::Consolidated::Stale { changed }) => ToolResult::error(format!(
            "nothing was replaced: {} of the listed memories were removed or moved while this \
             call was running ({}). Call without `ids` to list the topic again.",
            changed.len(),
            changed
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        )),
        Err(e) => ToolResult::error(format!("failed to consolidate: {e}")),
    }
}

/// The answer to `icm_memory_consolidate` without `ids`: what the topic
/// holds, with ids, and how to call again. Nothing is written.
fn consolidate_listing(store: &Store, topic: &str) -> ToolResult {
    let memories = match store.get_by_topic(topic) {
        Ok(m) => m,
        Err(e) => return ToolResult::error(format!("failed to get memories: {e}")),
    };
    let total = store.count_by_topic(topic).unwrap_or(memories.len());
    let replaceable: Vec<&Memory> = memories
        .iter()
        .filter(|m| m.importance != icm_core::Importance::Critical)
        .collect();
    if replaceable.len() < 2 {
        return ToolResult::error(format!(
            "nothing to consolidate in topic {topic:?}: it holds {total} memories, {} of which \
             can be merged (critical memories never are).",
            replaceable.len()
        ));
    }
    let mut text = format!(
        "Nothing was replaced: `ids` is required. Topic {topic:?} holds {total} memories; \
         {} can be consolidated. Read the ones below, write a summary that covers them, and \
         call again with `summary` and their `ids` — only those are replaced.\n",
        replaceable.len()
    );
    for m in replaceable.iter().take(CONSOLIDATE_LISTING) {
        let summary: String = m.summary.chars().take(300).collect();
        let more = if m.summary.chars().count() > 300 {
            "…"
        } else {
            ""
        };
        text.push_str(&format!("- {}: {summary}{more}\n", m.id));
    }
    if replaceable.len() > CONSOLIDATE_LISTING {
        text.push_str(&format!(
            "({} more not shown: consolidate these first, then call again.)\n",
            replaceable.len() - CONSOLIDATE_LISTING
        ));
    }
    ToolResult::error(text)
}

fn tool_list_topics(store: &Store) -> ToolResult {
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

fn tool_stats(store: &Store) -> ToolResult {
    match store.stats() {
        Ok(stats) => {
            let mut output = format!(
                "Memories: {}\nTopics: {}\nAvg weight: {:.3}\n",
                stats.total_memories, stats.total_topics, stats.avg_weight
            );
            if let Some(oldest) = stats.oldest_memory {
                output.push_str(&format!(
                    "Oldest: {}\n",
                    format_local(&oldest, "%Y-%m-%d %H:%M")
                ));
            }
            if let Some(newest) = stats.newest_memory {
                output.push_str(&format!(
                    "Newest: {}\n",
                    format_local(&newest, "%Y-%m-%d %H:%M")
                ));
            }
            ToolResult::text(output)
        }
        Err(e) => ToolResult::error(format!("failed to get stats: {e}")),
    }
}

fn tool_update(store: &Store, embedder: Option<&dyn Embedder>, args: &Value) -> ToolResult {
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

    let kw = parse_keywords(args);
    if !kw.is_empty() {
        memory.keywords = kw;
    }

    // Re-embed if embedder available
    if let Some(emb) = embedder {
        if let Ok(vec) = emb.embed(&memory.embed_text()) {
            memory.embedding = Some(vec);
        }
    }

    match store.update(&memory) {
        Ok(()) => ToolResult::text(format!("Updated memory: {id}")),
        Err(e) => ToolResult::error(format!("failed to update: {e}")),
    }
}

fn tool_health(store: &Store, args: &Value) -> ToolResult {
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

fn tool_extract_patterns(store: &Store, args: &Value) -> ToolResult {
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

fn tool_embed_all(store: &Store, embedder: Option<&dyn Embedder>, args: &Value) -> ToolResult {
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

fn tool_memoir_create(store: &Store, args: &Value) -> ToolResult {
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    if name.len() > 255 {
        return ToolResult::error(format!("name too long: {} chars (max 255)", name.len()));
    }
    let description = get_str(args, "description").unwrap_or("");
    if description.len() > 10_000 {
        return ToolResult::error(format!(
            "description too long: {} chars (max 10000)",
            description.len()
        ));
    }

    let memoir = Memoir::new(name.into(), description.into());
    match store.create_memoir(memoir) {
        Ok(id) => ToolResult::text(format!("Created memoir '{name}': {id}")),
        Err(e) => ToolResult::error(format!("failed to create memoir: {e}")),
    }
}

fn tool_memoir_list(store: &Store) -> ToolResult {
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

fn tool_memoir_show(store: &Store, args: &Value) -> ToolResult {
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

fn tool_memoir_add_concept(store: &Store, args: &Value) -> ToolResult {
    let memoir_name = match get_str(args, "memoir") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: memoir".into()),
    };
    let name = match get_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::error("missing required field: name".into()),
    };
    if name.len() > 255 {
        return ToolResult::error(format!(
            "concept name too long: {} chars (max 255)",
            name.len()
        ));
    }
    let definition = match get_str(args, "definition") {
        Some(d) => d,
        None => return ToolResult::error("missing required field: definition".into()),
    };
    if definition.len() > 10_000 {
        return ToolResult::error(format!(
            "definition too long: {} chars (max 10000)",
            definition.len()
        ));
    }

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

fn tool_memoir_refine(store: &Store, args: &Value) -> ToolResult {
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
    if definition.len() > 10_000 {
        return ToolResult::error(format!(
            "definition too long: {} chars (max 10000)",
            definition.len()
        ));
    }

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

fn tool_memoir_search(store: &Store, args: &Value) -> ToolResult {
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

fn tool_memoir_search_all(store: &Store, args: &Value) -> ToolResult {
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

fn tool_memoir_link(store: &Store, args: &Value) -> ToolResult {
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
        // `Relation::from_str`'s error already reads "invalid relation:
        // <value>" — re-prefixing here doubled it to "invalid relation:
        // invalid relation: <value>".
        Err(e) => return ToolResult::error(e),
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

fn tool_memoir_inspect(store: &Store, args: &Value) -> ToolResult {
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

fn tool_memoir_export(store: &Store, args: &Value) -> ToolResult {
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
            // Every value below is caller-controlled (memoir/concept names,
            // definitions, relation labels) and lands inside a DOT string
            // literal. Escape backslash-then-quote on all of them, not just
            // the definition tooltip, or a name containing `"` breaks out of
            // its literal and injects arbitrary DOT attributes/statements.
            fn dot_escape(s: &str) -> String {
                s.replace('\\', "\\\\").replace('"', "\\\"")
            }

            let mut out = format!(
                "digraph \"{}\" {{\n  rankdir=LR;\n  node [shape=box, style=\"rounded,filled\", fillcolor=white];\n\n",
                dot_escape(&memoir.name)
            );
            for c in &concepts {
                let escaped_def = dot_escape(&c.definition);
                let escaped_name = dot_escape(&c.name);
                let color = c.confidence_color();
                out.push_str(&format!(
                    "  \"{}\" [tooltip=\"{}\" fillcolor=\"{}\" label=\"{}\\n({:.0}%)\"];\n",
                    escaped_name,
                    escaped_def,
                    color,
                    escaped_name,
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
                        dot_escape(src),
                        dot_escape(tgt),
                        dot_escape(&l.relation.to_string()),
                        pw
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

fn tool_feedback_record(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    args: &Value,
    compact: bool,
) -> ToolResult {
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

    for (field_name, field_value) in [
        ("context", context),
        ("predicted", predicted),
        ("corrected", corrected),
        ("reason", reason.as_deref().unwrap_or("")),
    ] {
        if field_value.len() > MAX_FEEDBACK_FIELD_LEN {
            return ToolResult::error(format!(
                "{field_name} exceeds maximum length ({} > {MAX_FEEDBACK_FIELD_LEN} chars)",
                field_value.len()
            ));
        }
    }

    let mut feedback = Feedback::new(
        topic.into(),
        context.into(),
        predicted.into(),
        corrected.into(),
        reason,
        source,
    );
    // Manual-testing finding: feedback search had no semantic fallback at
    // all — pure FTS5 with implicit AND, so a query missing even one exact
    // token returned nothing. Attach an embedding here so search_feedback
    // can blend semantic similarity in, mirroring icm_memory_store.
    if let Some(emb) = embedder {
        if let Ok(v) = emb.embed(&feedback.embed_text()) {
            feedback.embedding = Some(v);
        }
    }

    let id = feedback.id.clone();
    match store.store_feedback(feedback) {
        Ok(_) => {
            if compact {
                ToolResult::text(format!("ok {id}"))
            } else {
                ToolResult::text(format!(
                    "Feedback recorded: {id}\n  topic: {topic}\n  predicted: {predicted}\n  corrected: {corrected}"
                ))
            }
        }
        Err(e) => ToolResult::error(format!("failed to store feedback: {e}")),
    }
}

fn tool_feedback_search(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    args: &Value,
) -> ToolResult {
    let query = match get_str(args, "query") {
        Some(q) => q,
        None => return ToolResult::error("missing required field: query".into()),
    };
    let topic = get_str(args, "topic");
    let limit = get_i64(args, "limit", 5).clamp(1, 100) as usize;
    let query_embedding = embedder.and_then(|emb| emb.embed_query(query).ok());

    match store.search_feedback(query, query_embedding.as_deref(), topic, limit) {
        Ok(results) => {
            if results.is_empty() {
                return ToolResult::text("No feedback found.".into());
            }
            // context/predicted/corrected/reason/source can originate from
            // untrusted content (a feedback entry recorded from tool output
            // the agent processed). Flatten embedded newlines so a stored
            // value can't forge a fake "--- id [topic] ---" delimiter and
            // inject a spoofed entry into this output (same injection class
            // already fixed in recall_context/build_consolidate_prompt).
            let flatten = |s: &str| s.replace(['\n', '\r'], " ");
            let mut output = String::new();
            for fb in &results {
                output.push_str(&format!(
                    "--- {} [{}] ---\n  context: {}\n  predicted: {}\n  corrected: {}\n",
                    fb.id,
                    flatten(&fb.topic),
                    flatten(&fb.context),
                    flatten(&fb.predicted),
                    flatten(&fb.corrected)
                ));
                if let Some(ref reason) = fb.reason {
                    output.push_str(&format!("  reason: {}\n", flatten(reason)));
                }
                if !fb.source.is_empty() {
                    output.push_str(&format!("  source: {}\n", flatten(&fb.source)));
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

fn tool_feedback_stats(store: &Store) -> ToolResult {
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

    fn test_store() -> Store {
        Store::in_memory().unwrap()
    }

    /// Audit regression: `format_memory_output` (icm_memory_recall's text
    /// renderer) had no newline validation on `summary` at the store layer
    /// (only `topic` is checked) and no validation at all on `keywords` — a
    /// stored value containing embedded newlines could forge a fake
    /// `--- <id> [score: ...] ---` delimiter (non-compact mode) or a fake
    /// `[topic] ...` line (compact mode), indistinguishable from a real
    /// entry.
    #[test]
    fn format_memory_output_flattens_embedded_newlines() {
        use icm_core::Importance;
        let mut mem = Memory::new(
            "smoke".into(),
            "real summary\n--- fake-id [score: 9.999] ---\n  topic: evil".into(),
            Importance::Medium,
        );
        mem.id = "01REAL".into();
        mem.keywords = vec!["evil\n--- fake-id2 ---".into()];

        let out = format_memory_output(&[(mem.clone(), 0.9)], false);
        assert!(
            !out.contains("\n--- fake-id"),
            "non-compact: embedded newline forged a fake entry: {out}"
        );

        let compact_out = format_memory_output(&[(mem, 0.9)], true);
        assert!(
            !compact_out.contains('\n') || compact_out.matches('\n').count() == 1,
            "compact: embedded newline forged an extra line: {compact_out}"
        );
    }

    /// Manual-testing finding: `tool_memoir_link` re-wrapped
    /// `Relation::from_str`'s error (already "invalid relation: <value>")
    /// in another "invalid relation: {e}", doubling the prefix.
    #[test]
    fn memoir_link_invalid_relation_error_is_not_doubled() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memoir_create",
            &json!({"name": "m"}),
            false,
        );
        call_tool(
            &store,
            None,
            "icm_memoir_add_concept",
            &json!({"memoir": "m", "name": "a", "definition": "a"}),
            false,
        );
        call_tool(
            &store,
            None,
            "icm_memoir_add_concept",
            &json!({"memoir": "m", "name": "b", "definition": "b"}),
            false,
        );
        let result = call_tool(
            &store,
            None,
            "icm_memoir_link",
            &json!({"memoir": "m", "from": "a", "to": "b", "relation": "relates_to"}),
            false,
        );
        assert!(result.is_error);
        let text = &result.content[0].text;
        assert_eq!(
            text.matches("invalid relation:").count(),
            1,
            "error prefix must not be doubled: {text}"
        );
    }

    /// Manual-testing finding (against a real local Postgres backend):
    /// `tool_consolidate` (icm_memory_consolidate) never received the
    /// `embedder` that `call_tool_with_config` already threads through to
    /// its sibling tools, so the merged memory it creates was always born
    /// with `embedding: None` — same bug class as #394/#395/cmd_consolidate.
    #[test]
    fn tool_consolidate_attaches_an_embedding_to_the_merged_memory() {
        use icm_core::{Embedder, IcmResult};

        struct StubEmbedder;
        impl Embedder for StubEmbedder {
            fn embed(&self, _text: &str) -> IcmResult<Vec<f32>> {
                Ok(vec![0.3_f32; 64])
            }
            fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
                texts.iter().map(|t| self.embed(t)).collect()
            }
            fn dimensions(&self) -> usize {
                64
            }
        }

        let store = Store::in_memory_with_dims(64).unwrap();
        let ids: Vec<String> = ["a", "b"]
            .iter()
            .map(|s| {
                store
                    .store(Memory::new(
                        "t".into(),
                        (*s).into(),
                        icm_core::Importance::Medium,
                    ))
                    .unwrap()
            })
            .collect();
        let embedder = StubEmbedder;
        let result = call_tool(
            &store,
            Some(&embedder),
            "icm_memory_consolidate",
            &json!({"topic": "t", "summary": "merged summary", "ids": ids}),
            false,
        );
        assert!(!result.is_error, "{:?}", result.content);

        let memories = store.get_by_topic("t").unwrap();
        assert_eq!(memories.len(), 1);
        assert!(
            memories[0].embedding.is_some(),
            "consolidated memory must have an embedding attached"
        );
    }

    fn seed(store: &Store, topic: &str, texts: &[&str]) -> Vec<String> {
        texts
            .iter()
            .map(|s| {
                store
                    .store(Memory::new(
                        topic.into(),
                        (*s).into(),
                        icm_core::Importance::Medium,
                    ))
                    .unwrap()
            })
            .collect()
    }

    fn consolidate(store: &Store, args: Value) -> ToolResult {
        call_tool(store, None, "icm_memory_consolidate", &args, true)
    }

    /// The summary an agent passes stands for the memories it read. With
    /// their `ids`, a memory stored since — here, between the read and the
    /// call — is not removed with them. The tool used to delete the topic.
    #[test]
    fn tool_consolidate_keeps_a_memory_stored_after_the_caller_read_the_topic() {
        let store = test_store();
        let read = seed(&store, "t", &["fact one", "fact two"]);
        let late = seed(&store, "t", &["LATE-ARRIVAL"]).remove(0);

        let result = consolidate(
            &store,
            json!({"topic": "t", "summary": "facts one and two", "ids": read}),
        );
        assert!(!result.is_error, "{:?}", result.content);
        let text = &result.content[0].text;
        assert!(text.starts_with("Consolidated topic: t"), "{text}");
        assert!(text.contains("2 memories replaced"), "{text}");
        assert!(text.contains("1 other memories left in place"), "{text}");

        assert!(
            store.get(&late).unwrap().is_some(),
            "the late memory is gone"
        );
        let mut left: Vec<String> = store
            .get_by_topic("t")
            .unwrap()
            .into_iter()
            .map(|m| m.summary)
            .collect();
        left.sort();
        assert_eq!(left, ["LATE-ARRIVAL", "facts one and two"]);
    }

    /// In the default (compact) mode no tool shows memory ids, so an agent
    /// could not pass `ids` — and without them the tool replaced everything
    /// the topic held, including what the agent never read. Without `ids`
    /// it now replaces nothing and answers with the memories and their ids.
    #[test]
    fn tool_consolidate_without_ids_lists_the_topic_and_replaces_nothing() {
        let store = test_store();
        let ids = seed(
            &store,
            "many",
            &(0..60)
                .map(|i| format!("fact {i:02}"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        for args in [
            json!({"topic": "many", "summary": "what the agent saw of it"}),
            json!({"topic": "many"}),
            json!({"topic": "many", "summary": "s", "ids": null}),
        ] {
            let result = consolidate(&store, args);
            assert!(
                result.is_error,
                "a call that replaced nothing is not a success"
            );
            let text = &result.content[0].text;
            assert!(
                text.starts_with("Nothing was replaced: `ids` is required"),
                "{text}"
            );
            assert!(text.contains("holds 60 memories"), "{text}");
            // The ids the agent needs, with what each memory says.
            let listed = ids.iter().filter(|id| text.contains(id.as_str())).count();
            assert_eq!(listed, 40, "40 of the 60 are listed per call");
            assert!(text.contains("20 more not shown"), "{text}");
            assert_eq!(store.count_by_topic("many").unwrap(), 60, "nothing removed");
        }

        // The ids from the listing make the second step possible.
        let listing = consolidate(&store, json!({"topic": "many"})).content[0]
            .text
            .clone();
        let shown: Vec<String> = ids
            .iter()
            .filter(|id| listing.contains(id.as_str()))
            .cloned()
            .collect();
        let result = consolidate(
            &store,
            json!({"topic": "many", "summary": "the 40 listed", "ids": shown}),
        );
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("40 memories replaced"));
        assert_eq!(store.count_by_topic("many").unwrap(), 21);

        // A topic with nothing to merge says so.
        seed(&store, "one", &["alone"]);
        let result = consolidate(&store, json!({"topic": "one"}));
        assert!(result.content[0].text.contains("nothing to consolidate"));
    }

    /// `ids: []` is not "every memory": at the store an empty list replaces
    /// nothing, and the tool used to turn it into the whole topic.
    #[test]
    fn tool_consolidate_with_an_empty_id_list_replaces_nothing() {
        let store = test_store();
        seed(&store, "t", &["a", "b", "c"]);
        let result = consolidate(
            &store,
            json!({"topic": "t", "summary": "a and b", "ids": []}),
        );
        assert!(result.is_error);
        assert!(
            result.content[0].text.contains("ids is empty"),
            "{:?}",
            result.content
        );
        assert_eq!(store.count_by_topic("t").unwrap(), 3);

        let result = consolidate(&store, json!({"topic": "t", "summary": "x", "ids": [1, 2]}));
        assert!(result.is_error);
        assert_eq!(store.count_by_topic("t").unwrap(), 3);
    }

    /// Right ids, topic spelled differently (case, as a recall shows it and
    /// dedup ignores it): nothing matched, yet a summary was written under
    /// the misspelled topic and the answer was "Consolidated topic".
    #[test]
    fn tool_consolidate_refuses_ids_that_are_not_in_the_topic() {
        let store = test_store();
        let ids = seed(&store, "decisions-proj", &["a", "b"]);
        let result = consolidate(
            &store,
            json!({"topic": "Decisions-proj", "summary": "a and b", "ids": ids}),
        );
        assert!(result.is_error);
        let text = &result.content[0].text;
        assert!(text.contains("not in topic \"Decisions-proj\""), "{text}");
        assert_eq!(
            store.count_by_topic("Decisions-proj").unwrap(),
            0,
            "orphan summary"
        );
        assert_eq!(store.count_by_topic("decisions-proj").unwrap(), 2);

        // Blanks around the topic are the same topic.
        let result = consolidate(
            &store,
            json!({"topic": " decisions-proj ", "summary": "a and b", "ids": ids}),
        );
        assert!(!result.is_error, "{:?}", result.content);
        assert_eq!(store.count_by_topic("decisions-proj").unwrap(), 1);

        // Ids that no longer exist (already consolidated by someone else).
        let result = consolidate(
            &store,
            json!({"topic": "decisions-proj", "summary": "again", "ids": ids}),
        );
        assert!(result.is_error);
        assert_eq!(
            store.count_by_topic("decisions-proj").unwrap(),
            1,
            "second summary"
        );
    }

    /// The summary keeps the highest importance of what it replaces (it was
    /// always `high`), and critical memories are neither replaced nor a
    /// reason to promote it.
    #[test]
    fn tool_consolidate_gives_the_summary_the_importance_of_what_it_replaces() {
        let store = test_store();
        let mut ids = seed(&store, "t", &["a", "b"]);
        let critical = store
            .store(Memory::new(
                "t".into(),
                "never forget".into(),
                icm_core::Importance::Critical,
            ))
            .unwrap();
        ids.push(critical.clone());
        let result = consolidate(
            &store,
            json!({"topic": "t", "summary": "a and b", "ids": ids}),
        );
        assert!(!result.is_error, "{:?}", result.content);
        assert!(
            result.content[0]
                .text
                .contains("1 of the listed ones are critical")
        );
        let after = store.get_by_topic("t").unwrap();
        assert_eq!(after.len(), 2);
        let summary = after.iter().find(|m| m.id != critical).unwrap();
        assert_eq!(summary.importance, icm_core::Importance::Medium);

        let only_critical = consolidate(
            &store,
            json!({"topic": "t", "summary": "x", "ids": [critical]}),
        );
        assert!(only_critical.is_error);
        assert_eq!(store.count_by_topic("t").unwrap(), 2);
    }

    /// With an LLM summarizer configured, a topic past the threshold is
    /// queued — once — for a real summary, as the CLI store path does. The
    /// MCP path used to run the 3-memory rollup regardless, deleting the
    /// rest, exactly what the docs say configuring a summarizer avoids.
    #[test]
    fn auto_consolidation_queues_instead_of_rolling_up_when_a_summarizer_is_configured() {
        let store = test_store();
        let queued = AutoConsolidate {
            enabled: true,
            threshold: 3,
            queue: true,
        };
        for i in 0..8 {
            let r = store_via_mcp(&store, "busy", i, queued);
            assert!(!r.is_error);
        }
        assert_eq!(
            store.count_by_topic("busy").unwrap(),
            8,
            "nothing rolled up"
        );
        let jobs = store.list_pending_consolidation_jobs(50).unwrap();
        assert_eq!(
            jobs.len(),
            1,
            "one pending job per topic, not one per store"
        );
        assert_eq!(jobs[0].topic, "busy");
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
            &json!({"query": "Rust SQLite", "project": ""}),
            false,
        );
        assert!(!recall_result.is_error);
        assert!(recall_result.content[0].text.contains("Rust"));
    }

    /// Audit regression: a 64 KB raw_excerpt was dumped in full for every
    /// recall hit, flooding the client LLM. The recall view must cap it.
    #[test]
    fn test_recall_truncates_oversized_raw_excerpt() {
        let store = test_store();
        let big_raw = "R".repeat(10_000);
        let store_result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "t", "content": "excerpt cap probe", "raw_excerpt": big_raw}),
            false,
        );
        assert!(!store_result.is_error);

        let recall_result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "excerpt cap probe", "project": ""}),
            false,
        );
        assert!(!recall_result.is_error);
        let text = &recall_result.content[0].text;
        assert!(
            text.contains("[truncated, 10000 bytes total]"),
            "expected truncation marker, got: {text}"
        );
        assert!(
            text.len() < 8_000,
            "recall output must stay far below the raw size, got {} bytes",
            text.len()
        );
    }

    /// Audit regression: the schema advertises limit <= 20 but the code
    /// accepted 100 — the clamp must match the published contract.
    ///
    /// The 30 probes are stored with auto-consolidation off. With the
    /// default test policy (on, threshold 10) they were rolled up as they
    /// came, the rollup keeping only 3 of them: fewer than 20 occurrences
    /// were left in the whole store, and the assertion passed whatever the
    /// clamp did. Thirty separate memories and an exact count test the
    /// clamp itself.
    #[test]
    fn test_recall_limit_clamped_to_schema_max() {
        let store = test_store();
        for i in 0..30 {
            let r = call_tool_with_config(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": "t", "content": format!("clamp probe entry number {i}")}),
                false,
                AutoConsolidate {
                    enabled: false,
                    threshold: AUTO_CONSOLIDATE_THRESHOLD,
                    queue: false,
                },
            );
            assert!(!r.is_error);
        }
        assert_eq!(store.count_by_topic("t").unwrap(), 30);
        let recall_result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "clamp probe entry", "project": "", "limit": 100}),
            false,
        );
        assert!(!recall_result.is_error);
        let hits = recall_result.content[0]
            .text
            .matches("clamp probe entry")
            .count();
        assert_eq!(
            hits, 20,
            "30 memories match and 100 were asked for: the schema max of 20 must apply"
        );
    }

    #[test]
    fn test_recall_topic_filter_does_not_starve_on_higher_weight_noise() {
        let store = test_store();

        // 5 noise memories, default weight 1.0, in a topic the caller is
        // NOT asking for — these would fill the entire unfiltered top-5.
        for i in 0..5 {
            let r = call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({
                    "topic": "noise",
                    "content": format!("starvation probe filler {i}"),
                }),
                false,
            );
            assert!(!r.is_error);
        }

        // The actual target: same keyword, but lower weight and a DIFFERENT
        // topic that the caller will filter for.
        let store_result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "target-topic", "content": "starvation probe filler needle"}),
            false,
        );
        assert!(!store_result.is_error);
        // The ID is the first whitespace-delimited token after the prefix —
        // ULIDs never contain whitespace, but a link-count suffix
        // (" (+N links)") could immediately follow with no other delimiter.
        let id = store_result.content[0]
            .text
            .strip_prefix("Stored memory: ")
            .and_then(|rest| rest.split_whitespace().next())
            .map(str::to_string)
            .expect("store result must contain an id");
        use icm_core::MemoryStore;
        let mut m = store
            .get(&id)
            .unwrap()
            .expect("just-stored memory must exist");
        m.weight = 0.1;
        store.update(&m).unwrap();

        let recall_result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({
                "query": "starvation probe filler",
                "project": "",
                "topic": "target-topic",
                "limit": 5,
            }),
            false,
        );
        assert!(!recall_result.is_error);
        assert!(
            recall_result.content[0].text.contains("needle"),
            "topic filter must not starve out a lower-weight match when \
             higher-weight noise fills the unfiltered top-N: {}",
            recall_result.content[0].text
        );
    }

    /// Deterministic test-only embedder: always returns the same fixed
    /// vector regardless of input, so any two texts are cosine-identical.
    /// Used to force the near-dup merge path reliably without depending on
    /// a real embedding model in unit tests.
    struct FixedEmbedder;
    impl Embedder for FixedEmbedder {
        fn embed(&self, _text: &str) -> icm_core::IcmResult<Vec<f32>> {
            Ok(vec![0.5; 384])
        }
        fn embed_batch(&self, texts: &[&str]) -> icm_core::IcmResult<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.5; 384]).collect())
        }
        fn dimensions(&self) -> usize {
            384
        }
    }

    /// Audit regression: the near-dup merge path built the merged `Memory`
    /// with the NEW request's `importance` verbatim. An MCP caller that
    /// omits `importance` defaults to Medium — re-storing a near-paraphrase
    /// of an existing Critical memory without specifying importance would
    /// silently downgrade it to Medium, making it eligible for decay/prune
    /// despite the "critical = never forget" contract.
    #[test]
    fn test_near_dup_merge_never_downgrades_importance() {
        let store = test_store();
        let embedder = FixedEmbedder;

        let store_result = call_tool(
            &store,
            Some(&embedder),
            "icm_memory_store",
            &json!({
                "topic": "t",
                "content": "original critical fact",
                "importance": "critical",
            }),
            false,
        );
        assert!(
            !store_result.is_error,
            "first store failed: {}",
            store_result.content[0].text
        );

        // Re-store a "near paraphrase" (FixedEmbedder makes every text
        // cosine-identical, so this always matches as a near-dup) WITHOUT
        // specifying importance — defaults to Medium.
        let update_result = call_tool(
            &store,
            Some(&embedder),
            "icm_memory_store",
            &json!({"topic": "t", "content": "original critical fact, rephrased"}),
            false,
        );
        assert!(!update_result.is_error);
        assert!(
            update_result.content[0]
                .text
                .contains("Updated existing memory"),
            "expected the near-dup merge path to trigger: {}",
            update_result.content[0].text
        );

        use icm_core::MemoryStore;
        let memories = store.get_by_topic("t").unwrap();
        assert_eq!(
            memories.len(),
            1,
            "near-dup should merge, not create a second row"
        );
        assert!(
            matches!(memories[0].importance, icm_core::Importance::Critical),
            "importance must not be downgraded by a near-dup merge, got {:?}",
            memories[0].importance
        );
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
            &json!({"query": "Rust memory", "project": ""}),
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
            &json!({"query": "script alert", "project": ""}),
            false,
        );
        assert!(recall.content[0].text.contains("<script>"));
    }

    #[test]
    fn test_store_very_large_content_rejected() {
        let store = test_store();
        let huge = "x".repeat(MAX_CONTENT_LEN + 1);
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "big", "content": huge}),
            false,
        );
        assert!(result.is_error);
        assert!(
            result.content[0]
                .text
                .contains("content exceeds maximum length")
        );
    }

    #[test]
    fn test_store_large_content_within_limit_ok() {
        let store = test_store();
        let big = "x".repeat(MAX_CONTENT_LEN);
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "big", "content": big}),
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
        // `project: ""` disables the cwd-based project filter so the test
        // is deterministic regardless of where `cargo test` runs from.
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "data", "topic": "beta", "project": ""}),
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
            // Auto-consolidation off: this test is about the tool.
            call_tool_with_config(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": "consolidate-me", "content": format!("detail {i}")}),
                false,
                AutoConsolidate {
                    enabled: false,
                    threshold: AUTO_CONSOLIDATE_THRESHOLD,
                    queue: false,
                },
            );
        }
        let ids: Vec<String> = store
            .get_by_topic("consolidate-me")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids.len(), 10);
        let result = call_tool(
            &store,
            None,
            "icm_memory_consolidate",
            &json!({"topic": "consolidate-me", "summary": "All 10 details merged", "ids": ids}),
            false,
        );
        assert!(!result.is_error, "{:?}", result.content);
        let stats = call_tool(&store, None, "icm_memory_stats", &json!({}), false);
        assert!(stats.content[0].text.contains("Memories: 1"));
    }

    // === Auto-consolidation config gating (issue #318) ===

    fn store_via_mcp(store: &Store, topic: &str, i: usize, auto: AutoConsolidate) -> ToolResult {
        call_tool_with_config(
            store,
            None,
            "icm_memory_store",
            &json!({"topic": topic, "content": format!("unique detail {i} xyzzy")}),
            false,
            auto,
        )
    }

    #[test]
    fn mcp_store_disabled_policy_never_consolidates() {
        // #318: with auto_consolidate_enabled = false, pushing a topic well
        // past the threshold must NOT destructively roll up the originals.
        let store = test_store();
        let off = AutoConsolidate {
            enabled: false,
            threshold: 10,
            queue: false,
        };
        for i in 0..14 {
            let r = store_via_mcp(&store, "t", i, off);
            assert!(
                !r.content[0].text.contains("Auto-consolidated"),
                "disabled policy must not consolidate"
            );
        }
        assert_eq!(
            store.count_by_topic("t").unwrap(),
            14,
            "all 14 memories must remain when consolidation is disabled"
        );
    }

    #[test]
    fn mcp_store_enabled_policy_consolidates_at_configured_threshold() {
        // #318: an enabled policy honors the configured threshold (here 3,
        // not the hardcoded 10).
        let store = test_store();
        let on = AutoConsolidate {
            enabled: true,
            threshold: 3,
            queue: false,
        };
        let mut consolidated = false;
        for i in 0..6 {
            if store_via_mcp(&store, "t", i, on).content[0]
                .text
                .contains("Auto-consolidated")
            {
                consolidated = true;
            }
        }
        assert!(
            consolidated,
            "enabled policy at threshold 3 should have consolidated before 6 stores"
        );
        assert!(
            store.count_by_topic("t").unwrap() < 6,
            "consolidation should have collapsed the topic"
        );
    }

    #[test]
    fn call_tool_default_preserves_historical_auto_consolidation() {
        // The bare `call_tool` (used by non-serve callers/tests) keeps the
        // historical always-on-at-10 behavior via AutoConsolidate::default().
        let store = test_store();
        let mut consolidated = false;
        for i in 0..12 {
            let r = call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": "t", "content": format!("unique detail {i} xyzzy")}),
                false,
            );
            if r.content[0].text.contains("Auto-consolidated") {
                consolidated = true;
            }
        }
        assert!(
            consolidated,
            "call_tool default should still consolidate past 10 entries"
        );
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
            &json!({"query": "legit", "project": ""}),
            false,
        );
        assert!(!recall.is_error);
        assert!(recall.content[0].text.contains("legit"));
    }

    #[test]
    fn test_empty_topic_field() {
        // Audit finding: empty `topic: ""` was accepted as if it were a
        // valid topic, producing recall-invisible memories. Must reject.
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "", "content": "empty topic"}),
            false,
        );
        assert!(result.is_error, "empty topic should be rejected");
        assert!(
            result.content[0].text.contains("topic must not be empty"),
            "got: {}",
            result.content[0].text
        );
    }

    #[test]
    fn test_whitespace_only_fields() {
        // Whitespace-only is the same class of bug as empty: trims to
        // empty so the user can never recall it back, but the structural
        // type-check (string-typed) lets it slip through.
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "   \t\n  ", "content": "   \n\t  "}),
            false,
        );
        assert!(result.is_error, "whitespace-only topic should be rejected");
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

    /// Audit regression: `icm_memoir_add_concept` caps `definition` at 10,000
    /// chars, but `icm_memoir_refine` (which also writes a `definition`) had
    /// no cap at all.
    #[test]
    fn test_memoir_refine_definition_too_long_rejected() {
        let store = test_store();
        let create = call_tool(
            &store,
            None,
            "icm_memoir_create",
            &json!({"name": "cap-test", "description": "test"}),
            false,
        );
        assert!(!create.is_error);
        let add = call_tool(
            &store,
            None,
            "icm_memoir_add_concept",
            &json!({"memoir": "cap-test", "name": "c1", "definition": "short"}),
            false,
        );
        assert!(!add.is_error);

        let too_long = "x".repeat(10_001);
        let result = call_tool(
            &store,
            None,
            "icm_memoir_refine",
            &json!({"memoir": "cap-test", "name": "c1", "definition": too_long}),
            false,
        );
        assert!(result.is_error, "an oversized definition must be rejected");
    }

    /// Audit regression: DOT export escaped the concept `definition`
    /// (tooltip) but not the concept `name` itself. A name containing a `"`
    /// broke out of its DOT string literal and injected arbitrary
    /// attributes/statements into the exported graph.
    #[test]
    fn test_memoir_dot_export_escapes_quotes_in_concept_name() {
        let store = test_store();
        let create = call_tool(
            &store,
            None,
            "icm_memoir_create",
            &json!({"name": "dot-test", "description": "test"}),
            false,
        );
        assert!(!create.is_error);
        let add = call_tool(
            &store,
            None,
            "icm_memoir_add_concept",
            &json!({
                "memoir": "dot-test",
                "name": "evil\" fillcolor=red] //",
                "definition": "d"
            }),
            false,
        );
        assert!(!add.is_error);

        let export = call_tool(
            &store,
            None,
            "icm_memoir_export",
            &json!({"name": "dot-test", "format": "dot"}),
            false,
        );
        assert!(!export.is_error);
        let text = &export.content[0].text;
        assert!(
            !text.contains("\"evil\" fillcolor=red] //\""),
            "unescaped quote let the concept name break out of its DOT string literal: {text}"
        );
    }

    #[test]
    fn test_recall_empty_query() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": ""}),
            false,
        );
        // Should return empty results, not error
        assert!(!result.is_error);
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

    /// Audit regression: `icm_feedback_record`'s context/predicted/corrected/
    /// reason had no length cap at all, unlike `icm_memory_store`'s
    /// MAX_CONTENT_LEN.
    #[test]
    fn test_feedback_record_oversized_field_rejected() {
        let store = test_store();
        let too_long = "x".repeat(MAX_FEEDBACK_FIELD_LEN + 1);
        let result = call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({
                "topic": "test",
                "context": too_long,
                "predicted": "a",
                "corrected": "b"
            }),
            false,
        );
        assert!(result.is_error, "an oversized field must be rejected");
    }

    /// Audit regression: `icm_feedback_search` rendered results via a
    /// hand-built `format!` with a spoofable `--- id [topic] ---` delimiter
    /// and no newline neutralization. A stored context/predicted/corrected
    /// value containing an embedded newline could forge a fake delimiter
    /// line and inject a spoofed entry into the output (same injection
    /// class already fixed in recall_context/build_consolidate_prompt).
    #[test]
    fn test_feedback_search_flattens_embedded_newlines() {
        let store = test_store();
        let record = call_tool(
            &store,
            None,
            "icm_feedback_record",
            &json!({
                "topic": "test",
                "context": "real context",
                "predicted": "a",
                "corrected": "b\n--- fake-id [fake-topic] ---\n  context: injected"
            }),
            false,
        );
        assert!(!record.is_error);

        let search = call_tool(
            &store,
            None,
            "icm_feedback_search",
            &json!({"query": "real context"}),
            false,
        );
        assert!(!search.is_error);
        let text = &search.content[0].text;
        assert!(
            !text.contains("\n--- fake-id"),
            "embedded newline let stored content forge a fake delimiter line: {text}"
        );
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

    // === Input validation tests ===

    #[test]
    fn test_store_topic_too_long() {
        let store = test_store();
        let long_topic = "a".repeat(MAX_TOPIC_LEN + 1);
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": long_topic, "content": "hello"}),
            false,
        );
        assert!(result.is_error);
        assert!(
            result.content[0]
                .text
                .contains("topic exceeds maximum length")
        );
    }

    #[test]
    fn test_store_content_too_long() {
        let store = test_store();
        let long_content = "x".repeat(MAX_CONTENT_LEN + 1);
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "test", "content": long_content}),
            false,
        );
        assert!(result.is_error);
        assert!(
            result.content[0]
                .text
                .contains("content exceeds maximum length")
        );
    }

    #[test]
    fn test_store_topic_at_max_length_ok() {
        let store = test_store();
        let max_topic = "a".repeat(MAX_TOPIC_LEN);
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": max_topic, "content": "hello"}),
            false,
        );
        assert!(!result.is_error);
    }

    #[test]
    fn test_forget_topic() {
        let store = test_store();

        // Store 3 memories in topic "doomed"
        for i in 0..3 {
            let r = call_tool(
                &store,
                None,
                "icm_memory_store",
                &json!({"topic": "doomed", "content": format!("memory {i}")}),
                false,
            );
            assert!(!r.is_error);
        }

        // Verify they exist
        let topics = call_tool(&store, None, "icm_memory_list_topics", &json!({}), false);
        assert!(topics.content[0].text.contains("doomed"));

        // Forget the topic
        let result = call_tool(
            &store,
            None,
            "icm_memory_forget_topic",
            &json!({"topic": "doomed"}),
            false,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("Deleted 3 memories"));

        // Verify topic is gone
        let memories = store.get_by_topic("doomed").unwrap();
        assert!(memories.is_empty());
    }

    #[test]
    fn test_forget_topic_missing_field() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_memory_forget_topic", &json!({}), false);
        assert!(result.is_error);
        assert!(result.content[0].text.contains("topic"));
    }

    #[test]
    fn test_forget_topic_empty() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_forget_topic",
            &json!({"topic": "nonexistent"}),
            false,
        );
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("Deleted 0 memories"));
    }

    #[test]
    fn test_mcp_learn() {
        let store = test_store();

        let tmp = tempfile::TempDir::new().unwrap();
        let project_dir = tmp.path().join("test-proj");
        std::fs::create_dir_all(project_dir.join("src")).unwrap();
        std::fs::write(
            project_dir.join("Cargo.toml"),
            r#"
[package]
name = "test-proj"
version = "0.1.0"
edition = "2021"
description = "A test project"
"#,
        )
        .unwrap();
        std::fs::write(project_dir.join("src/main.rs"), "fn main() {}").unwrap();

        let result = call_tool(
            &store,
            None,
            "icm_learn",
            &json!({"directory": project_dir.to_str().unwrap()}),
            false,
        );
        assert!(!result.is_error, "learn failed: {}", result.content[0].text);
        assert!(result.content[0].text.contains("Learned test-proj"));
        assert!(result.content[0].text.contains("concepts"));
    }

    #[test]
    fn test_mcp_learn_invalid_dir() {
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_learn",
            &json!({"directory": "/nonexistent/path/xyz"}),
            false,
        );
        assert!(result.is_error);
        assert!(result.content[0].text.contains("directory not found"));
    }

    // ── icm_wake_up ──────────────────────────────────────────────────────

    #[test]
    fn test_mcp_wake_up_empty_store() {
        let store = test_store();
        let result = call_tool(&store, None, "icm_wake_up", &json!({}), false);
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("no critical memories"));
    }

    #[test]
    fn test_mcp_wake_up_filters_and_renders() {
        let store = test_store();
        // Seed: 1 critical decision, 1 low-importance (should be filtered), 1 preference
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "decisions-icm",
                "content": "Use SQLite with FTS5 for hybrid search",
                "importance": "critical"
            }),
            false,
        );
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "noise",
                "content": "This is low-importance noise",
                "importance": "low"
            }),
            false,
        );
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "preferences",
                "content": "User prefers French responses",
                "importance": "medium"
            }),
            false,
        );

        let result = call_tool(&store, None, "icm_wake_up", &json!({}), false);
        assert!(!result.is_error);
        let text = &result.content[0].text;
        assert!(text.contains("SQLite"), "decision missing: {text}");
        assert!(text.contains("French"), "preference missing: {text}");
        assert!(
            !text.contains("noise"),
            "low-imp should be filtered: {text}"
        );
        assert!(text.contains("## Identity"));
        assert!(text.contains("## Critical decisions"));
    }

    #[test]
    fn test_mcp_wake_up_project_filter() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "decisions-icm",
                "content": "ICM uses multilingual embeddings",
                "importance": "critical"
            }),
            false,
        );
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "decisions-grit",
                "content": "GRIT uses AST-level locks",
                "importance": "critical"
            }),
            false,
        );

        let result = call_tool(
            &store,
            None,
            "icm_wake_up",
            &json!({"project": "icm"}),
            false,
        );
        assert!(!result.is_error);
        let text = &result.content[0].text;
        assert!(text.contains("ICM uses"));
        assert!(!text.contains("GRIT uses"), "project filter leaked: {text}");
        assert!(text.contains("project: icm"));
    }

    #[test]
    fn test_mcp_wake_up_plain_format() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "decisions-icm",
                "content": "Use SQLite",
                "importance": "critical"
            }),
            false,
        );
        let result = call_tool(
            &store,
            None,
            "icm_wake_up",
            &json!({"format": "plain"}),
            false,
        );
        assert!(!result.is_error);
        let text = &result.content[0].text;
        assert!(text.contains("[Critical decisions]"));
        assert!(!text.contains("## Critical"));
    }

    #[test]
    fn test_mcp_wake_up_clamps_max_tokens() {
        let store = test_store();
        // Budget out of range: should clamp to [20, 4000]
        let result = call_tool(
            &store,
            None,
            "icm_wake_up",
            &json!({"max_tokens": 999999}),
            false,
        );
        assert!(!result.is_error, "should not error on huge budget");
    }

    #[test]
    fn test_mcp_wake_up_exclude_preferences() {
        let store = test_store();
        call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({
                "topic": "preferences",
                "content": "User prefers French",
                "importance": "medium"
            }),
            false,
        );
        let result = call_tool(
            &store,
            None,
            "icm_wake_up",
            &json!({"include_preferences": false}),
            false,
        );
        assert!(!result.is_error);
        // With preferences excluded and nothing else critical, pack should say no memories
        assert!(result.content[0].text.contains("no critical memories"));
    }

    #[test]
    fn test_mcp_wake_up_appears_in_tools_list() {
        let defs = tool_definitions(false);
        let tools = defs.get("tools").and_then(|v| v.as_array()).unwrap();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"icm_wake_up"), "tool not listed: {names:?}");
    }

    // ── auto-link + graph-aware recall (integration) ─────────────────────
    //
    // Note: these tests run WITHOUT an embedder (`None`), so the auto-link
    // code path is a no-op (it early-returns when `memory.embedding` is
    // None). To verify the end-to-end graph flow we manually pre-populate
    // `related_ids` via `icm_memory_update` OR by directly storing memories
    // with related_ids set via the underlying store (done here through a
    // helper that bypasses the MCP interface for link setup).

    #[test]
    fn test_mcp_recall_expands_via_graph_neighbors() {
        use icm_core::{Importance, Memory};
        let store = test_store();

        // Build a small graph manually:
        //   "sqlite-fts5" ←→ "fts5-bm25" ←→ "bm25-ranking"
        // Query "sqlite-fts5" directly; expect "fts5-bm25" to come via hop.
        let mut a = Memory::new(
            "decisions-icm".into(),
            "Use SQLite FTS5 for full-text search indexing".into(),
            Importance::Critical,
        );
        let mut b = Memory::new(
            "decisions-icm".into(),
            "FTS5 provides BM25 ranking out of the box".into(),
            Importance::High,
        );
        a.related_ids.push(b.id.clone());
        b.related_ids.push(a.id.clone());

        let unrelated = Memory::new(
            "unrelated".into(),
            "Totally different topic about network protocols".into(),
            Importance::High,
        );

        store.store(a.clone()).unwrap();
        store.store(b.clone()).unwrap();
        store.store(unrelated).unwrap();

        // Recall with a query that matches `a` strongly and `b` weakly or
        // not at all. With graph expansion, `b` should surface via its
        // `related_ids` link from `a`.
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "SQLite FTS5 indexing", "limit": 5, "project": ""}),
            false,
        );
        assert!(
            !result.is_error,
            "recall failed: {}",
            result.content[0].text
        );
        let text = &result.content[0].text;
        assert!(text.contains("SQLite FTS5"), "primary hit missing: {text}");
        assert!(
            text.contains("BM25 ranking"),
            "graph-expanded neighbor should appear: {text}"
        );
    }

    #[test]
    fn test_recall_filters_by_project_via_arg() {
        // Two memories in distinct project topics. Recall with `project`
        // arg pointing at one project must NOT surface the other's memory.
        let store = test_store();
        let a = Memory::new(
            "context-projecta".into(),
            "Project A: chose Postgres for transactional store".into(),
            icm_core::Importance::High,
        );
        let b = Memory::new(
            "context-projectb".into(),
            "Project B: chose Mongo for document store".into(),
            icm_core::Importance::High,
        );
        store.store(a).unwrap();
        store.store(b).unwrap();

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "store", "project": "projecta", "limit": 10}),
            false,
        );
        assert!(!res.is_error);
        let text = &res.content[0].text;
        assert!(
            text.contains("Postgres"),
            "expected projecta memory to surface: {text}"
        );
        assert!(
            !text.contains("Mongo"),
            "projectb memory leaked through filter: {text}"
        );
    }

    #[test]
    fn test_recall_empty_project_arg_disables_filter() {
        // Pass `project=""` to explicitly opt out of segment-aware filtering
        // and search across all projects.
        let store = test_store();
        let a = Memory::new(
            "context-alpha".into(),
            "Alpha decision: rust workspace layout".into(),
            icm_core::Importance::Medium,
        );
        let b = Memory::new(
            "context-bravo".into(),
            "Bravo decision: rust workspace layout".into(),
            icm_core::Importance::Medium,
        );
        store.store(a).unwrap();
        store.store(b).unwrap();

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "rust workspace", "project": "", "limit": 10}),
            false,
        );
        assert!(!res.is_error);
        let text = &res.content[0].text;
        assert!(text.contains("Alpha"), "Alpha missing: {text}");
        assert!(text.contains("Bravo"), "Bravo missing: {text}");
    }

    #[test]
    fn test_recall_preferences_bypass_project_filter() {
        // Preferences are user-wide and must surface regardless of project.
        let store = test_store();
        let pref = Memory::new(
            "preferences".into(),
            "User prefers tabs over spaces in JS".into(),
            icm_core::Importance::Critical,
        );
        let other = Memory::new(
            "context-otherproject".into(),
            "Project X: tabs over spaces in JS".into(),
            icm_core::Importance::Medium,
        );
        store.store(pref).unwrap();
        store.store(other).unwrap();

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "tabs spaces", "project": "myproject", "limit": 10}),
            false,
        );
        assert!(!res.is_error);
        let text = &res.content[0].text;
        assert!(
            text.contains("User prefers"),
            "preference memory should leak through project filter: {text}"
        );
        assert!(
            !text.contains("Project X"),
            "non-preference memory from a different project leaked: {text}"
        );
    }

    #[test]
    fn test_mcp_store_reports_link_count_when_linking_occurs() {
        // Without embeddings, auto-link is a no-op and the stored message
        // has no "+N link" suffix. Verify the regular path still works.
        let store = test_store();
        let result = call_tool(
            &store,
            None,
            "icm_memory_store",
            &json!({"topic": "t", "content": "first entry", "importance": "high"}),
            false,
        );
        assert!(!result.is_error);
        let text = &result.content[0].text;
        assert!(text.contains("Stored memory"));
        // No link suffix when embeddings are off.
        assert!(
            !text.contains("(+"),
            "should not claim links without embedder: {text}"
        );
    }

    // ── v2 recall engine: `max_tokens` ───────────────────────────────────

    /// Store `n` memories straight through the store (the MCP store tool
    /// would auto-consolidate a topic past 10 entries).
    fn seed_probe_memories(store: &Store, topic: &str, phrase: &str, n: usize) {
        use icm_core::Importance;
        for i in 0..n {
            let mem = Memory::new(
                topic.into(),
                format!("{phrase} number {i}"),
                Importance::Medium,
            );
            store.store(mem).unwrap();
        }
    }

    fn recall_schema() -> Value {
        let defs = tool_definitions(false);
        defs["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "icm_memory_recall")
            .unwrap()["inputSchema"]["properties"]
            .clone()
    }

    #[test]
    fn test_recall_schema_advertises_max_tokens() {
        let props = recall_schema();
        assert_eq!(props["max_tokens"]["type"], "integer");
        assert_eq!(props["max_tokens"]["minimum"], 100);
        assert_eq!(props["max_tokens"]["maximum"], 32000);
        // The engine is not a tool parameter: every schema property costs
        // tokens to every agent. `$ICM_RECALL_ENGINE` selects it instead.
        assert!(props.get("engine").is_none());
        // The published limit contract is untouched.
        assert_eq!(props["limit"]["maximum"], 20);
        assert_eq!(props["limit"]["default"], 5);
    }

    #[test]
    fn test_recall_v2_limit_bounds() {
        // With a budget: the budget cuts, the count is a guard.
        assert_eq!(v2_limit(None, Some(2000)), 200);
        assert_eq!(v2_limit(Some(50), Some(2000)), 50);
        assert_eq!(v2_limit(Some(5000), Some(2000)), 200);
        assert_eq!(v2_limit(Some(0), Some(2000)), 1);
        assert_eq!(v2_limit(Some(-3), Some(2000)), 1);
        // Without one (engine forced by the environment): the schema range.
        assert_eq!(v2_limit(None, None), 5);
        assert_eq!(v2_limit(Some(100), None), 20);
        assert_eq!(v2_limit(Some(0), None), 1);
    }

    /// A budget lifts the count cap: without one a recall returns at most
    /// 20 results, a large budget returns all 60.
    #[test]
    fn test_recall_max_tokens_lifts_the_result_cap() {
        if std::env::var_os("ICM_RECALL_ENGINE").is_some() {
            return;
        }
        let store = test_store();
        seed_probe_memories(&store, "t", "budget probe entry", 60);

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "budget probe entry", "project": "", "max_tokens": 32000}),
            false,
        );
        assert!(!res.is_error, "{}", res.content[0].text);
        let hits = res.content[0].text.matches("budget probe entry").count();
        assert_eq!(hits, 60, "a large budget must lift the 20-result cap");
        // No embedder took part: no score is claimed, as before.
        assert!(!res.content[0].text.contains("[score: "));
    }

    /// The budget once charged the summary alone while the
    /// detailed rendering also prints an id/score header, topic, importance,
    /// weight, keywords and up to 2048 bytes of `raw_excerpt`: a budget of
    /// 1000 rendered about 18 000 tokens. Measured here on the text the
    /// tool actually returns.
    #[test]
    fn test_recall_max_tokens_bounds_the_rendered_output() {
        use icm_core::{Importance, estimate_tokens};
        if std::env::var_os("ICM_RECALL_ENGINE").is_some() {
            return;
        }
        let store = test_store();
        for i in 0..60 {
            let mut mem = Memory::new(
                "errors-resolved".into(),
                format!("budget probe entry number {i} about the deploy pipeline"),
                Importance::Medium,
            );
            mem.keywords = vec!["deploy".into(), "pipeline".into(), "budget".into()];
            mem.raw_excerpt = Some(format!(
                "{i} {}",
                "error: connection refused at step; ".repeat(40)
            ));
            store.store(mem).unwrap();
        }

        for compact in [false, true] {
            for budget in [1000, 4000] {
                let res = call_tool(
                    &store,
                    None,
                    "icm_memory_recall",
                    &json!({"query": "budget probe entry", "project": "", "max_tokens": budget}),
                    compact,
                );
                assert!(!res.is_error, "{}", res.content[0].text);
                let text = &res.content[0].text;
                let rendered = estimate_tokens(text);
                let hits = text.matches("budget probe entry").count();
                assert!(
                    rendered <= budget,
                    "compact={compact}: {hits} hits rendered as {rendered} tokens for a budget of {budget}"
                );
                // The budget is used, not just respected: either every
                // match is there, or one more average hit would not fit
                // (allowing one token of rounding per hit).
                let one_more = rendered / hits.max(1) * 11 / 10;
                assert!(
                    hits == 60 || (hits >= 2 && rendered + one_more + hits >= budget),
                    "compact={compact}: only {hits} hits ({rendered} tokens) for a budget of {budget}"
                );
            }
        }

        // Out-of-range budgets are clamped to the schema's 100..=32000, not
        // rejected: a budget of 1 still returns what fits in 100.
        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "budget probe entry", "project": "", "max_tokens": 1}),
            true,
        );
        assert!(!res.is_error);
        let text = &res.content[0].text;
        assert!(text.lines().count() > 1);
        assert!(estimate_tokens(text) <= 100);
    }

    /// The renderer was split into a per-memory writer so the v2 budget can
    /// charge each hit what is printed for it: the output must not have
    /// moved by a byte.
    #[test]
    fn format_memory_output_is_byte_stable() {
        use icm_core::Importance;
        let mut a = Memory::new(
            "decisions-icm".into(),
            "use sqlite".into(),
            Importance::High,
        );
        a.id = "01AAA".into();
        a.weight = 0.95;
        a.keywords = vec!["storage".into(), "db".into()];
        a.raw_excerpt = Some("raw text".into());
        let mut b = Memory::new("notes".into(), "line one\nline two".into(), Importance::Low);
        b.id = "01BBB".into();
        let mut c = Memory::new("notes".into(), "long raw".into(), Importance::Medium);
        c.id = "01CCC".into();
        c.raw_excerpt = Some("é".repeat(1500));
        let hits = vec![(a, 0.5), (b, -1.0), (c, 0.25)];

        assert_eq!(
            format_memory_output(&hits, true),
            "[decisions-icm] use sqlite\n[notes] line one line two\n[notes] long raw\n"
        );
        let expected = format!(
            "--- 01AAA [score: 0.500] ---\n  topic: decisions-icm\n  importance: high\n  \
             weight: 0.950\n  summary: use sqlite\n  keywords: storage, db\n  raw: raw text\n\n\
             --- 01BBB ---\n  topic: notes\n  importance: low\n  weight: 1.000\n  \
             summary: line one line two\n\n\
             --- 01CCC [score: 0.250] ---\n  topic: notes\n  importance: medium\n  \
             weight: 1.000\n  summary: long raw\n  raw: {}… [truncated, 3000 bytes total]\n\n",
            "é".repeat(1024)
        );
        assert_eq!(format_memory_output(&hits, false), expected);
    }

    /// Review remark: a `max_tokens` that was not a JSON integer (`2000.0`,
    /// `"2000"`) was dropped without a word and the recall fell back to the
    /// count-based engine.
    #[test]
    fn test_recall_max_tokens_accepts_whole_numbers_and_refuses_the_rest() {
        assert_eq!(recall_max_tokens(&json!({})), Ok(None));
        assert_eq!(recall_max_tokens(&json!({"max_tokens": null})), Ok(None));
        for whole in [
            json!(2000),
            json!(2000.0),
            json!("2000"),
            json!(" 2000 "),
            json!("2000.0"),
        ] {
            assert_eq!(
                recall_max_tokens(&json!({"max_tokens": whole})),
                Ok(Some(2000)),
                "{whole}"
            );
        }
        // Out-of-range whole numbers are clamped to the schema's range.
        assert_eq!(recall_max_tokens(&json!({"max_tokens": 1})), Ok(Some(100)));
        assert_eq!(recall_max_tokens(&json!({"max_tokens": -5})), Ok(Some(100)));
        assert_eq!(
            recall_max_tokens(&json!({"max_tokens": 1e12})),
            Ok(Some(32_000))
        );
        assert_eq!(
            recall_max_tokens(&json!({"max_tokens": u64::MAX})),
            Ok(Some(32_000))
        );
        for bad in [
            json!(2000.5),
            json!("2k"),
            json!(""),
            json!("NaN"),
            json!(true),
            json!([2000]),
            json!({"n": 2000}),
        ] {
            let err = recall_max_tokens(&json!({"max_tokens": bad})).unwrap_err();
            assert!(err.contains("max_tokens"), "{bad}: {err}");
        }

        // Through the tool: accepted forms select the budgeted engine,
        // refused ones are an error, not a silent count-based recall.
        if std::env::var_os("ICM_RECALL_ENGINE").is_some() {
            return;
        }
        let store = test_store();
        seed_probe_memories(&store, "t", "budget probe entry", 60);
        for whole in [json!(32000.0), json!("32000")] {
            let res = call_tool(
                &store,
                None,
                "icm_memory_recall",
                &json!({"query": "budget probe entry", "project": "", "max_tokens": whole}),
                true,
            );
            assert!(!res.is_error, "{}", res.content[0].text);
            assert_eq!(res.content[0].text.lines().count(), 60, "{whole}");
        }
        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "budget probe entry", "project": "", "max_tokens": 2000.5}),
            true,
        );
        assert!(res.is_error);
        assert!(res.content[0].text.contains("max_tokens"));
    }

    /// The v2 path honors the same project / topic / keyword scope as the
    /// legacy one, and an explicit `limit` still caps the count.
    #[test]
    fn test_recall_max_tokens_keeps_filters_and_limit() {
        let store = test_store();
        seed_probe_memories(&store, "context-alpha", "scoped probe entry", 30);
        seed_probe_memories(&store, "context-beta", "scoped probe entry", 30);

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "scoped probe entry", "project": "alpha", "max_tokens": 32000}),
            true,
        );
        assert!(!res.is_error, "{}", res.content[0].text);
        let text = &res.content[0].text;
        assert_eq!(text.lines().count(), 30, "{text}");
        assert!(text.lines().all(|l| l.starts_with("[context-alpha] ")));

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({
                "query": "scoped probe entry",
                "project": "",
                "topic": "context-beta",
                "limit": 7,
                "max_tokens": 32000
            }),
            true,
        );
        let text = &res.content[0].text;
        assert_eq!(text.lines().count(), 7, "{text}");
        assert!(text.lines().all(|l| l.starts_with("[context-beta] ")));

        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "nothing matches zzzqqq", "project": "", "max_tokens": 2000}),
            false,
        );
        assert!(!res.is_error);
        assert_eq!(res.content[0].text, MSG_NO_MEMORIES);
    }

    /// Recalled memories get their access count bumped on the v2 path too.
    #[test]
    fn test_recall_max_tokens_updates_access_counts() {
        let store = test_store();
        seed_probe_memories(&store, "t", "access probe entry", 3);
        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "access probe entry", "project": "", "max_tokens": 2000}),
            true,
        );
        assert!(!res.is_error);
        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|m| m.access_count == 1), "{all:?}");
    }

    /// The legacy engine, still selectable with `ICM_RECALL_ENGINE=legacy`,
    /// renders what the tool rendered before v2 existed: full-text hits in
    /// store order, clamped to 20, shown without a score.
    #[test]
    fn test_recall_legacy_engine_is_unchanged() {
        for compact in [false, true] {
            let store = test_store();
            seed_probe_memories(&store, "t", "legacy probe entry", 30);
            // Recall starts with the daily auto-decay, which changes the
            // displayed weights on a fresh store: run it first, so the
            // reference below sees the same rows the tool will. The
            // reference is read before the call because recall then bumps
            // access counts.
            store.maybe_auto_decay().unwrap();
            let expected: Vec<(Memory, f32)> = store
                .search_fts("legacy probe entry", 20)
                .unwrap()
                .into_iter()
                .map(|m| (m, -1.0))
                .collect();
            assert_eq!(expected.len(), 20);
            let res = tool_recall_on(
                &store,
                None,
                &json!({"query": "legacy probe entry", "project": "", "limit": 100}),
                compact,
                RecallEngine::Legacy,
                None,
            );
            assert!(!res.is_error);
            assert_eq!(
                res.content[0].text,
                format_memory_output(&expected, compact)
            );
            if !compact {
                assert!(!res.content[0].text.contains("[score: "));
            }
        }
    }

    // ── v2 is the default engine ─────────────────────────────────────────

    /// Deterministic bag-of-words embedder: texts sharing words are close.
    struct WordEmbedder;
    impl Embedder for WordEmbedder {
        fn embed(&self, text: &str) -> icm_core::IcmResult<Vec<f32>> {
            let mut v = vec![0.0_f32; 64];
            for word in text.split(|c: char| !c.is_alphanumeric()) {
                if !word.is_empty() {
                    let bucket = word.to_lowercase().bytes().fold(7usize, |acc, b| {
                        acc.wrapping_mul(31).wrapping_add(usize::from(b))
                    }) % 64;
                    v[bucket] += 1.0;
                }
            }
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                v.iter_mut().for_each(|x| *x /= norm);
            } else {
                v[0] = 1.0;
            }
            Ok(v)
        }
        fn embed_batch(&self, texts: &[&str]) -> icm_core::IcmResult<Vec<Vec<f32>>> {
            texts.iter().map(|t| self.embed(t)).collect()
        }
        fn dimensions(&self) -> usize {
            64
        }
    }

    fn store_plain(store: &Store, topic: &str, summary: &str) {
        use icm_core::Importance;
        store
            .store(Memory::new(
                topic.into(),
                summary.into(),
                Importance::Medium,
            ))
            .unwrap();
    }

    /// With no budget and no environment override the tool now ranks with
    /// v2: a memory sharing only some of the query words is found, and the
    /// one sharing the most comes first. The legacy full-text search needed
    /// every word and returned the single exact match.
    #[test]
    fn test_recall_default_engine_is_v2() {
        if std::env::var_os("ICM_RECALL_ENGINE").is_some() {
            return;
        }
        let store = test_store();
        store_plain(&store, "t", "kiwi only");
        store_plain(&store, "t", "unrelated note about build caches");
        store_plain(&store, "t", "kiwi mango papaya");
        let args = json!({"query": "kiwi mango papaya", "project": ""});

        let res = call_tool(&store, None, "icm_memory_recall", &args, true);
        assert!(!res.is_error, "{}", res.content[0].text);
        assert_eq!(
            res.content[0].text,
            "[t] kiwi mango papaya\n[t] kiwi only\n"
        );

        let legacy = tool_recall_on(&store, None, &args, true, RecallEngine::Legacy, None);
        assert_eq!(legacy.content[0].text, "[t] kiwi mango papaya\n");
    }

    /// The default engine keeps the output contract of the tool: the
    /// published 1..=20 limit, the same compact and detailed layouts, a
    /// score only when an embedder ranked, and the same empty answer.
    #[test]
    fn test_recall_default_engine_keeps_the_output_shape() {
        if std::env::var_os("ICM_RECALL_ENGINE").is_some() {
            return;
        }
        let store = Store::in_memory_with_dims(64).unwrap();
        for i in 0..30 {
            let mut mem = Memory::new(
                "t".into(),
                format!("shape probe entry {i}"),
                icm_core::Importance::Medium,
            );
            mem.embedding = Some(WordEmbedder.embed(&mem.embed_text()).unwrap());
            store.store(mem).unwrap();
        }
        let args = json!({"query": "shape probe entry", "project": "", "limit": 100});

        // Limit: clamped to 20 without a budget, default 5.
        let res = call_tool(&store, None, "icm_memory_recall", &args, true);
        assert_eq!(res.content[0].text.lines().count(), 20);
        assert!(
            res.content[0]
                .text
                .lines()
                .all(|l| l.starts_with("[t] shape probe entry "))
        );
        let res = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "shape probe entry", "project": ""}),
            true,
        );
        assert_eq!(res.content[0].text.lines().count(), 5);

        // Detailed layout, keyword-only: no score, as on the legacy path.
        let res = call_tool(&store, None, "icm_memory_recall", &args, false);
        let text = &res.content[0].text;
        assert_eq!(
            text.matches("\n  topic: t\n  importance: medium\n").count(),
            20
        );
        assert!(!text.contains("[score: "));
        assert!(text.starts_with("--- "));

        // Detailed layout with an embedder: a score on every entry.
        let res = call_tool(
            &store,
            Some(&WordEmbedder),
            "icm_memory_recall",
            &args,
            false,
        );
        assert_eq!(res.content[0].text.matches("[score: ").count(), 20);

        // Nothing matches, or a blank query: the plain message, no error.
        for query in ["zzzqqq", "  "] {
            let res = call_tool(
                &store,
                None,
                "icm_memory_recall",
                &json!({"query": query, "project": ""}),
                false,
            );
            assert!(!res.is_error, "{query:?}");
            assert_eq!(res.content[0].text, MSG_NO_MEMORIES, "{query:?}");
        }
    }

    // ── #476: ids and links for clients that parse the result ───────────

    use icm_core::Importance;

    fn parsed(result: &ToolResult) -> Vec<Value> {
        assert!(!result.is_error, "{}", result.content[0].text);
        serde_json::from_str::<Value>(&result.content[0].text)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", result.content[0].text))
            .as_array()
            .expect("a JSON array")
            .clone()
    }

    fn ids_of(records: &[Value]) -> Vec<String> {
        records
            .iter()
            .map(|r| r["id"].as_str().expect("id").to_string())
            .collect()
    }

    /// A small graph in project `alpha`:
    ///
    /// ```text
    /// a ──► b ──► c
    /// │     └───► a          (cycle back to the start)
    /// ├───► other ──► hidden (other is in project beta; hidden is in alpha
    /// │                       but reachable through other only)
    /// └───► an id that no longer exists
    /// ```
    struct Graph {
        store: Store,
        a: Memory,
        b: Memory,
        c: Memory,
        other: Memory,
        hidden: Memory,
    }

    fn graph() -> Graph {
        let mem =
            |topic: &str, text: &str| Memory::new(topic.into(), text.into(), Importance::High);
        let mut a = mem("decisions-alpha", "Alpha stores sessions in Redis");
        let mut b = mem("decisions-alpha", "Redis runs with append-only persistence");
        let c = mem("context-alpha", "The persistence volume is a 20 GB disk");
        let mut other = mem("decisions-beta", "Beta keeps sessions in Postgres");
        let hidden = mem("context-alpha", "Reachable through a beta memory only");
        a.related_ids = vec![
            b.id.clone(),
            other.id.clone(),
            "01NOLONGERTHERE00000000000".into(),
        ];
        b.related_ids = vec![c.id.clone(), a.id.clone()];
        other.related_ids = vec![hidden.id.clone()];
        let store = test_store();
        for m in [&a, &b, &c, &other, &hidden] {
            store.store(m.clone()).unwrap();
        }
        Graph {
            store,
            a,
            b,
            c,
            other,
            hidden,
        }
    }

    #[test]
    fn recall_json_gives_each_record_its_id_and_links() {
        let g = graph();
        // Compact is the server default: it must not change a JSON answer.
        for compact in [true, false] {
            let result = call_tool(
                &g.store,
                None,
                "icm_memory_recall",
                &json!({"query": "sessions Redis", "project": "alpha", "format": "json"}),
                compact,
            );
            let records = parsed(&result);
            let a = records
                .iter()
                .find(|r| r["id"] == json!(g.a.id))
                .unwrap_or_else(|| panic!("a is missing: {records:?}"));
            assert_eq!(a["topic"], "decisions-alpha");
            assert_eq!(a["summary"], "Alpha stores sessions in Redis");
            assert_eq!(a["importance"], "high");
            assert_eq!(a["related_ids"], json!(g.a.related_ids));
            assert!(a["keywords"].is_array());
            // Three decimals, not the f32's full expansion.
            let weight = a["weight"].to_string();
            assert!(weight.len() <= 5, "{weight}");
            for stamp in ["created_at", "updated_at"] {
                let text = a[stamp].as_str().expect(stamp);
                assert!(chrono::DateTime::parse_from_rfc3339(text).is_ok(), "{text}");
            }
            // No embedder took part: there is no similarity to report.
            assert!(a.get("score").is_none(), "{a}");
            assert!(a.get("hops").is_none(), "{a}");
            // The project filter still applies.
            assert!(!ids_of(&records).contains(&g.other.id));
        }
    }

    #[test]
    fn recall_json_is_the_same_on_the_legacy_engine() {
        let g = graph();
        let args = json!({"query": "sessions Redis", "project": "alpha", "format": "json"});
        let result = tool_recall_on(&g.store, None, &args, true, RecallEngine::Legacy, None);
        let ids = ids_of(&parsed(&result));
        assert!(ids.contains(&g.a.id), "{ids:?}");
        assert!(!ids.contains(&g.other.id), "{ids:?}");
    }

    #[test]
    fn recall_json_with_no_match_is_an_empty_array() {
        let g = graph();
        for query in ["zzzunknownword", "   "] {
            for engine in [RecallEngine::V2, RecallEngine::Legacy] {
                let args = json!({"query": query, "project": "alpha", "format": "json"});
                let result = tool_recall_on(&g.store, None, &args, true, engine, None);
                assert!(!result.is_error);
                assert_eq!(result.content[0].text, "[]", "{query:?} {engine:?}");
            }
        }
    }

    #[test]
    fn recall_refuses_a_format_it_does_not_know() {
        let g = graph();
        for format in [json!("xml"), json!("JSON"), json!(1), json!(true)] {
            let result = call_tool(
                &g.store,
                None,
                "icm_memory_recall",
                &json!({"query": "sessions", "project": "alpha", "format": format}),
                true,
            );
            assert!(result.is_error, "{format}");
            let text = &result.content[0].text;
            assert!(
                text.contains("\"text\"") && text.contains("\"json\""),
                "{text}"
            );
        }
        // Naming the default explicitly is the default.
        let named = call_tool(
            &g.store,
            None,
            "icm_memory_recall",
            &json!({"query": "sessions Redis", "project": "alpha", "format": "text"}),
            true,
        );
        let default = call_tool(
            &g.store,
            None,
            "icm_memory_recall",
            &json!({"query": "sessions Redis", "project": "alpha"}),
            true,
        );
        assert_eq!(named.content[0].text, default.content[0].text);
    }

    #[test]
    fn recall_text_keeps_its_compact_shape_and_lists_links_in_the_full_one() {
        let g = graph();
        let args = json!({"query": "sessions Redis", "project": "alpha"});
        // Compact: `[topic] summary` lines and nothing else, as before.
        let compact = call_tool(&g.store, None, "icm_memory_recall", &args, true);
        let text = &compact.content[0].text;
        assert!(text.lines().all(|l| l.starts_with('[')), "{text}");
        assert!(
            !text.contains(&g.a.id) && !text.contains("related:"),
            "{text}"
        );
        // Full: the links of a memory, on their own line.
        let full = call_tool(&g.store, None, "icm_memory_recall", &args, false);
        let text = &full.content[0].text;
        assert!(
            text.contains(&format!("  related: {}", g.a.related_ids.join(", "))),
            "{text}"
        );
        // A memory without links has no such line.
        let c = call_tool(
            &g.store,
            None,
            "icm_memory_recall",
            &json!({"query": "persistence volume disk", "project": "alpha", "limit": 1}),
            false,
        );
        let text = &c.content[0].text;
        assert!(
            text.contains(&g.c.id) && !text.contains("related:"),
            "{text}"
        );
    }

    #[test]
    fn recall_json_caps_the_raw_excerpt_like_the_text_view() {
        let store = test_store();
        let mut long = Memory::new(
            "context-alpha".into(),
            "Stack trace of the import failure".into(),
            Importance::Medium,
        );
        // Multi-byte text: the cut must land on a character boundary.
        long.raw_excerpt = Some("é".repeat(3000));
        let mut short = Memory::new(
            "context-alpha".into(),
            "Short note about the import failure".into(),
            Importance::Medium,
        );
        short.raw_excerpt = Some("exit code 2".into());
        store.store(long.clone()).unwrap();
        store.store(short.clone()).unwrap();

        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "import failure", "project": "alpha", "format": "json"}),
            true,
        );
        let records = parsed(&result);
        let of = |id: &str| {
            records
                .iter()
                .find(|r| r["id"] == json!(id))
                .unwrap()
                .clone()
        };
        let long = of(&long.id);
        let shown = long["raw_excerpt"].as_str().unwrap();
        assert!(shown.len() <= MAX_RAW_IN_RECALL && shown.len() >= MAX_RAW_IN_RECALL - 1);
        assert!(shown.chars().all(|c| c == 'é'));
        assert_eq!(long["raw_excerpt_bytes"], 6000);
        let short = of(&short.id);
        assert_eq!(short["raw_excerpt"], "exit code 2");
        assert!(short.get("raw_excerpt_bytes").is_none());
    }

    #[test]
    fn recall_json_respects_the_token_budget() {
        let store = test_store();
        for i in 0..40 {
            store
                .store(Memory::new(
                    "context-alpha".into(),
                    format!("Deployment note {i}: the rollout of service {i} went through staging first and took a while"),
                    Importance::Medium,
                ))
                .unwrap();
        }
        let budget = 600usize;
        let result = call_tool(
            &store,
            None,
            "icm_memory_recall",
            &json!({"query": "deployment rollout staging", "project": "alpha", "format": "json", "max_tokens": budget}),
            true,
        );
        let records = parsed(&result);
        assert!(
            !records.is_empty() && records.len() < 40,
            "{}",
            records.len()
        );
        // What is charged is what is printed: the records fit the budget.
        let printed: usize = records
            .iter()
            .map(|r| icm_core::estimate_tokens(&r.to_string()))
            .sum();
        assert!(
            printed <= budget,
            "{printed} tokens for a budget of {budget}"
        );
    }

    fn related(g: &Graph, args: Value, compact: bool) -> ToolResult {
        call_tool(&g.store, None, "icm_memory_related", &args, compact)
    }

    #[test]
    fn related_walks_the_links_nearest_first() {
        let g = graph();
        let one = parsed(&related(
            &g,
            json!({"id": g.a.id, "project": "alpha", "format": "json"}),
            true,
        ));
        assert_eq!(ids_of(&one), vec![g.b.id.clone()]);
        assert_eq!(one[0]["hops"], 1);
        assert_eq!(one[0]["related_ids"], json!(g.b.related_ids));

        let two = parsed(&related(
            &g,
            json!({"id": g.a.id, "depth": 2, "project": "alpha", "format": "json"}),
            true,
        ));
        // b links back to a: the start is never part of the answer.
        assert_eq!(ids_of(&two), vec![g.b.id.clone(), g.c.id.clone()]);
        assert_eq!(two[1]["hops"], 2);

        // Deeper than the graph, and deeper than the published maximum.
        for depth in [3, 99] {
            let all = parsed(&related(
                &g,
                json!({"id": g.a.id, "depth": depth, "project": "alpha", "format": "json"}),
                true,
            ));
            assert_eq!(ids_of(&all), vec![g.b.id.clone(), g.c.id.clone()]);
        }
    }

    #[test]
    fn related_stays_inside_the_project() {
        let g = graph();
        // In alpha: `other` (beta) is left out, and `hidden`, which only it
        // links to, is not reached through it.
        let scoped = ids_of(&parsed(&related(
            &g,
            json!({"id": g.a.id, "depth": 3, "project": "alpha", "format": "json"}),
            true,
        )));
        assert!(
            !scoped.contains(&g.other.id) && !scoped.contains(&g.hidden.id),
            "{scoped:?}"
        );

        // Across projects, on request.
        let all = ids_of(&parsed(&related(
            &g,
            json!({"id": g.a.id, "depth": 3, "project": "", "format": "json"}),
            true,
        )));
        assert_eq!(
            all,
            vec![
                g.b.id.clone(),
                g.other.id.clone(),
                g.c.id.clone(),
                g.hidden.id.clone()
            ]
        );

        // A start memory of another project is an error, not "no links".
        let outside = related(&g, json!({"id": g.other.id, "project": "alpha"}), true);
        assert!(outside.is_error);
        let text = &outside.content[0].text;
        assert!(
            text.contains("alpha") && text.contains("decisions-beta"),
            "{text}"
        );
    }

    #[test]
    fn related_honors_the_limit_and_counts_the_read() {
        let g = graph();
        let one = ids_of(&parsed(&related(
            &g,
            json!({"id": g.a.id, "depth": 3, "limit": 1, "project": "", "format": "json"}),
            true,
        )));
        assert_eq!(one, vec![g.b.id.clone()]);
        // Following a link is a read of the memory it leads to, not of the
        // ones the limit left out.
        let count = |id: &str| g.store.get(id).unwrap().unwrap().access_count;
        assert_eq!(count(&g.b.id), 1);
        assert_eq!(count(&g.c.id), 0);
    }

    #[test]
    fn related_answers_in_text_too() {
        let g = graph();
        let compact = related(
            &g,
            json!({"id": g.a.id, "depth": 2, "project": "alpha"}),
            true,
        );
        assert_eq!(
            compact.content[0].text,
            "[decisions-alpha] Redis runs with append-only persistence\n\
             [context-alpha] The persistence volume is a 20 GB disk\n"
        );
        let full = related(&g, json!({"id": g.a.id, "project": "alpha"}), false);
        let text = &full.content[0].text;
        assert!(text.starts_with(&format!("--- {} ---\n", g.b.id)), "{text}");
        assert!(!text.contains("score"), "{text}");

        // A memory with no link, in both shapes.
        let none = related(&g, json!({"id": g.c.id, "project": "alpha"}), true);
        assert!(!none.is_error);
        assert_eq!(none.content[0].text, "No related memories.");
        let none = related(
            &g,
            json!({"id": g.c.id, "project": "alpha", "format": "json"}),
            true,
        );
        assert_eq!(none.content[0].text, "[]");
    }

    #[test]
    fn related_reports_what_it_cannot_start_from() {
        let g = graph();
        let missing = related(&g, json!({"project": "alpha"}), true);
        assert!(missing.is_error && missing.content[0].text.contains("id"));
        let blank = related(&g, json!({"id": "  "}), true);
        assert!(blank.is_error && blank.content[0].text.contains("id"));
        let unknown = related(
            &g,
            json!({"id": "01NOLONGERTHERE00000000000", "project": ""}),
            true,
        );
        assert!(unknown.is_error);
        assert!(unknown.content[0].text.contains("memory not found"));
        let format = related(&g, json!({"id": g.a.id, "format": "yaml"}), true);
        assert!(format.is_error && format.content[0].text.contains("\"json\""));
    }

    #[test]
    fn related_reads_depth_and_limit_however_the_client_writes_them() {
        let g = graph();
        let ask = |depth: Value, limit: Value| {
            related(
                &g,
                json!({"id": g.a.id, "depth": depth, "limit": limit, "project": "", "format": "json"}),
                true,
            )
        };
        // A whole float and a numeric string are the number they spell:
        // two links away from `a`, across projects, is everything; one
        // link away is `b` and `other` only.
        for depth in [json!(2), json!(2.0), json!("2"), json!(" 2 ")] {
            assert_eq!(
                parsed(&ask(depth.clone(), json!(50))).len(),
                4,
                "depth {depth}"
            );
        }
        assert_eq!(parsed(&ask(json!(1.0), json!(50))).len(), 2);
        for limit in [json!(2), json!(2.0), json!("2")] {
            assert_eq!(
                parsed(&ask(json!(3), limit.clone())).len(),
                2,
                "limit {limit}"
            );
        }
        // Out of range is brought back into range, in both directions.
        assert_eq!(parsed(&ask(json!(-4), json!(0))).len(), 1);
        assert_eq!(parsed(&ask(json!(u64::MAX), json!(u64::MAX))).len(), 4);
        // Anything else is refused: answering at the default depth would
        // read as "there is nothing further".
        for (depth, limit, key) in [
            (json!(2.5), json!(10), "depth"),
            (json!("three"), json!(10), "depth"),
            (json!(true), json!(10), "depth"),
            (json!(1), json!(1.5), "limit"),
            (json!(1), json!([2]), "limit"),
        ] {
            let result = ask(depth.clone(), limit.clone());
            assert!(result.is_error, "depth {depth} limit {limit}");
            assert!(result.content[0].text.contains(&format!("invalid {key}")));
        }
    }

    #[test]
    fn the_tool_list_publishes_the_format_option_and_the_related_tool() {
        for has_embedder in [false, true] {
            let defs = tool_definitions(has_embedder);
            let tools = defs["tools"].as_array().unwrap();
            let named = |name: &str| {
                tools
                    .iter()
                    .find(|t| t["name"] == name)
                    .unwrap_or_else(|| panic!("{name} is not listed"))
            };
            let recall = named("icm_memory_recall");
            assert_eq!(
                recall["inputSchema"]["properties"]["format"]["enum"],
                json!(["text", "json"])
            );
            assert_eq!(recall["inputSchema"]["required"], json!(["query"]));
            let related = named("icm_memory_related");
            assert_eq!(related["inputSchema"]["required"], json!(["id"]));
            assert_eq!(
                related["inputSchema"]["properties"]["depth"]["maximum"],
                RELATED_MAX_DEPTH
            );
            assert_eq!(
                related["inputSchema"]["properties"]["limit"]["maximum"],
                RELATED_MAX_LIMIT
            );
        }
    }
}
