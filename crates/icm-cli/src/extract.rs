//! Rule-based extraction and context injection for auto-extraction layers.
//!
//! Layer 0: Extract facts from text using keyword scoring (zero LLM cost,
//! English-only). When an `Embedder` is supplied, the keyword scorer is
//! replaced by [`crate::extract_semantic::SemanticScorer`] which works
//! cross-lingually via the multilingual embedder.
//! Layer 2: Recall and format context for prompt injection.

use std::collections::HashSet;

use anyhow::Result;
use icm_core::{is_preference_topic, project_matches, Embedder, Importance, Memory, MemoryStore};
use icm_store::SqliteStore;

use crate::extract_semantic::{AnchorKind, SemanticScorer};

/// `(topic, content, importance, anchor_kind)` — internal shape for
/// the unified scoring pipeline. Kind is `None` for facts produced by
/// the keyword scorer (legacy English path).
type ScoredFact = (String, String, Importance, Option<AnchorKind>);

/// Extract key facts from text and store them in ICM.
/// Returns the number of facts stored.
pub fn extract_and_store(store: &SqliteStore, text: &str, project: &str) -> Result<usize> {
    extract_and_store_with_opts(store, text, project, false, Importance::Critical)
}

/// Extract and store with option to store raw text as fallback.
///
/// `max_importance` clamps each extracted fact's auto-assigned importance.
/// Callers that ingest **untrusted** content (hook handlers reading
/// transcripts produced by the LLM and any tool it called) should pass
/// `Importance::Medium` so a malicious assistant message containing
/// `"DECISION: ..."` (which the rule-based extractor would otherwise
/// promote to High) cannot inject itself into wake-up packs as a
/// "critical decision". CLI / bench callers ingesting user-explicit
/// input pass `Importance::Critical` to disable the cap.
///
/// This variant uses the English-only keyword scorer. For multilingual
/// content prefer [`extract_and_store_with_embedder`].
pub fn extract_and_store_with_opts(
    store: &SqliteStore,
    text: &str,
    project: &str,
    store_raw: bool,
    max_importance: Importance,
) -> Result<usize> {
    extract_and_store_with_embedder(store, text, project, store_raw, max_importance, None)
}

/// Extract and store with optional embedder for multilingual scoring.
///
/// When `embedder` is `Some`, candidate sentences are scored against
/// semantic anchors via cosine similarity in the embedder's vector
/// space. The default model (`intfloat/multilingual-e5-base`) covers
/// 100+ languages, so a French decision sentence and its English
/// counterpart both match the decision anchor.
///
/// When `embedder` is `None` (e.g. `--no-embeddings` or the embedder
/// failed to load), the function falls back to the keyword scorer in
/// [`extract_facts`]. This preserves the pre-existing behaviour for
/// users who run with embeddings disabled. We also fall back if the
/// scorer's anchor-build call fails — better to extract English facts
/// only than to extract nothing at all.
pub fn extract_and_store_with_embedder(
    store: &SqliteStore,
    text: &str,
    project: &str,
    store_raw: bool,
    max_importance: Importance,
    embedder: Option<&dyn Embedder>,
) -> Result<usize> {
    let facts: Vec<ScoredFact> = match embedder {
        Some(emb) => match SemanticScorer::new(emb) {
            Ok(scorer) => extract_facts_semantic(text, project, emb, &scorer)
                .unwrap_or_else(|_| extract_facts_with_kind(text, project)),
            Err(_) => extract_facts_with_kind(text, project),
        },
        None => extract_facts_with_kind(text, project),
    };

    let mut stored = 0;
    for (topic, content, importance, kind) in &facts {
        let mut mem = Memory::new(
            topic.clone(),
            content.clone(),
            cap_importance(*importance, max_importance),
        );
        if let Some(k) = kind {
            mem.keywords.push(k.as_tag().to_string());
        }
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

/// Adapter that wraps `extract_facts` so its output shape matches
/// the semantic path: `(topic, content, importance, Option<AnchorKind>)`.
fn extract_facts_with_kind(text: &str, project: &str) -> Vec<ScoredFact> {
    extract_facts(text, project)
        .into_iter()
        .map(|(t, c, i)| (t, c, i, None))
        .collect()
}

/// Score candidate sentences semantically and return the surviving
/// ones tagged with their matched anchor kind. Falls back to
/// `extract_facts_with_kind` if the embedder can't embed a batch.
fn extract_facts_semantic(
    text: &str,
    project: &str,
    embedder: &dyn Embedder,
    scorer: &SemanticScorer,
) -> Result<Vec<ScoredFact>> {
    let sentences = split_sentences(text);
    let candidates: Vec<&str> = sentences
        .iter()
        .filter(|s| {
            let len = s.chars().count();
            (20..=500).contains(&len)
        })
        .map(|s| s.as_str())
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let scored = scorer.score_batch(embedder, &candidates)?;

    let mut facts: Vec<ScoredFact> = Vec::new();
    for (sentence, result) in candidates.iter().zip(scored) {
        if let Some((kind, _margin)) = result {
            let dominated = facts
                .iter()
                .any(|(_, existing, _, _)| jaccard_similar(existing, sentence));
            if !dominated {
                facts.push((
                    format!("context-{project}"),
                    sentence.to_string(),
                    kind.importance(),
                    Some(kind),
                ));
            }
        }
    }

    Ok(facts)
}

/// Clamp `value` to at most `cap`. `Importance` doesn't derive `Ord` (it
/// would imply a numeric ranking that's not meaningful in all contexts),
/// so map locally to a partial order: Critical > High > Medium > Low.
fn cap_importance(value: Importance, cap: Importance) -> Importance {
    let rank = |i: Importance| match i {
        Importance::Critical => 4,
        Importance::High => 3,
        Importance::Medium => 2,
        Importance::Low => 1,
    };
    if rank(value) > rank(cap) {
        cap
    } else {
        value
    }
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

    // Per-memory and aggregate caps to bound the injection size.
    // Without these, a single oversized memory (e.g. 50KB summary)
    // produced a 50KB system-reminder injection on every prompt — a
    // 12k-token tax for one bad write. Users still see the head of
    // the summary; the tail is ellipsised. Aggregate cap is applied
    // after per-memory truncation so the bullet structure stays
    // readable even when many memories are recalled.
    const PER_MEMORY_CHAR_CAP: usize = 400;
    const AGGREGATE_CHAR_CAP: usize = 4_000;

    let mut ctx = String::from(
        "Here is context from previous analysis of this project. \
         Use it to answer efficiently without re-reading files.\n\n",
    );
    for mem in &relevant {
        let summary = if mem.summary.chars().count() > PER_MEMORY_CHAR_CAP {
            // Truncate at a UTF-8 boundary using char count, then add
            // an ellipsis. We deliberately don't try to break on word
            // or sentence boundaries — keeping the head of the text
            // verbatim is more honest about what's stored.
            let mut truncated: String = mem.summary.chars().take(PER_MEMORY_CHAR_CAP).collect();
            truncated.push_str(" […]");
            truncated
        } else {
            mem.summary.clone()
        };
        let line = format!("- {summary}\n");
        if ctx.len() + line.len() > AGGREGATE_CHAR_CAP {
            // Stop appending bullets — the aggregate cap dominates.
            // The user gets the most relevant memories first (the
            // caller already sorted by relevance) and a truncation
            // marker so they know more was available.
            ctx.push_str("- (recall context truncated — increase per-memory or aggregate caps)\n");
            break;
        }
        ctx.push_str(&line);
    }
    ctx.push_str("\n---\n\n");

    Ok(ctx)
}

/// Public wrapper for CLI dry-run that uses the semantic scorer
/// when an embedder is provided. Falls back to the keyword scorer
/// when `embedder` is `None` or anchor build fails. The third tuple
/// field is the matched anchor kind (or `None` for the keyword path)
/// so the dry-run output can show the user *why* each fact was
/// promoted.
pub fn extract_facts_public_with_embedder(
    text: &str,
    project: &str,
    embedder: Option<&dyn Embedder>,
) -> Vec<(String, String, Importance, Option<AnchorKind>)> {
    if let Some(emb) = embedder {
        if let Ok(scorer) = SemanticScorer::new(emb) {
            if let Ok(facts) = extract_facts_semantic(text, project, emb, &scorer) {
                return facts;
            }
        }
    }
    extract_facts_with_kind(text, project)
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

        // Reject AI narration / action-announcement sentences before they
        // hit the scorer. These trigger the existing keyword tables (e.g.
        // "I'm going to **deploy**" hits the `deployed` bucket) and end up
        // stored as if they were facts. Cheap prefix check on lowercase.
        // Researcher audit R03/R01: ~60% of `context-icm` noise on the prod
        // DB matched these patterns.
        if NARRATION_PREFIXES.iter().any(|p| lower.starts_with(p)) {
            continue;
        }

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

        // Decision / rationale language. R01 audit: closing-decision
        // language ("OK so we'll...", "let's go with...", "the call is...")
        // was missing — sentences that *finalize* a debate scored below
        // threshold and were dropped, even though they're the highest-value
        // memory in a session. Added here.
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
            "ok so we",
            "ok, so we",
            "so we'll",
            "so we will",
            "let's go with",
            "going with",
            "final answer:",
            "the call is",
            "settled on",
            "opted for",
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

/// Prefixes that mark a sentence as AI narration / action announcement
/// rather than a factual statement worth remembering. Case-insensitive
/// match on the *start* of the trimmed sentence — these are the patterns
/// LLMs emit while working ("Let me check…", "I'll now read…") that the
/// keyword scorer otherwise mis-classifies as decisions or actions.
const NARRATION_PREFIXES: &[&str] = &[
    "let me ",
    "let's ",
    "i will ",
    "i'll ",
    "i'm going to ",
    "i am going to ",
    "i'm now ",
    "i am now ",
    "now i'll ",
    "now let me ",
    "next, i'll ",
    "next i'll ",
    "first, i'll ",
    "first i'll ",
    "first, let me ",
    "first let me ",
    "reading the ",
    "looking at the ",
    "checking the ",
    "running the ",
    "storing this ",
    "i should ",
    "i need to check",
    "i need to look",
    "i need to read",
    "i need to verify",
    "i'll start by ",
    "i will start by ",
    "let me start by ",
    "let me check ",
];

/// English-language honorific abbreviations that end in `.` followed by a
/// space + capitalized name. The naive splitter used to break sentences
/// like `Mr. Smith joined the team.` into `... Mr.` + `Smith joined ...`.
/// We suppress sentence-boundary detection when the buffer ends with one
/// of these tokens (case-insensitive, preceded by whitespace or the
/// start of the buffer).
const HONORIFIC_PREFIXES: &[&str] = &[
    "Mr.", "Mrs.", "Ms.", "Dr.", "Prof.", "Sr.", "Jr.", "St.", "Mt.", "Rev.", "Hon.", "Lt.",
    "Sgt.", "Cpl.", "Pvt.", "Gen.", "Col.", "Maj.", "Capt.", "Cmdr.",
];

/// Return true iff `buf` ends in an honorific abbreviation that should
/// suppress sentence-boundary detection on the trailing dot.
fn ends_with_honorific(buf: &str) -> bool {
    let trimmed = buf.trim_end();
    for &h in HONORIFIC_PREFIXES {
        if trimmed.len() >= h.len() && trimmed.ends_with(h) {
            // Must be preceded by whitespace or be at the start, so we
            // don't accidentally match `weatherstr.` containing `St.`.
            let prefix_start = trimmed.len() - h.len();
            if prefix_start == 0
                || trimmed
                    .as_bytes()
                    .get(prefix_start - 1)
                    .map(|b| b.is_ascii_whitespace())
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

/// Split a block of text into sentence-sized chunks suitable for fact
/// extraction. The previous implementation split on every `.` or `\n`,
/// which truncated URLs at `https://github.`, file paths at `$HOME/.`,
/// version numbers at `0.10.32`, and surfaced markdown artifacts as
/// standalone "sentences". Those fragments then ended up in the store
/// and got replayed verbatim by the UserPromptSubmit hook, polluting
/// every prompt's context.
///
/// Rules:
/// - A `.`, `?`, or `!` is a sentence terminator **only if followed
///   by whitespace or end-of-input**. A `.` followed immediately by a
///   non-whitespace character (letter, digit, `/`, `:`, etc.) is part
///   of a URL, file path, version number, or abbreviation — keep going.
/// - `:` is *not* a terminator. In technical prose it overwhelmingly
///   introduces a clause (`Error: ...`, `Note: ...`, `Fixed: was
///   using ...`) rather than ending one. Splitting on `:` produced
///   two short fragments that both fell below `MIN_SENTENCE_LEN` and
///   the underlying fact was dropped.
/// - `\n` is a hard boundary (preserves the existing line-aware behaviour
///   for lists and tool output) but the resulting fragment goes through
///   `is_keepable_fragment` before being kept.
/// - Triple-backtick-fenced blocks (markdown code) are skipped entirely:
///   their content is rarely usable as a "fact" and tends to contain
///   noise (paths, JSON, etc.).
///
/// The kept fragments must (via `is_keepable_fragment`):
/// - Be at least `MIN_SENTENCE_LEN` chars long.
/// - Not start with a markdown artifact prefix (`> `, `- `, `- [`, `* `,
///   `+ `, `# `, ```` ``` ````, `|`).
/// - Not start with a subordinating-clause connector (`because `,
///   `rather than `, `instead of `, …) — those are mid-sentence cuts.
/// - Not start with a lowercase letter — likewise a mid-sentence cut.
/// - Not end in a dangling URL/path token (`://`, `/`, `\`, `=`).
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
        } else if matches!(ch, '.' | '?' | '!') {
            match next {
                None => true,
                Some(c) if c.is_whitespace() => {
                    // `.` followed by whitespace is *usually* a sentence
                    // boundary — but English honorifics like `Mr.`,
                    // `Mrs.`, `Dr.`, `Prof.`, `St.`, `Sr.`, `Jr.` are
                    // followed by a space + capitalized name, not a new
                    // sentence. Suppress the boundary in that case.
                    !ends_with_honorific(&current)
                }
                _ => false, // `.` inside URL/path/version/abbreviation
            }
        } else {
            // `:` is intentionally NOT a sentence terminator. In
            // technical prose it overwhelmingly introduces a clause
            // (`Error: ...`, `Note: ...`, `Fixed NOT_ANY condition: was
            // using ...`) rather than ending one. Splitting on `:`
            // produced two short fragments that both fell below
            // `MIN_SENTENCE_LEN`, dropping the underlying fact entirely.
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
    // `|` covers GFM table rows (header, separator, body) which are
    // structural even when they happen to contain a numeric "fact".
    if stripped.starts_with("> ")
        || stripped.starts_with("- [")
        || stripped.starts_with("- ")
        || stripped.starts_with("* ")
        || stripped.starts_with("+ ")
        || stripped.starts_with("# ")
        || stripped.starts_with("```")
        || stripped.starts_with('|')
    {
        return false;
    }

    // Subordinating-clause starters: a fragment beginning with one of
    // these is almost always a sentence cut mid-thought (log line wraps,
    // table cells dumped to text, partial quotes from larger blocks).
    // The keyword scorer would otherwise promote them to High because
    // of "because", "rather than", "instead of" — but those tokens
    // belong in the *middle* of a decision, not the start of a
    // standalone fact. Cheap lowercase prefix match.
    let lower = stripped.to_lowercase();
    if SUBORDINATING_STARTERS.iter().any(|p| lower.starts_with(p)) {
        return false;
    }

    // Lowercase-letter start is a strong signal of a fragment cut from
    // the *middle* of a larger sentence. Real prose almost always
    // capitalises the first letter; even technical names that start
    // lowercase (`iOS`, `eBPF`, `npm`) are rare enough as the *first
    // word of a complete sentence* that the false-negative cost is
    // small. The false-positive cost — surfacing `operator on libsql
    // query result rather than .ok()` or `the libsql FTS5 trigger
    // failed on UPDATE` as standalone "facts" — is large because the
    // keyword scorer would promote them to High via decision/error
    // tokens further down. Digits, quotes and other non-alphabetic
    // starts are allowed (e.g. `0.10.42 shipped with the dedup fix`,
    // `"DECISION:" lines from a transcript dump`).
    if let Some(first) = stripped.chars().next() {
        if first.is_alphabetic() && first.is_lowercase() {
            return false;
        }
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

/// Lowercase prefixes that mark a fragment as the tail of a sentence
/// rather than the start of one. Match is on `.starts_with(...)` after
/// `to_lowercase()`, so each entry must end with a trailing space (or
/// punctuation) to avoid eating a real word that happens to start with
/// the same letters (e.g. `because` must not match `becausewhen`).
const SUBORDINATING_STARTERS: &[&str] = &[
    "because ",
    "rather than ",
    "instead of ",
    "although ",
    "though ",
    "since ",
    "while ",
    "whereas ",
    "whether ",
    "due to ",
    "such that ",
    "so that ",
    "as if ",
    "as though ",
    "in order to ",
];

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
        let stored =
            extract_and_store_with_opts(&store, text, "test", true, Importance::Critical).unwrap();
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

    // ── Honorific allowlist regression tests (audit batch 7) ──────────

    #[test]
    fn split_keeps_honorific_titles_intact() {
        // Audit A5 finding: `Mr. Smith joined the team.` used to split
        // into `... Mr.` + `Smith joined the team.` Now we suppress the
        // boundary on common English honorifics.
        let text = "The architecture team led by Mr. Smith joined the project last quarter.";
        let chunks = split_sentences(text);
        assert_eq!(
            chunks.len(),
            1,
            "honorific split shouldn't break: {chunks:?}"
        );
        assert!(chunks[0].contains("Mr. Smith"));
    }

    #[test]
    fn split_keeps_dr_and_prof_honorifics() {
        // Same logic for Dr., Prof., and the others.
        let cases = [
            "Dr. Watson published a follow-up paper on the new architecture.",
            "Prof. Knuth still recommends Knuth-Bendix completion for these problems.",
            "St. Augustine FL hosts the team's annual offsite gathering each spring.",
        ];
        for text in &cases {
            let chunks = split_sentences(text);
            assert_eq!(
                chunks.len(),
                1,
                "honorific should not split the sentence: {text:?} -> {chunks:?}"
            );
        }
    }

    #[test]
    fn split_drops_markdown_table_rows() {
        // GFM table rows are structure, not facts. Both header and body
        // rows must be filtered out — they were observed leaking into
        // `context-icm` as standalone "memories" that contained nothing
        // but column titles or aggregated numbers.
        let cases = [
            "| Fin S2 (mai 2027) | ~1 815 K€ |",
            "| Path | p50 | Coût $ | Qualité output |",
            "| **V9** | E2E claude -p × 5 sessions | PASS partiel |",
            "|---|---|---|",
        ];
        for text in &cases {
            let chunks = split_sentences(text);
            assert!(
                chunks.is_empty(),
                "table row should be filtered: {text:?} -> {chunks:?}"
            );
        }
    }

    #[test]
    fn split_drops_subordinating_starters() {
        // Fragments cut from larger sentences and starting with a
        // subordinating conjunction should not be kept. They contain
        // decision-keyword vocabulary ("because", "rather than") that
        // the scorer would otherwise promote to High importance.
        let cases = [
            "because the keywords column was JSON, not TEXT.",
            "rather than using a HashMap for performance reasons here.",
            "instead of the previous SQLite-only implementation path.",
            "although the build pipeline still requires manual triggers.",
            "while the feature flag rollout was paused last week.",
            "due to the shared connection pool starvation pattern observed.",
        ];
        for text in &cases {
            let chunks = split_sentences(text);
            assert!(
                chunks.is_empty(),
                "subordinating-clause fragment should be filtered: {text:?} -> {chunks:?}"
            );
        }
    }

    #[test]
    fn extract_facts_drops_table_and_subordinating_fragments() {
        // End-to-end: the scoring layer must not promote these patterns.
        let inputs = [
            "| Fin S2 (mai 2027) | ~1 815 K€ |",
            "because the keywords column was JSON, not TEXT.",
            "rather than using a HashMap for performance reasons here.",
        ];
        for text in &inputs {
            let facts = extract_facts(text, "test");
            assert!(
                facts.is_empty(),
                "extractor must drop fragment: {text:?} -> {facts:?}"
            );
        }
    }

    #[test]
    fn split_drops_lowercase_start_fragments() {
        // Bench-quality probe outputs `operator on libsql query result
        // rather than .ok().` and `the libsql FTS5 trigger failed on
        // UPDATE because the keywords column was JSON, not TEXT.` —
        // both are sentence cuts that the keyword scorer would
        // promote to High via decision/error vocabulary. The
        // lowercase-start rule rejects them at the splitter.
        let cases = [
            "operator on libsql query result rather than .ok().",
            "the libsql FTS5 trigger failed on UPDATE because the keywords column was JSON, not TEXT.",
            "was using check_all instead of check_some in evaluator.rs throughout that module.",
        ];
        for text in &cases {
            let chunks = split_sentences(text);
            assert!(
                chunks.is_empty(),
                "lowercase-start fragment must be filtered: {text:?} -> {chunks:?}"
            );
        }
    }

    #[test]
    fn split_keeps_digit_or_quote_starts() {
        // Digit-led and quote-led sentences are common in technical
        // prose (version-led, transcript dumps) and must survive the
        // lowercase-start filter — that filter only fires on
        // alphabetic lowercase starts.
        let cases = [
            (
                "0.10.42 shipped with the dedup hotfix on develop yesterday.",
                "0.10.42",
            ),
            (
                "\"DECISION:\" lines from the transcript were promoted to High.",
                "DECISION",
            ),
            (
                "`icm consolidate` produces a summary that mentions key concepts.",
                "icm consolidate",
            ),
        ];
        for (text, marker) in &cases {
            let chunks = split_sentences(text);
            assert!(
                chunks.iter().any(|c| c.contains(marker)),
                "non-alphabetic-start sentence must survive: {text:?} -> {chunks:?}"
            );
        }
    }

    #[test]
    fn extract_facts_keeps_real_decision_with_subordinating_word_inside() {
        // Sanity: the subordinating-starter check rejects only fragments
        // that *begin* with one of these connectors. A real decision
        // sentence containing "because" mid-clause must still pass.
        let text = "We picked SQLite because Postgres was overkill for the embedded use case here.";
        let facts = extract_facts(text, "test");
        assert!(
            !facts.is_empty(),
            "real decision sentence with mid-clause `because` must survive"
        );
    }

    #[test]
    fn split_does_not_match_honorific_substring_in_word() {
        // `weatherstr.` should not be treated as ending in `St.` (the
        // suffix matches but isn't preceded by whitespace, so it's part
        // of a longer token). With honorific suppression off the dot
        // ends a sentence, so we expect the trailing clause to come
        // through as a kept chunk. We use an uppercase first letter on
        // the trailing clause so it survives `is_keepable_fragment`'s
        // lowercase-start filter — that filter is unrelated to the
        // honorific path under test.
        let text = "The weatherstr. Value contains the forecast string and is then logged.";
        let chunks = split_sentences(text);
        assert!(
            !chunks.is_empty(),
            "honorific NOT suppressed: weatherstr. dot should be a sentence boundary, leaving the trailing clause as a chunk: {chunks:?}"
        );
        assert!(
            chunks.iter().any(|c| c.contains("Value contains")),
            "trailing clause should survive: {chunks:?}"
        );
    }
}
