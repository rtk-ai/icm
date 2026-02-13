//! Rule-based extraction and context injection for auto-extraction layers.
//!
//! Layer 0: Extract facts from text using keyword scoring (zero LLM cost).
//! Layer 2: Recall and format context for prompt injection.

use std::collections::HashSet;

use anyhow::Result;
use icm_core::{Importance, Memory, MemoryStore};
use icm_store::SqliteStore;

/// Extract key facts from text and store them in ICM.
/// Returns the number of facts stored.
pub fn extract_and_store(store: &SqliteStore, text: &str, project: &str) -> Result<usize> {
    let facts = extract_facts(text, project);
    let mut stored = 0;
    for (topic, content, importance) in &facts {
        let mem = Memory::new(topic.clone(), content.clone(), *importance);
        store.store(mem)?;
        stored += 1;
    }
    Ok(stored)
}

/// Recall relevant memories and format as context preamble for prompt injection.
pub fn recall_context(store: &SqliteStore, query: &str, limit: usize) -> Result<String> {
    // Try FTS search with the query
    let results = store.search_fts(query, limit * 2)?;

    let relevant: Vec<_> = if results.is_empty() {
        // Fallback: get all memories sorted by weight
        let topics = store.list_topics()?;
        let mut all = Vec::new();
        for (topic, _) in &topics {
            all.extend(store.get_by_topic(topic)?);
        }
        all.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap());
        all.truncate(limit);
        all
    } else {
        results.into_iter().take(limit).collect()
    };

    if relevant.is_empty() {
        return Ok(String::new());
    }

    let mut ctx = String::from(
        "Here is context from previous analysis of this project. \
         Use it to answer efficiently without re-reading files.\n\n",
    );
    for mem in &relevant {
        ctx.push_str(&format!("- {}\n", mem.summary));
    }
    ctx.push_str("\n---\n\n");

    Ok(ctx)
}

/// Public wrapper for CLI dry-run display.
pub fn extract_facts_public(text: &str, project: &str) -> Vec<(String, String, Importance)> {
    extract_facts(text, project)
}

/// Extract key facts from text using keyword scoring.
fn extract_facts(text: &str, project: &str) -> Vec<(String, String, Importance)> {
    let sentences = split_sentences(text);
    let mut scored: Vec<(f32, String, Importance)> = Vec::new();

    for sentence in &sentences {
        let s = sentence.trim();
        if s.len() < 20 || s.len() > 500 {
            continue;
        }

        let lower = s.to_lowercase();
        let mut score = 0.0f32;
        let mut importance = Importance::Medium;

        // --- Generic knowledge signals ---

        // Sentences with numbers are often factual (ports, sizes, dates, metrics)
        let has_numbers = s.chars().any(|c| c.is_ascii_digit());
        if has_numbers {
            score += 1.5;
        }

        // Sentences with proper nouns (capitalized words not at sentence start)
        let words: Vec<&str> = s.split_whitespace().collect();
        let proper_nouns = words
            .iter()
            .skip(1)
            .filter(|w| {
                w.len() > 1
                    && w.chars().next().is_some_and(|c| c.is_uppercase())
                    && !w.chars().all(|c| c.is_uppercase()) // skip ALL_CAPS
            })
            .count();
        if proper_nouns >= 2 {
            score += 1.5;
        }

        // Definitions and specifications
        for kw in &[
            "maximum",
            "minimum",
            "default",
            "requires",
            "supports",
            "timeout",
            "threshold",
            "configured",
            "limited by",
            "port",
            "nodes",
            "cluster",
            "protocol",
            "phase",
        ] {
            if lower.contains(kw) {
                score += 1.5;
            }
        }

        // Architecture / design keywords
        for kw in &[
            "architecture",
            "module",
            "pipeline",
            "component",
            "design",
            "structure",
            "layer",
            "implementation",
            "deployed",
            "system",
            "framework",
            "model",
        ] {
            if lower.contains(kw) {
                score += 2.0;
            }
        }

        // Algorithm / technical depth
        for kw in &[
            "algorithm",
            "implements",
            "complexity",
            "o(n",
            "recursive",
            "tolerance",
            "consensus",
            "replication",
            "latency",
            "throughput",
            "bandwidth",
            "fault",
        ] {
            if lower.contains(kw) {
                score += 3.0;
                importance = Importance::High;
            }
        }

        // Decision / rationale language
        for kw in &[
            "chose",
            "chosen",
            "decided",
            "because",
            "instead of",
            "trade-off",
            "rather than",
            "reason",
            "motivated",
            "proposed",
            "introduced",
            "invented",
        ] {
            if lower.contains(kw) {
                score += 2.5;
                importance = Importance::High;
            }
        }

        // Performance data
        for kw in &[
            "benchmark",
            "performance",
            "measured",
            "achieves",
            "tps",
            "latency",
            "availability",
            "scales",
        ] {
            if lower.contains(kw) {
                score += 2.0;
            }
        }

        // Named entities and references
        for kw in &[
            "licensed",
            "published",
            "paper",
            "team",
            "professor",
            "university",
            "company",
            "stanford",
            "mit",
        ] {
            if lower.contains(kw) {
                score += 1.0;
            }
        }

        // Code references
        if lower.contains(".rs") || lower.contains("fn ") || lower.contains("struct ") {
            score += 1.0;
        }

        if score >= 3.0 {
            scored.push((score, s.to_string(), importance));
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.truncate(30);

    // Dedup similar sentences
    let mut facts: Vec<(String, String, Importance)> = Vec::new();
    for (_score, content, importance) in scored {
        let dominated = facts
            .iter()
            .any(|(_, existing, _)| jaccard_similar(existing, &content));
        if !dominated {
            facts.push((format!("context-{project}"), content, importance));
        }
    }

    facts.truncate(20);
    facts
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if ch == '.' || ch == '\n' {
            let trimmed = current.trim().to_string();
            if trimmed.len() > 15 {
                sentences.push(trimmed);
            }
            current = String::new();
        }
    }

    let trimmed = current.trim().to_string();
    if trimmed.len() > 15 {
        sentences.push(trimmed);
    }

    sentences
}

fn jaccard_similar(a: &str, b: &str) -> bool {
    let a_words: HashSet<&str> = a.split_whitespace().collect();
    let b_words: HashSet<&str> = b.split_whitespace().collect();
    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();
    if union == 0 {
        return true;
    }
    (intersection as f64 / union as f64) > 0.6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_facts_finds_algorithm() {
        let text = "The parser uses Pratt precedence climbing algorithm. \
                     It handles right-associative operators like exponentiation. \
                     The code is clean.";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty());
        assert!(facts[0].1.contains("Pratt"));
    }

    #[test]
    fn test_extract_facts_skips_short() {
        let text = "OK. Yes. Fine.";
        let facts = extract_facts(text, "test");
        assert!(facts.is_empty());
    }

    #[test]
    fn test_jaccard_similar() {
        assert!(jaccard_similar(
            "The parser uses Pratt algorithm",
            "The parser uses Pratt algorithm for parsing"
        ));
        assert!(!jaccard_similar(
            "Matrix operations use cofactor expansion",
            "Complex numbers use conjugate division"
        ));
    }

    #[test]
    fn test_recall_context_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let ctx = recall_context(&store, "anything", 5).unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_extract_and_recall_roundtrip() {
        let store = SqliteStore::in_memory().unwrap();
        let text = "The project uses Pratt precedence climbing algorithm for parsing expressions. \
                     Welford's online algorithm provides streaming mean and variance computation. \
                     The matrix determinant uses recursive cofactor expansion along the first row.";
        let stored = extract_and_store(&store, text, "mathlib").unwrap();
        assert!(stored > 0);

        let ctx = recall_context(&store, "parsing algorithm", 5).unwrap();
        assert!(!ctx.is_empty());
        assert!(ctx.contains("Pratt") || ctx.contains("parsing") || ctx.contains("algorithm"));
    }
}
