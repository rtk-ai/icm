//! Token-cost benchmark for `icm recall`-style payloads.
//!
//! Builds a fixture of synthetic memories, serializes them in several
//! candidate formats (JSON pretty/compact, JSON-Lines, TOML, TOON, custom
//! compact pipe), and reports byte size + estimated tokens + (optionally)
//! real tokens via Anthropic `count_tokens`. Goal: pick the format that
//! costs the fewest tokens when injected into an LLM context, as a
//! grounded answer to the "JSON vs TOML vs TOON" question.

use anyhow::{Context, Result};
use chrono::{Duration, TimeZone, Utc};
use icm_core::{Importance, Memory, MemorySource, Scope};
use serde::Serialize;

/// Public entry point used by `main.rs` to dispatch the CLI command.
pub fn cmd_bench_format(count: usize, model: &str, no_api: bool) -> Result<()> {
    let memories = synthetic_memories(count);

    let formats: Vec<(&str, String)> = vec![
        ("json-pretty", to_json_pretty(&memories)?),
        ("json-compact", to_json_compact(&memories)?),
        ("jsonl", to_jsonl(&memories)?),
        ("toml", to_toml(&memories)?),
        ("toon", to_toon(&memories)),
        ("compact", to_compact(&memories)),
    ];

    let baseline_bytes = formats
        .iter()
        .find(|(n, _)| *n == "json-compact")
        .map(|(_, s)| s.len())
        .unwrap_or(1);

    println!(
        "Bench-format: {} synthetic memories, model={}, api={}",
        count,
        model,
        if no_api { "off" } else { "on" }
    );
    println!();
    println!(
        "{:<14} {:>8} {:>11} {:>11} {:>10}",
        "format", "bytes", "est_tokens", "api_tokens", "vs_jsonc"
    );
    println!("{}", "-".repeat(58));

    let api_key = if no_api {
        None
    } else {
        std::env::var("ANTHROPIC_API_KEY").ok()
    };

    for (name, payload) in &formats {
        let bytes = payload.len();
        let est = estimate_tokens(payload);
        let api = match api_key.as_deref() {
            Some(k) => match count_tokens_anthropic(k, model, payload) {
                Ok(n) => Some(n),
                Err(e) => {
                    eprintln!("warning: count_tokens failed for {name}: {e}");
                    None
                }
            },
            None => None,
        };
        let api_str = api.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        let ratio = bytes as f64 / baseline_bytes as f64;
        println!(
            "{:<14} {:>8} {:>11} {:>11} {:>10.2}",
            name, bytes, est, api_str, ratio
        );
    }

    if api_key.is_none() && !no_api {
        println!();
        println!("note: ANTHROPIC_API_KEY not set; api_tokens omitted.");
        println!("      est_tokens uses chars/4 — rough, ignores Opus 4.7 +35% inflation.");
    }

    Ok(())
}

// --- format serializers ---------------------------------------------------

fn to_json_pretty(memories: &[Memory]) -> Result<String> {
    serde_json::to_string_pretty(memories).context("serde_json pretty")
}

fn to_json_compact(memories: &[Memory]) -> Result<String> {
    serde_json::to_string(memories).context("serde_json compact")
}

fn to_jsonl(memories: &[Memory]) -> Result<String> {
    let mut out = String::new();
    for m in memories {
        out.push_str(&serde_json::to_string(m).context("serde_json jsonl")?);
        out.push('\n');
    }
    Ok(out)
}

fn to_toml(memories: &[Memory]) -> Result<String> {
    #[derive(Serialize)]
    struct Wrap<'a> {
        memories: &'a [Memory],
    }
    toml::to_string(&Wrap { memories }).context("toml serialize")
}

/// TOON: header declared once, then CSV-style rows.
///
/// We pick a small, recall-relevant column subset so the format isn't
/// penalized by long optional fields (raw_excerpt, embedding) it isn't
/// designed to carry. Drill-down into those is meant to use a separate
/// `icm get <id>` call in the CLI-first usage pattern.
fn to_toon(memories: &[Memory]) -> String {
    let cols = ["id", "topic", "importance", "weight", "summary", "keywords"];
    let mut out = String::new();
    out.push_str(&format!(
        "memories[{}]{{{}}}:\n",
        memories.len(),
        cols.join(",")
    ));
    for m in memories {
        let row = [
            m.id.clone(),
            m.topic.clone(),
            format!("{}", m.importance),
            format!("{:.3}", m.weight),
            m.summary.clone(),
            m.keywords.join(";"),
        ];
        let escaped: Vec<String> = row.iter().map(|f| toon_escape(f)).collect();
        out.push_str("  ");
        out.push_str(&escaped.join(","));
        out.push('\n');
    }
    out
}

/// CSV-style escaping for a TOON cell.
fn toon_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        let inner = field.replace('"', "\"\"");
        format!("\"{inner}\"")
    } else {
        field.to_string()
    }
}

/// Pipe-separated minimal one-line-per-memory format. Smallest payload
/// but loses structure — included as a "lower bound" reference.
fn to_compact(memories: &[Memory]) -> String {
    let mut out = String::new();
    for m in memories {
        out.push_str(&format!(
            "{}|{}|{:.3}|{}\n",
            m.id, m.topic, m.weight, m.summary
        ));
    }
    out
}

// --- token estimation -----------------------------------------------------

fn estimate_tokens(s: &str) -> usize {
    // chars/4 is the classic rough heuristic. Underestimates for
    // syntax-heavy formats (JSON: every '{', '"' is its own token in
    // most BPE tokenizers) and for the Opus 4.7 tokenizer (+35%).
    // Still useful as a free baseline when no API key is available.
    s.chars().count().div_ceil(4)
}

fn count_tokens_anthropic(api_key: &str, model: &str, content: &str) -> Result<usize> {
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
    });

    let resp = ureq::post("https://api.anthropic.com/v1/messages/count_tokens")
        .set("x-api-key", api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send_string(&body.to_string())
        .context("count_tokens request failed")?;

    let status = resp.status();
    let text = resp.into_string().context("read count_tokens response")?;
    if status != 200 {
        anyhow::bail!("count_tokens HTTP {}: {}", status, text);
    }

    #[derive(serde::Deserialize)]
    struct R {
        input_tokens: usize,
    }
    let parsed: R = serde_json::from_str(&text).context("parse count_tokens response")?;
    Ok(parsed.input_tokens)
}

// --- synthetic fixture ----------------------------------------------------

fn synthetic_memories(n: usize) -> Vec<Memory> {
    let topics = [
        "preferences",
        "decisions-icm",
        "erreurs-resolues",
        "contexte-icm",
        "decisions-architecture",
    ];
    let importances = [
        Importance::Critical,
        Importance::High,
        Importance::Medium,
        Importance::Low,
    ];
    let summaries = [
        "User prefers Rust over Go for systems work.",
        "Use Turso for cloud sync; rusqlite for local-first storage.",
        "FTS5 + sqlite-vec hybrid search beats pure vector at recall@5.",
        "Auto-decay runs once per 24h on first recall of the day to amortize cost.",
        "MCP overhead is ~3-4k tokens per session; CLI-first avoids it entirely.",
        "Importance levels gate decay multipliers: critical=0, high=0.5, medium=1, low=2.",
        "icm_wake_up should be invoked early in conversation to leverage 5-min prompt cache.",
        "Embeddings via fastembed (BGE-small) — 384 dims, no API key required.",
        "Fixed: prune --dry-run was diverging from real prune by 1 memory in audit batch 12.",
        "Project filter is segment-aware: 'preferences' always passes through regardless of project.",
    ];
    let keywords = [
        vec!["rust", "go", "language"],
        vec!["turso", "sqlite", "storage"],
        vec!["fts5", "vector", "hybrid", "search"],
        vec!["decay", "lifecycle"],
        vec!["mcp", "cli", "tokens"],
        vec!["importance", "decay"],
        vec!["cache", "wake_up", "tokens"],
        vec!["fastembed", "bge", "embeddings"],
        vec!["audit", "prune", "dry-run"],
        vec!["project", "topic", "filter"],
    ];

    let base = Utc.with_ymd_and_hms(2026, 4, 1, 12, 0, 0).unwrap();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut m = Memory::new(
            topics[i % topics.len()].to_string(),
            summaries[i % summaries.len()].to_string(),
            importances[i % importances.len()],
        );
        // Fixed ULID-like id so output is reproducible across runs.
        m.id = format!("01HZZBENCH{:016}", i);
        let ts = base + Duration::hours(i as i64);
        m.created_at = ts;
        m.updated_at = ts;
        m.last_accessed = ts;
        m.access_count = (i % 7) as u32;
        m.weight = 1.0 - (i as f32) * 0.05;
        m.keywords = keywords[i % keywords.len()]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        m.source = MemorySource::Manual;
        m.related_ids = Vec::new();
        m.embedding = None;
        m.scope = Scope::User;
        out.push(m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_fixture_is_reproducible() {
        let a = synthetic_memories(5);
        let b = synthetic_memories(5);
        assert_eq!(a.len(), 5);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.id, y.id);
            assert_eq!(x.summary, y.summary);
            assert_eq!(x.weight, y.weight);
        }
    }

    #[test]
    fn toon_header_then_rows() {
        let mems = synthetic_memories(3);
        let s = to_toon(&mems);
        let mut lines = s.lines();
        let header = lines.next().unwrap();
        assert!(header.starts_with("memories[3]{"));
        assert!(header.contains("id,topic,importance,weight,summary,keywords"));
        assert_eq!(lines.clone().count(), 3, "one row per memory");
    }

    #[test]
    fn toon_escapes_commas_and_quotes() {
        assert_eq!(toon_escape("plain"), "plain");
        assert_eq!(toon_escape("a, b"), "\"a, b\"");
        assert_eq!(toon_escape("she said \"hi\""), "\"she said \"\"hi\"\"\"");
    }

    #[test]
    fn all_formats_serialize_without_panic() {
        let mems = synthetic_memories(4);
        assert!(!to_json_pretty(&mems).unwrap().is_empty());
        assert!(!to_json_compact(&mems).unwrap().is_empty());
        assert!(!to_jsonl(&mems).unwrap().is_empty());
        assert!(!to_toml(&mems).unwrap().is_empty());
        assert!(!to_toon(&mems).is_empty());
        assert!(!to_compact(&mems).is_empty());
    }

    #[test]
    fn estimate_tokens_is_chars_div4_ceil() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
