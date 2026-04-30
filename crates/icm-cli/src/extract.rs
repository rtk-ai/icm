//! Rule-based extraction and context injection for auto-extraction layers.
//!
//! Layer 0: Extract facts from text using keyword scoring (zero LLM cost).
//! Layer 2: Recall and format context for prompt injection.

use std::collections::HashSet;

use anyhow::Result;
use icm_core::{is_preference_topic, project_matches, Importance, Memory, MemoryStore};
use icm_store::SqliteStore;

/// Extract key facts from text and store them in ICM.
/// Returns the number of facts stored.
pub fn extract_and_store(store: &SqliteStore, text: &str, project: &str) -> Result<usize> {
    extract_and_store_with_opts(store, text, project, false)
}

/// Extract and store with option to store raw text as fallback.
pub fn extract_and_store_with_opts(
    store: &SqliteStore,
    text: &str,
    project: &str,
    store_raw: bool,
) -> Result<usize> {
    let facts = extract_facts(text, project);
    let mut stored = 0;
    for (topic, content, importance) in &facts {
        let mem = Memory::new(topic.clone(), content.clone(), *importance);
        store.store(mem)?;
        stored += 1;
    }

    // Fallback: store truncated raw text as low-importance memory
    if stored == 0 && store_raw && text.len() >= 50 {
        let raw = if text.len() > 2000 {
            &text[text.len() - 2000..]
        } else {
            text
        };
        let mem = Memory::new(
            format!("context-{project}"),
            raw.to_string(),
            Importance::Low,
        );
        store.store(mem)?;
        stored = 1;
    }

    Ok(stored)
}

/// Recall relevant memories and format as context preamble for prompt injection.
///
/// When `project` is `Some(name)`, results are restricted to memories whose
/// topic matches that project (segment-aware match via
/// [`icm_core::project_matches`]). Preference / identity topics are always
/// kept so global user guidance is not stripped. When `project` is `None`,
/// no project filtering is applied (back-compat with non-hook callers).
///
/// Issue: previously the hook concatenated the project name into the FTS
/// query as a soft scoring hint, which let high-FTS-score memories from
/// other projects bleed into the recalled context. The hard filter here
/// prevents cross-project leakage.
pub fn recall_context(
    store: &SqliteStore,
    query: &str,
    project: Option<&str>,
    limit: usize,
) -> Result<String> {
    let project_filter = |m: &Memory| -> bool {
        match project {
            None => true,
            Some("") => true,
            Some(p) => is_preference_topic(&m.topic) || project_matches(&m.topic, Some(p)),
        }
    };

    // Oversample FTS results so that filtering still leaves enough candidates.
    let fts_results = store.search_fts(query, limit.saturating_mul(4).max(limit))?;
    let filtered: Vec<Memory> = fts_results
        .into_iter()
        .filter(|m| project_filter(m))
        .take(limit)
        .collect();

    let relevant: Vec<_> = if filtered.is_empty() {
        // Fallback: get all (project-matching) memories sorted by weight.
        let topics = store.list_topics()?;
        let mut all = Vec::new();
        for (topic, _) in &topics {
            for mem in store.get_by_topic(topic)? {
                if project_filter(&mem) {
                    all.push(mem);
                }
            }
        }
        all.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all.truncate(limit);
        all
    } else {
        filtered
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
    extract_facts_with_threshold(text, project, 2.0, 20)
}

/// Extract key facts with configurable threshold and limit.
fn extract_facts_with_threshold(
    text: &str,
    project: &str,
    min_score: f32,
    max_facts: usize,
) -> Vec<(String, String, Importance)> {
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

        // --- Dev tool signals (Claude Code hook context) ---

        // Bug fixes and error resolution
        for kw in &[
            "fixed",
            "resolved",
            "bug",
            "workaround",
            "root cause",
            "regression",
            "patch",
        ] {
            if lower.contains(kw) {
                score += 2.5;
                importance = Importance::High;
            }
        }

        // Configuration and setup
        for kw in &[
            "configured",
            "installed",
            "migrated",
            "upgraded",
            "downgraded",
            "enabled",
            "disabled",
            "deprecated",
        ] {
            if lower.contains(kw) {
                score += 2.0;
            }
        }

        // Error patterns from tool outputs
        for kw in &[
            "error:",
            "failed:",
            "warning:",
            "panic",
            "traceback",
            "exception",
            "denied",
            "unauthorized",
            "not found",
            "timed out",
        ] {
            if lower.contains(kw) {
                score += 2.0;
            }
        }

        // Version and release info
        for kw in &[
            "version",
            "release",
            "v0.",
            "v1.",
            "v2.",
            "changelog",
            "breaking change",
        ] {
            if lower.contains(kw) {
                score += 1.5;
            }
        }

        // Provider / infrastructure context
        for kw in &[
            "provider",
            "database",
            "credential",
            "endpoint",
            "webhook",
            "api key",
            "token",
            "connection",
            "cluster",
        ] {
            if lower.contains(kw) {
                score += 1.5;
            }
        }

        // File paths and code locations
        if s.contains('/')
            && (lower.contains(".rs")
                || lower.contains(".ts")
                || lower.contains(".py")
                || lower.contains(".toml")
                || lower.contains(".json"))
        {
            score += 1.5;
        }

        // --- Conversational signals (PreCompact / transcript context) ---

        // Preferences and rules ("always do X", "never do Y", "use X instead of Y")
        for kw in &[
            "always ",
            "never ",
            "must ",
            "should not",
            "shouldn't",
            "don't ",
            "do not ",
            "prefer ",
            "avoid ",
            "make sure",
            "important to",
            "remember to",
            "rule:",
            "convention:",
        ] {
            if lower.contains(kw) {
                score += 2.5;
                importance = Importance::High;
            }
        }

        // Learnings and insights
        for kw in &[
            "learned",
            "realized",
            "turns out",
            "the trick is",
            "the fix was",
            "the solution",
            "the key is",
            "the problem was",
            "discovered that",
            "figured out",
            "gotcha",
            "caveat",
            "pitfall",
            "note to self",
        ] {
            if lower.contains(kw) {
                score += 2.5;
                importance = Importance::High;
            }
        }

        // Project decisions and context from conversation
        for kw in &[
            "we're using",
            "we use ",
            "we switched",
            "we migrated",
            "we deploy",
            "the project",
            "the repo ",
            "the codebase",
            "our stack",
            "we went with",
            "we picked",
        ] {
            if lower.contains(kw) {
                score += 2.5;
            }
        }

        // Constraints and blockers
        for kw in &[
            "doesn't work",
            "does not work",
            "won't work",
            "will not work",
            "incompatible",
            "breaks when",
            "only works",
            "doesn't support",
            "does not support",
            "limitation",
            "can't use",
            "cannot use",
            "not compatible",
            "not supported",
        ] {
            if lower.contains(kw) {
                score += 2.5;
                importance = Importance::High;
            }
        }

        if score >= min_score {
            scored.push((score, s.to_string(), importance));
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.truncate(max_facts.max(1) * 2); // Keep 2x for dedup pass

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

    facts.truncate(max_facts);
    facts
}

/// Minimum char count for a fragment to be kept after splitting.
const MIN_SENTENCE_LEN: usize = 30;

/// Split a block of text into sentence-sized chunks suitable for fact
/// extraction. The previous implementation split on every `.` or `\n`,
/// which truncated URLs at `https://github.`, file paths at `$HOME/.`,
/// version numbers at `0.10.32`, and surfaced markdown artifacts as
/// standalone "sentences". Those fragments then ended up in the store
/// and got replayed verbatim by the UserPromptSubmit hook, polluting
/// every prompt's context.
///
/// Rules:
/// - A `.`, `?`, `!`, or `:` is a sentence terminator **only if followed
///   by whitespace or end-of-input**. A `.` followed immediately by a
///   non-whitespace character (letter, digit, `/`, `:`, etc.) is part
///   of a URL, file path, version number, or abbreviation — keep going.
/// - `\n` is a hard boundary (preserves the existing line-aware behaviour
///   for lists and tool output) but the resulting fragment goes through
///   `is_keepable_fragment` before being kept.
/// - Triple-backtick-fenced blocks (markdown code) are skipped entirely:
///   their content is rarely usable as a "fact" and tends to contain
///   noise (paths, JSON, etc.).
///
/// The kept fragments must (via `is_keepable_fragment`):
/// - End in a sentence terminator (`.`, `?`, `!`, `:`).
/// - Not start with a markdown artifact prefix (`> `, `- `, `- [`, `* `,
///   `+ `, `# `).
/// - Be at least `MIN_SENTENCE_LEN` chars long.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_code_fence = false;

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        // Toggle code-fence state when we see ``` at the start of a line.
        let at_line_start = i == 0 || chars[i - 1] == '\n';
        if at_line_start
            && i + 2 < chars.len()
            && chars[i] == '`'
            && chars[i + 1] == '`'
            && chars[i + 2] == '`'
        {
            in_code_fence = !in_code_fence;
            // Skip past the rest of the fence-marker line.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            // Flush whatever we had buffered before the fence.
            let trimmed = current.trim().to_string();
            if is_keepable_fragment(&trimmed) {
                out.push(trimmed);
            }
            current.clear();
            i += 1; // step over the trailing '\n' if any
            continue;
        }

        if in_code_fence {
            i += 1;
            continue;
        }

        current.push(ch);

        let next = chars.get(i + 1).copied();
        let boundary = if ch == '\n' {
            true
        } else if matches!(ch, '.' | '?' | '!' | ':') {
            match next {
                None => true,
                Some(c) if c.is_whitespace() => true,
                _ => false, // `.` inside URL/path/version/abbreviation
            }
        } else {
            false
        };

        if boundary {
            let trimmed = current.trim().to_string();
            if is_keepable_fragment(&trimmed) {
                out.push(trimmed);
            }
            current.clear();
        }
        i += 1;
    }

    let trimmed = current.trim().to_string();
    if is_keepable_fragment(&trimmed) {
        out.push(trimmed);
    }

    out
}

/// Predicate for whether a candidate fragment from `split_sentences`
/// deserves to be kept and considered for fact extraction. Centralises
/// the rejections so the splitter stays focused on boundary detection.
///
/// We deliberately do NOT require a terminating punctuation here: many
/// real-world inputs (one-line tool outputs, log lines, terse user
/// notes) skip the trailing period. The URL/path/version-aware boundary
/// detection in `split_sentences` is what actually prevents the
/// truncation bugs we saw in production — this filter just strips
/// markdown structure and obvious junk.
fn is_keepable_fragment(s: &str) -> bool {
    if s.chars().count() < MIN_SENTENCE_LEN {
        return false;
    }

    let stripped = s.trim_start();

    // Markdown artifact line prefixes — these are structure, not facts.
    // Order matters: `- [` (task list) must be checked before `- `.
    if stripped.starts_with("> ")
        || stripped.starts_with("- [")
        || stripped.starts_with("- ")
        || stripped.starts_with("* ")
        || stripped.starts_with("+ ")
        || stripped.starts_with("# ")
        || stripped.starts_with("```")
    {
        return false;
    }

    // Catch dangling URL/path tokens at the end. Boundary detection
    // should have prevented these from forming, but this is cheap
    // belt-and-suspenders against future regressions.
    let last_word = s.split_whitespace().next_back().unwrap_or("");
    if last_word.ends_with("://")
        || last_word.ends_with('/')
        || last_word.ends_with('\\')
        || last_word.ends_with('=')
    {
        return false;
    }

    true
}

// ── Fact classification ──────────────────────────────────────────────────

/// Words that look capitalized mid-sentence but are not person/project names.
const ENTITY_STOP_WORDS: &[&str] = &[
    "The",
    "This",
    "That",
    "These",
    "Those",
    "When",
    "Where",
    "What",
    "Which",
    "How",
    "But",
    "And",
    "Also",
    "However",
    "Furthermore",
    "Moreover",
    "Therefore",
    "Because",
    "After",
    "Before",
    "During",
    "Since",
    "Until",
    "While",
    "Although",
    "Though",
    "Some",
    "Many",
    "Most",
    "Each",
    "Every",
    "All",
    "Any",
    "Both",
    "Other",
    "Here",
    "There",
    "Now",
    "Then",
    "Just",
    "Only",
    "Even",
    "Still",
    "Yet",
    "Yes",
    "No",
    "Not",
    "Can",
    "Could",
    "Would",
    "Should",
    "Will",
    "May",
    "Might",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
    "API",
    "CLI",
    "URL",
    "HTTP",
    "HTTPS",
    "JSON",
    "HTML",
    "CSS",
    "SQL",
    "SSH",
    "DNS",
    "TCP",
    "UDP",
    "TLS",
    "SSL",
    "REST",
    "GRPC",
    "OAuth",
    "JWT",
    "AWS",
    "GCP",
    "Azure",
    "Docker",
    "Linux",
    "Mac",
    "Windows",
    "Rust",
    "Python",
    "Java",
    "Node",
    "React",
    "Vue",
    "Svelte",
    "Next",
    "Git",
    "Github",
    "Gitlab",
    "Slack",
    "Chrome",
    "Firefox",
    "Safari",
    "Postgres",
    "Redis",
    "Mongo",
    "SQLite",
    "MySQL",
    "True",
    "False",
    "None",
    "Null",
    "Ok",
    "Err",
];

const ENTITY_PATTERNS_BEFORE: &[&str] = &[
    "with ",
    "from ",
    "ask ",
    "tell ",
    "cc ",
    "ping ",
    "by ",
    "per ",
    "asked ",
    "told ",
    "called ",
    "messaged ",
    "emailed ",
    "thank ",
    "thanks ",
];

const ENTITY_PATTERNS_AFTER: &[&str] = &[
    " said",
    " mentioned",
    " suggested",
    " proposed",
    " agreed",
    " thinks",
    " wants",
    " noted",
    " confirmed",
    " replied",
    " approved",
    " reviewed",
    " reported",
    " fixed",
    " built",
    "'s ",
    "'s,",
    "'s.",
    "'s:",
];

/// Classify a fact into kind tags based on keyword matching.
pub fn classify_fact(content: &str) -> Vec<String> {
    let lower = content.to_lowercase();
    let mut tags = Vec::new();

    let decision_kw = [
        "decided",
        "chose",
        "chosen",
        "went with",
        "picked",
        "switched to",
        "instead of",
        "rather than",
        "trade-off",
        "trade off",
        "we chose",
        "settled on",
        "opted for",
    ];
    if decision_kw.iter().any(|kw| lower.contains(kw)) {
        tags.push("kind:decision".to_string());
    }

    let preference_kw = [
        "always ",
        "never ",
        "prefer ",
        "avoid ",
        "convention:",
        "rule:",
        "make sure",
        "important to",
        "must ",
        "should not",
        "shouldn't",
        "don't ",
        "do not ",
    ];
    if preference_kw.iter().any(|kw| lower.contains(kw)) {
        tags.push("kind:preference".to_string());
    }

    let problem_kw = [
        "bug",
        "error:",
        "failed:",
        "root cause",
        "workaround",
        "doesn't work",
        "does not work",
        "breaks when",
        "limitation",
        "incompatible",
        "regression",
        "crash",
    ];
    if problem_kw.iter().any(|kw| lower.contains(kw)) {
        tags.push("kind:problem".to_string());
    }

    let milestone_kw = [
        "shipped",
        "launched",
        "deployed",
        "released",
        "migrated",
        "completed",
        "go live",
        "went live",
        "milestone",
        "v1.",
        "v2.",
        "v3.",
    ];
    if milestone_kw.iter().any(|kw| lower.contains(kw)) {
        tags.push("kind:milestone".to_string());
    }

    tags
}

/// Detect person/project entity names in text using heuristic patterns.
pub fn detect_entities(content: &str) -> Vec<String> {
    let stop_set: HashSet<&str> = ENTITY_STOP_WORDS.iter().copied().collect();
    let mut found: HashSet<String> = HashSet::new();

    let words: Vec<&str> = content.split_whitespace().collect();
    let lower = content.to_lowercase();

    for (i, word) in words.iter().enumerate() {
        let clean = word
            .trim_start_matches('@')
            .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '\'');
        let base = clean.trim_end_matches("'s").trim_end_matches("\u{2019}s");
        if base.len() < 2 || base.len() > 15 {
            continue;
        }
        let first = base.chars().next().unwrap_or('a');
        if !first.is_uppercase() {
            continue;
        }
        if base.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) {
            continue;
        }
        if i == 0 {
            continue;
        }
        if i > 0 {
            let prev = words[i - 1];
            if prev.ends_with('.') || prev.ends_with('!') || prev.ends_with('?') {
                continue;
            }
        }
        if stop_set.contains(base) {
            continue;
        }

        let is_possessive = clean.ends_with("'s") || clean.ends_with("\u{2019}s");
        let base_lower = base.to_lowercase();

        let has_before_pattern = ENTITY_PATTERNS_BEFORE.iter().any(|pat| {
            let search = format!("{pat}{base_lower}");
            lower.contains(&search)
        });
        let has_after_pattern = ENTITY_PATTERNS_AFTER.iter().any(|pat| {
            let search = format!("{base_lower}{pat}");
            lower.contains(&search)
        });
        let has_mention = content.contains(&format!("@{base}"));

        if has_before_pattern || has_after_pattern || has_mention || is_possessive {
            found.insert(base.to_string());
        }
    }

    found
        .into_iter()
        .map(|name| format!("entity:{name}"))
        .collect()
}

/// Extract facts with classification and entity detection.
pub fn extract_and_classify(
    text: &str,
    project: &str,
) -> Vec<(String, String, Importance, Vec<String>)> {
    let facts = extract_facts(text, project);
    let global_entities = detect_entities(text);

    facts
        .into_iter()
        .map(|(topic, content, importance)| {
            let mut extra_kw = classify_fact(&content);
            let local_entities = detect_entities(&content);
            for e in local_entities {
                if !extra_kw.contains(&e) {
                    extra_kw.push(e);
                }
            }
            for e in &global_entities {
                let name = e.strip_prefix("entity:").unwrap_or(e);
                if content.contains(name) && !extra_kw.contains(e) {
                    extra_kw.push(e.clone());
                }
            }
            (topic, content, importance, extra_kw)
        })
        .collect()
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
    fn test_extract_dev_signals_bugfix() {
        let text =
            "Fixed NOT_ANY condition: was using check_all instead of check_some in evaluator.rs";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract bug fix facts");
    }

    #[test]
    fn test_extract_dev_signals_error() {
        let text = "Error: Provider configuration error: Missing Proxmox VE API Endpoint";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract error facts");
    }

    #[test]
    fn test_extract_dev_signals_config() {
        let text = "Configured release-please for Cargo workspace with simple release-type instead of rust";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract config facts");
    }

    #[test]
    fn test_extract_conversational_preference() {
        let text = "Always query ALL calendars in Google Calendar API, not just primary";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract preference/rule");
    }

    #[test]
    fn test_extract_conversational_learning() {
        let text = "Turns out the hooks must be bash scripts not bun or TypeScript for Claude Code";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract learning/insight");
    }

    #[test]
    fn test_extract_conversational_constraint() {
        let text =
            "The fastembed crate does not work with cross-compilation for ARM64 on Linux CI runners";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract constraint");
    }

    #[test]
    fn test_extract_conversational_decision() {
        let text = "We switched from OpenAI embeddings to fastembed because we wanted zero external dependencies";
        let facts = extract_facts(text, "test");
        assert!(!facts.is_empty(), "should extract project decision");
    }

    #[test]
    fn test_store_raw_fallback() {
        let store = SqliteStore::in_memory().unwrap();
        let text = "Just some random conversation text that has no particular keywords but is still somewhat meaningful context about the ongoing work session";
        let stored = extract_and_store_with_opts(&store, text, "test", true).unwrap();
        assert_eq!(stored, 1, "should store raw text as fallback");
    }

    #[test]
    fn test_recall_context_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let ctx = recall_context(&store, "anything", None, 5).unwrap();
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

        let ctx = recall_context(&store, "parsing algorithm", None, 5).unwrap();
        assert!(!ctx.is_empty());
        assert!(ctx.contains("Pratt") || ctx.contains("parsing") || ctx.contains("algorithm"));
    }

    #[test]
    fn test_recall_context_filters_other_projects() {
        // Two projects' memories share search terms; the project filter must
        // strip the cross-project hits from FTS results.
        let store = SqliteStore::in_memory().unwrap();

        let mem_a = Memory::new(
            "context-projecta".to_string(),
            "Auth refactor switched to OIDC bearer tokens".to_string(),
            Importance::High,
        );
        let mem_b = Memory::new(
            "context-projectb".to_string(),
            "Auth refactor moved to session cookies".to_string(),
            Importance::High,
        );
        store.store(mem_a).unwrap();
        store.store(mem_b).unwrap();

        let ctx = recall_context(&store, "Auth refactor", Some("projecta"), 5).unwrap();
        assert!(ctx.contains("OIDC"), "must include projecta memory");
        assert!(
            !ctx.contains("session cookies"),
            "must NOT leak projectb memory: {ctx}"
        );
    }

    #[test]
    fn test_recall_context_keeps_preferences_when_filtering() {
        let store = SqliteStore::in_memory().unwrap();

        let project_mem = Memory::new(
            "context-myapp".to_string(),
            "Deployment uses Helm charts".to_string(),
            Importance::High,
        );
        let pref_mem = Memory::new(
            "preferences".to_string(),
            "User prefers terse responses".to_string(),
            Importance::Critical,
        );
        store.store(project_mem).unwrap();
        store.store(pref_mem).unwrap();

        let ctx = recall_context(&store, "deployment", Some("myapp"), 5).unwrap();
        assert!(ctx.contains("Helm"));
        // Preferences are global, so they survive project filtering even when
        // the FTS query is unrelated.
        let ctx2 = recall_context(&store, "anything random", Some("myapp"), 5).unwrap();
        assert!(
            ctx2.contains("terse responses"),
            "preferences must not be stripped by project filter: {ctx2}"
        );
    }

    #[test]
    fn test_recall_context_no_project_keeps_all() {
        // When no project is specified, behavior matches the pre-filter API:
        // every matching memory is returned regardless of topic.
        let store = SqliteStore::in_memory().unwrap();

        store
            .store(Memory::new(
                "context-projecta".to_string(),
                "Token refresh logic".to_string(),
                Importance::High,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "context-projectb".to_string(),
                "Token rotation policy".to_string(),
                Importance::High,
            ))
            .unwrap();

        let ctx = recall_context(&store, "Token", None, 5).unwrap();
        assert!(ctx.contains("refresh"));
        assert!(ctx.contains("rotation"));
    }

    #[test]
    fn test_classify_decision() {
        let tags = classify_fact("We decided to use SQLite instead of Postgres for simplicity");
        assert!(tags.contains(&"kind:decision".to_string()));
    }

    #[test]
    fn test_classify_preference() {
        let tags = classify_fact("Always use snake_case for Rust function names");
        assert!(tags.contains(&"kind:preference".to_string()));
    }

    #[test]
    fn test_classify_problem() {
        let tags = classify_fact("Bug: the connection pool doesn't work under high concurrency");
        assert!(tags.contains(&"kind:problem".to_string()));
    }

    #[test]
    fn test_classify_milestone() {
        let tags = classify_fact("We shipped v2.0 to production last Friday");
        assert!(tags.contains(&"kind:milestone".to_string()));
    }

    #[test]
    fn test_detect_entities_conversational() {
        let entities =
            detect_entities("The API was slow, Sarah mentioned it, and Bob's PR fixed it");
        assert!(entities.contains(&"entity:Sarah".to_string()));
        assert!(entities.contains(&"entity:Bob".to_string()));
    }

    #[test]
    fn test_detect_entities_no_false_positives() {
        let entities = detect_entities("The HTTP API returns JSON when called on Monday");
        assert!(entities.is_empty());
    }

    #[test]
    fn test_detect_entities_at_mention() {
        let entities = detect_entities("Can you check with @Alice on the deployment");
        assert!(entities.contains(&"entity:Alice".to_string()));
    }

    #[test]
    fn test_extract_and_classify_enriches() {
        let text = "We decided to use SQLite because Postgres was overkill";
        let results = extract_and_classify(text, "test");
        assert!(!results.is_empty());
        let (_, _, _, kw) = &results[0];
        assert!(kw.iter().any(|k| k.starts_with("kind:")));
    }

    // ── Regression tests for the splitter ──────────────────────────────
    // Each input here was actually observed in a real session of this
    // assistant; the previous splitter produced truncated garbage that
    // got replayed into every prompt's context.

    #[test]
    fn split_keeps_url_intact() {
        // Was previously cut at "https://github." because of the dot
        // before "com". The full sentence must come back as one piece.
        let text = "PR ouverte : **https://github.com/rtk-ai/icm/pull/136**.";
        let chunks = split_sentences(text);
        assert!(!chunks.is_empty(), "URL sentence dropped entirely");
        assert!(
            chunks.iter().any(|c| c.contains("github.com/rtk-ai/icm")),
            "URL was truncated mid-domain: {chunks:?}"
        );
        // And no fragment is just the truncated `https://github.` part.
        assert!(
            !chunks.iter().any(|c| c.ends_with("github.")),
            "split surfaced a truncated `github.` fragment: {chunks:?}"
        );
    }

    #[test]
    fn split_keeps_path_intact() {
        // Was previously cut at "$HOME/." because of the dot before "icm".
        let text =
            "Set the DB location with `export ICM_DATABASE_URL=\"file:$HOME/.icm/memories.db\"`.";
        let chunks = split_sentences(text);
        assert!(!chunks.is_empty());
        assert!(
            chunks.iter().any(|c| c.contains("$HOME/.icm/memories.db")),
            "path was truncated: {chunks:?}"
        );
    }

    #[test]
    fn split_keeps_version_intact() {
        // Version numbers like 0.10.32 used to split at every dot.
        let text = "We just released icm 0.10.32 with the audit batch fixes.";
        let chunks = split_sentences(text);
        assert!(chunks.iter().any(|c| c.contains("0.10.32")));
    }

    #[test]
    fn split_drops_blockquote_lines() {
        // Lines starting with `> ` are quoted prose, not new facts.
        let text = "> Hey, we just documented this in the README — see the new section.";
        let chunks = split_sentences(text);
        assert!(
            chunks.is_empty(),
            "blockquote line should be filtered out: {chunks:?}"
        );
    }

    #[test]
    fn split_drops_task_list_lines() {
        // Markdown task-list bullets are structure, not facts.
        let text = "- [ ] Spot-check 2-3 localized READMEs render correctly (Arabic RTL, CJK).";
        let chunks = split_sentences(text);
        assert!(
            chunks.is_empty(),
            "task-list line should be filtered out: {chunks:?}"
        );
    }

    #[test]
    fn split_drops_dangling_url_token() {
        // Belt-and-suspenders: even if a future bug somehow lets a
        // mid-URL split through, the trailing-token check catches it.
        let text = "Open the issue at https://github.com/rtk-ai/icm/pull/";
        let chunks = split_sentences(text);
        assert!(
            chunks.is_empty(),
            "fragment ending in dangling `/` should be filtered: {chunks:?}"
        );
    }

    #[test]
    fn split_skips_code_fence_content() {
        // Triple-backtick blocks are code, not prose. Their contents
        // shouldn't pollute the fact stream even if they happen to look
        // sentence-shaped.
        let text = "Here is the snippet:\n\
                    ```\n\
                    let x = 1;\n\
                    println!(\"value: {}\", x);\n\
                    ```\n\
                    The output is what you'd expect for an integer literal.";
        let chunks = split_sentences(text);
        assert!(chunks.iter().any(|c| c.contains("output is what")));
        assert!(
            !chunks.iter().any(|c| c.contains("println!")),
            "code-fence content leaked: {chunks:?}"
        );
    }

    #[test]
    fn split_keeps_two_real_sentences_intact() {
        // Sanity: the splitter must still produce sentences for normal prose.
        let text = "The parser uses Pratt precedence. \
                     It handles right-associative operators well.";
        let chunks = split_sentences(text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("Pratt"));
        assert!(chunks[1].contains("right-associative"));
    }
}
