//! Import conversations from external sources into ICM.
//!
//! Supported formats:
//! - Claude.ai JSON export
//! - ChatGPT conversations.json
//! - Claude Code JSONL sessions
//! - Slack JSON export
//! - Plain text files
//!
//! Zero dependencies beyond serde_json (already in icm-cli).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};

use icm_core::{Memory, MemorySource, MemoryStore};
use icm_store::SqliteStore;

use crate::extract;

// ── Data structures ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Exchange {
    pub role: Role,
    pub content: String,
    #[allow(dead_code)]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportFormat {
    ClaudeAi,
    ChatGpt,
    ClaudeCode,
    Slack,
    Text,
}

// ── Format detection ─────────────────────────────────────────────────────

/// Detect format from file extension and content peek.
pub fn detect_format(path: &Path) -> Result<ImportFormat> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jsonl" => return Ok(ImportFormat::ClaudeCode),
        "txt" | "md" => return Ok(ImportFormat::Text),
        "json" => {}
        _ => return Ok(ImportFormat::Text),
    }

    // Peek JSON content to discriminate formats
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let peek = if content.len() > 4096 {
        &content[..4096]
    } else {
        &content
    };

    if peek.contains("chat_messages") || (peek.contains("\"uuid\"") && peek.contains("\"sender\""))
    {
        Ok(ImportFormat::ClaudeAi)
    } else if peek.contains("\"mapping\"") && peek.contains("\"conversation_id\"") {
        Ok(ImportFormat::ChatGpt)
    } else if peek.contains("\"messages\"")
        && (peek.contains("\"subtype\"") || peek.contains("\"client_msg_id\""))
    {
        Ok(ImportFormat::Slack)
    } else {
        Ok(ImportFormat::Text)
    }
}

// ── Parsers ──────────────────────────────────────────────────────────────

/// Parse Claude.ai JSON export.
/// Format: {"uuid":"...", "chat_messages":[{"sender":"human"|"assistant","text":"..."}]}
/// Or array of such objects.
pub fn parse_claude_ai(content: &str) -> Result<(Vec<Exchange>, String)> {
    let val: serde_json::Value = serde_json::from_str(content).context("invalid Claude.ai JSON")?;

    // Could be a single conversation or an array
    let conversations = if val.is_array() {
        val.as_array().unwrap().clone()
    } else {
        vec![val]
    };

    let mut exchanges = Vec::new();
    let mut thread_id = String::from("claude-ai");

    for convo in &conversations {
        if let Some(uuid) = convo.get("uuid").and_then(|v| v.as_str()) {
            thread_id = uuid.to_string();
        }

        let messages = convo
            .get("chat_messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for msg in &messages {
            let sender = msg.get("sender").and_then(|v| v.as_str()).unwrap_or("");
            let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                continue;
            }
            let role = match sender {
                "human" => Role::User,
                "assistant" => Role::Assistant,
                _ => Role::System,
            };
            exchanges.push(Exchange {
                role,
                content: text.to_string(),
                timestamp: None,
            });
        }
    }

    Ok((exchanges, thread_id))
}

/// Parse ChatGPT conversations.json export.
/// Format: {"conversation_id":"...", "mapping":{"node_id":{"message":{"author":{"role":"..."},"content":{"parts":["..."]}}}}}
pub fn parse_chatgpt(content: &str) -> Result<(Vec<Exchange>, String)> {
    let val: serde_json::Value = serde_json::from_str(content).context("invalid ChatGPT JSON")?;

    // Could be array of conversations or single
    let conversations = if val.is_array() {
        val.as_array().unwrap().clone()
    } else {
        vec![val]
    };

    let mut exchanges = Vec::new();
    let mut thread_id = String::from("chatgpt");

    for convo in &conversations {
        if let Some(cid) = convo.get("conversation_id").and_then(|v| v.as_str()) {
            thread_id = cid.to_string();
        }

        let mapping = match convo.get("mapping").and_then(|v| v.as_object()) {
            Some(m) => m,
            None => continue,
        };

        // Collect nodes with create_time for sorting
        let mut nodes: Vec<(f64, &serde_json::Value)> = mapping
            .values()
            .filter_map(|node| {
                let msg = node.get("message")?;
                let ct = msg
                    .get("create_time")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                Some((ct, msg))
            })
            .collect();
        // Guard against NaN create_time (would otherwise panic the import).
        nodes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        for (_, msg) in &nodes {
            let role_str = msg
                .pointer("/author/role")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let parts = msg
                .pointer("/content/parts")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let text: String = parts
                .iter()
                .filter_map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                continue;
            }
            let role = match role_str {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => Role::System,
            };
            exchanges.push(Exchange {
                role,
                content: text,
                timestamp: None,
            });
        }
    }

    Ok((exchanges, thread_id))
}

/// Parse Claude Code JSONL session.
/// Each line: {"type":"user"|"assistant"|"result","message":{"role":"...","content":[{"type":"text","text":"..."}]}}
pub fn parse_claude_code(content: &str) -> Result<(Vec<Exchange>, String)> {
    let mut exchanges = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let role = match msg_type {
            "user" | "human" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };

        let msg_content = entry.pointer("/message/content");
        let text = match msg_content {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                        item.get("text").and_then(|v| v.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };

        if !text.is_empty() {
            exchanges.push(Exchange {
                role,
                content: text,
                timestamp: None,
            });
        }
    }

    // Thread ID from first line's session_id or fallback
    let thread_id = content
        .lines()
        .next()
        .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .and_then(|v| {
            v.get("session_id")
                .and_then(|s| s.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "claude-code".to_string());

    Ok((exchanges, thread_id))
}

/// Parse Slack JSON export.
/// Format: [{"user":"U123","text":"...","ts":"1234567.890"}] or {"messages":[...]}
pub fn parse_slack(content: &str) -> Result<(Vec<Exchange>, String)> {
    let val: serde_json::Value = serde_json::from_str(content).context("invalid Slack JSON")?;

    let messages = if val.is_array() {
        val.as_array().unwrap().clone()
    } else if let Some(msgs) = val.get("messages").and_then(|v| v.as_array()) {
        msgs.clone()
    } else {
        bail!("no messages found in Slack export");
    };

    let mut exchanges = Vec::new();
    let thread_id = "slack".to_string();

    for msg in &messages {
        let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let subtype = msg.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        // Skip join/leave/bot messages
        if !subtype.is_empty() {
            continue;
        }
        exchanges.push(Exchange {
            role: Role::User, // Slack messages are all "user" from ICM perspective
            content: text.to_string(),
            timestamp: None,
        });
    }

    Ok((exchanges, thread_id))
}

/// Parse plain text file as a single user exchange.
pub fn parse_text(content: &str, path: &Path) -> Result<(Vec<Exchange>, String)> {
    let thread_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("text")
        .to_string();

    // Check if text has > markers (transcript format)
    let lines: Vec<&str> = content.lines().collect();
    let quote_count = lines
        .iter()
        .filter(|l| l.trim_start().starts_with('>'))
        .count();

    if quote_count >= 3 {
        // Parse as transcript: > lines are user, rest is assistant
        let mut exchanges = Vec::new();
        let mut current_role = Role::Assistant;
        let mut current_text = String::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with('>') {
                if !current_text.is_empty() {
                    exchanges.push(Exchange {
                        role: current_role.clone(),
                        content: current_text.trim().to_string(),
                        timestamp: None,
                    });
                    current_text.clear();
                }
                current_role = Role::User;
                current_text.push_str(trimmed.trim_start_matches('>').trim());
                current_text.push('\n');
            } else {
                if current_role == Role::User && !current_text.is_empty() {
                    exchanges.push(Exchange {
                        role: Role::User,
                        content: current_text.trim().to_string(),
                        timestamp: None,
                    });
                    current_text.clear();
                    current_role = Role::Assistant;
                }
                current_text.push_str(trimmed);
                current_text.push('\n');
            }
        }
        if !current_text.is_empty() {
            exchanges.push(Exchange {
                role: current_role,
                content: current_text.trim().to_string(),
                timestamp: None,
            });
        }
        return Ok((exchanges, thread_id));
    }

    // Default: entire file as single user message
    Ok((
        vec![Exchange {
            role: Role::User,
            content: content.to_string(),
            timestamp: None,
        }],
        thread_id,
    ))
}

// ── File collection ──────────────────────────────────────────────────────

const IMPORT_EXTENSIONS: &[&str] = &["json", "jsonl", "txt", "md"];

fn collect_importable_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_recursive(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "node_modules" || name == "__pycache__" {
                continue;
            }
            collect_recursive(&path, files)?;
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if IMPORT_EXTENSIONS.contains(&ext) {
                files.push(path);
            }
        }
    }
    Ok(())
}

// ── Exchange → text ──────────────────────────────────────────────────────

fn exchanges_to_text(exchanges: &[Exchange]) -> String {
    let mut text = String::new();
    for ex in exchanges {
        if ex.role == Role::System {
            continue;
        }
        let label = match ex.role {
            Role::User => "[user]",
            Role::Assistant => "[assistant]",
            Role::System => continue,
        };
        text.push_str(label);
        text.push_str(": ");
        text.push_str(&ex.content);
        text.push('\n');
    }
    text
}

// ── Main import command ──────────────────────────────────────────────────

pub fn cmd_import(
    store: &SqliteStore,
    path: PathBuf,
    format: Option<ImportFormat>,
    project: String,
    dry_run: bool,
) -> Result<()> {
    let files = if path.is_dir() {
        collect_importable_files(&path)?
    } else {
        vec![path]
    };

    if files.is_empty() {
        println!("No importable files found.");
        return Ok(());
    }

    let mut total_facts = 0;
    let mut total_files = 0;

    for file in &files {
        let fmt = match format {
            Some(f) => f,
            None => detect_format(file)?,
        };

        let content =
            std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;

        if content.trim().is_empty() {
            continue;
        }

        let (exchanges, thread_id) = match fmt {
            ImportFormat::ClaudeAi => parse_claude_ai(&content)?,
            ImportFormat::ChatGpt => parse_chatgpt(&content)?,
            ImportFormat::ClaudeCode => parse_claude_code(&content)?,
            ImportFormat::Slack => parse_slack(&content)?,
            ImportFormat::Text => parse_text(&content, file)?,
        };

        if exchanges.is_empty() {
            continue;
        }

        let text = exchanges_to_text(&exchanges);
        let facts = extract::extract_and_classify(&text, &project);

        if facts.is_empty() {
            continue;
        }

        let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let fact_count = facts.len();

        for (topic, content, importance, extra_kw) in facts {
            if dry_run {
                let kind_tags: Vec<&str> = extra_kw
                    .iter()
                    .filter(|k: &&String| k.starts_with("kind:"))
                    .map(|s: &String| s.as_str())
                    .collect();
                let entity_tags: Vec<&str> = extra_kw
                    .iter()
                    .filter(|k: &&String| k.starts_with("entity:"))
                    .map(|s: &String| s.as_str())
                    .collect();
                println!(
                    "  [{importance}] ({topic}) {content}{}{}",
                    if kind_tags.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", kind_tags.join(" "))
                    },
                    if entity_tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", entity_tags.join(", "))
                    },
                );
            } else {
                let mut mem = Memory::new(topic, content, importance);
                mem.source = MemorySource::Conversation {
                    thread_id: thread_id.clone(),
                };
                mem.keywords = extra_kw;
                let raw = if text.len() > 500 {
                    &text[..500]
                } else {
                    &text
                };
                mem.raw_excerpt = Some(raw.to_string());
                store.store(mem)?;
            }
        }

        total_facts += fact_count;
        total_files += 1;

        if dry_run {
            println!("  -- {file_name}: {fact_count} facts (format: {fmt:?})\n");
        }
    }

    if dry_run {
        println!("Would import {total_facts} facts from {total_files} files (dry run).");
    } else {
        println!("Imported {total_facts} facts from {total_files} files.");
    }

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_jsonl() {
        let path = Path::new("session.jsonl");
        assert_eq!(detect_format(path).unwrap(), ImportFormat::ClaudeCode);
    }

    #[test]
    fn test_detect_format_txt() {
        let path = Path::new("notes.txt");
        assert_eq!(detect_format(path).unwrap(), ImportFormat::Text);
    }

    #[test]
    fn test_detect_format_md() {
        let path = Path::new("README.md");
        assert_eq!(detect_format(path).unwrap(), ImportFormat::Text);
    }

    #[test]
    fn test_parse_claude_ai_json() {
        let sample = r#"{"uuid":"abc-123","chat_messages":[
            {"sender":"human","text":"Hello, how are you?"},
            {"sender":"assistant","text":"I am doing well, thank you!"}
        ]}"#;
        let (exchanges, thread_id) = parse_claude_ai(sample).unwrap();
        assert_eq!(exchanges.len(), 2);
        assert_eq!(thread_id, "abc-123");
        assert_eq!(exchanges[0].role, Role::User);
        assert_eq!(exchanges[1].role, Role::Assistant);
        assert!(exchanges[0].content.contains("Hello"));
    }

    #[test]
    fn test_parse_chatgpt_json() {
        let sample = r#"{
            "conversation_id": "conv-456",
            "mapping": {
                "n1": {"message": {"author": {"role": "user"}, "content": {"parts": ["We decided to use Rust"]}, "create_time": 1.0}},
                "n2": {"message": {"author": {"role": "assistant"}, "content": {"parts": ["Good choice!"]}, "create_time": 2.0}}
            }
        }"#;
        let (exchanges, thread_id) = parse_chatgpt(sample).unwrap();
        assert_eq!(exchanges.len(), 2);
        assert_eq!(thread_id, "conv-456");
        assert_eq!(exchanges[0].role, Role::User);
        assert!(exchanges[0].content.contains("Rust"));
    }

    #[test]
    fn test_parse_claude_code_jsonl() {
        let sample = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Fix the bug in auth module"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I fixed the authentication error in middleware.rs"}]}}"#
        );
        let (exchanges, _) = parse_claude_code(sample).unwrap();
        assert_eq!(exchanges.len(), 2);
        assert_eq!(exchanges[0].role, Role::User);
        assert!(exchanges[0].content.contains("bug"));
        assert_eq!(exchanges[1].role, Role::Assistant);
    }

    #[test]
    fn test_parse_slack_json() {
        let sample = r#"[
            {"user":"U001","text":"We should switch to GraphQL","ts":"1234.0"},
            {"user":"U002","text":"I agree, REST is getting complex","ts":"1235.0"},
            {"subtype":"channel_join","text":"joined the channel","ts":"1236.0"}
        ]"#;
        let (exchanges, _) = parse_slack(sample).unwrap();
        assert_eq!(exchanges.len(), 2); // channel_join skipped
        assert!(exchanges[0].content.contains("GraphQL"));
    }

    #[test]
    fn test_parse_text_plain() {
        let (exchanges, thread_id) =
            parse_text("Just some project notes", Path::new("notes.txt")).unwrap();
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].role, Role::User);
        assert_eq!(thread_id, "notes");
    }

    #[test]
    fn test_parse_text_transcript() {
        let content = "> What is the status of the deploy?\nDeploy is running.\n> Any errors?\nNo errors found.\n> Great, ship it.\nDone.";
        let (exchanges, _) = parse_text(content, Path::new("chat.txt")).unwrap();
        assert!(exchanges.len() >= 3); // At least 3 user turns
        assert!(exchanges.iter().any(|e| e.role == Role::User));
    }

    #[test]
    fn test_exchanges_to_text() {
        let exchanges = vec![
            Exchange {
                role: Role::User,
                content: "Hello".into(),
                timestamp: None,
            },
            Exchange {
                role: Role::Assistant,
                content: "Hi there".into(),
                timestamp: None,
            },
        ];
        let text = exchanges_to_text(&exchanges);
        assert!(text.contains("[user]: Hello"));
        assert!(text.contains("[assistant]: Hi there"));
    }

    #[test]
    fn test_import_roundtrip() {
        let store = SqliteStore::in_memory().unwrap();
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"We decided to use SQLite instead of Postgres because we need zero external dependencies. Sarah agreed with the decision."}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Good choice. SQLite is embedded and requires no server process."}]}}"#,
        );
        let (exchanges, thread_id) = parse_claude_code(jsonl).unwrap();
        let text = exchanges_to_text(&exchanges);
        let facts = extract::extract_and_classify(&text, "test");

        // Should extract at least one fact (decision + entities)
        assert!(!facts.is_empty(), "should extract facts from conversation");

        // Store them
        for (topic, content, importance, extra_kw) in &facts {
            let mut mem = Memory::new(topic.clone(), content.clone(), *importance);
            mem.source = MemorySource::Conversation {
                thread_id: thread_id.clone(),
            };
            mem.keywords = extra_kw.clone();
            store.store(mem).unwrap();
        }

        // Recall
        let results = store.search_fts("SQLite decision", 5).unwrap();
        assert!(!results.is_empty(), "should recall imported facts");
    }
}
