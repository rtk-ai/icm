//! Wake-up pack: build a compact, token-budgeted snapshot of critical memories
//! suitable for injection into an LLM system prompt at session start.
//!
//! Inspired by MemPalace's 4-layer memory stack and `wake-up` command, adapted
//! to ICM's importance/decay model.

use chrono::{DateTime, Utc};

use crate::error::IcmResult;
use crate::memory::{Importance, Memory};
use crate::store::MemoryStore;

/// Output format for the wake-up pack.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WakeUpFormat {
    /// Plain text bullet list, no headers.
    Plain,
    /// Markdown with section headers (default).
    #[default]
    Markdown,
}

/// Options for building a wake-up pack.
#[derive(Debug, Clone)]
pub struct WakeUpOptions<'a> {
    /// Optional project filter (matched as substring against topic).
    pub project: Option<&'a str>,
    /// Approximate token budget (1 token ≈ 4 chars).
    pub max_tokens: usize,
    /// Output format.
    pub format: WakeUpFormat,
    /// Include preference/identity memories regardless of project filter.
    pub include_preferences: bool,
}

impl Default for WakeUpOptions<'_> {
    fn default() -> Self {
        Self {
            project: None,
            max_tokens: 500,
            format: WakeUpFormat::Markdown,
            include_preferences: true,
        }
    }
}

/// Category a memory lands in for the wake-up rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    Identity,
    Decision,
    Constraint,
    Error,
    Milestone,
    Context,
}

impl Category {
    /// Section header label.
    fn label(self) -> &'static str {
        match self {
            Self::Identity => "Identity & preferences",
            Self::Decision => "Critical decisions",
            Self::Constraint => "Active constraints",
            Self::Error => "Recent errors resolved",
            Self::Milestone => "Milestones",
            Self::Context => "Project context",
        }
    }

    fn all_ordered() -> [Category; 6] {
        [
            Self::Identity,
            Self::Decision,
            Self::Constraint,
            Self::Error,
            Self::Milestone,
            Self::Context,
        ]
    }
}

#[derive(Debug, Clone)]
struct ScoredMemory {
    memory: Memory,
    score: f32,
    category: Category,
}

/// Build a wake-up pack by querying the store directly.
///
/// Selects critical/high memories optionally scoped by project, ranks them by
/// `importance * recency * weight`, then truncates to fit `max_tokens`.
pub fn build_wake_up<S: MemoryStore + ?Sized>(
    store: &S,
    opts: &WakeUpOptions<'_>,
) -> IcmResult<String> {
    let all = store.list_all()?;
    Ok(build_wake_up_from_memories(all, opts))
}

/// Build a wake-up pack from an in-memory list (pure, testable).
#[must_use]
pub fn build_wake_up_from_memories(memories: Vec<Memory>, opts: &WakeUpOptions<'_>) -> String {
    let now = Utc::now();

    let mut candidates: Vec<ScoredMemory> = memories
        .into_iter()
        .filter(|m| {
            let is_pref = opts.include_preferences && is_preference_topic(&m.topic);
            // Critical/high are always eligible; preferences always eligible
            // when the option is set (they may be medium-importance).
            matches!(m.importance, Importance::Critical | Importance::High) || is_pref
        })
        .filter(|m| project_matches(&m.topic, opts.project))
        .map(|m| {
            let score = compute_score(&m, now);
            let category = categorize(&m);
            ScoredMemory {
                memory: m,
                score,
                category,
            }
        })
        .collect();

    // Ties broken by id (lexicographic / ULID-monotonic) so the wake-up
    // ordering stays deterministic — required for the prompt-cache prefix
    // match to survive across runs with equal-scored memories.
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.memory.id.cmp(&b.memory.id))
    });

    // Token budget: 1 token ≈ 4 characters (rough approximation).
    let max_chars = opts.max_tokens.saturating_mul(4);
    let selected = truncate_by_budget(candidates, max_chars);

    render(&selected, opts)
}

/// Return true when a topic looks like it holds identity/preference data.
///
/// Preference topics are global (not scoped to a single project) and are
/// always included by `project_matches` so user identity / cross-project
/// guidance is not stripped out by project filtering.
pub fn is_preference_topic(topic: &str) -> bool {
    let lower = topic.to_lowercase();
    lower == "preferences"
        || lower == "identity"
        || lower.starts_with("preferences-")
        || lower.starts_with("preferences.")
        || lower.contains("preference")
        || lower.starts_with("user-")
        || lower.starts_with("user.")
}

/// Return true if a topic matches the project filter, or if the filter is
/// absent. Matching is **segment-aware**: both the topic and the project
/// filter are split on `-`, `.`, `_`, `/`, `:` and every project segment must
/// appear as a complete topic segment (case-insensitive). This avoids false
/// positives like `"icm"` matching `"icmp-notes"` while still allowing
/// `"icm"` to match `"decisions-icm-core"` (via the `"icm"` segment) and
/// `"icm-core"` to match `"decisions-icm-core"` (via both segments).
pub fn project_matches(topic: &str, project: Option<&str>) -> bool {
    let Some(proj) = project else {
        return true;
    };
    if proj.is_empty() {
        return true;
    }
    // Preferences/identity are global and always included when filtering.
    if is_preference_topic(topic) {
        return true;
    }

    let is_delim = |c: char| matches!(c, '-' | '.' | '_' | '/' | ':');
    let topic_segs: Vec<String> = topic
        .split(is_delim)
        .map(str::to_lowercase)
        .filter(|s| !s.is_empty())
        .collect();
    let proj_segs: Vec<String> = proj
        .split(is_delim)
        .map(str::to_lowercase)
        .filter(|s| !s.is_empty())
        .collect();

    if proj_segs.is_empty() {
        return true;
    }

    proj_segs.iter().all(|ps| topic_segs.contains(ps))
}

fn compute_score(m: &Memory, now: DateTime<Utc>) -> f32 {
    let importance_weight = match m.importance {
        Importance::Critical => 10.0,
        Importance::High => 5.0,
        Importance::Medium => 2.0,
        Importance::Low => 0.5,
    };
    // Recency is access-aware: use the more recent of created_at and
    // last_accessed, so memories that are frequently recalled stay fresh.
    let reference = m.created_at.max(m.last_accessed);
    let days = (now - reference).num_days().max(0) as f32;
    // Recency factor: 1.0 at day 0, ~0.5 at day 30, ~0.25 at day 90.
    let recency = 1.0 / (1.0 + days / 30.0);
    let stored_weight = m.weight.max(0.01);
    importance_weight * recency * stored_weight
}

fn categorize(m: &Memory) -> Category {
    let t = m.topic.to_lowercase();
    let s = m.summary.to_lowercase();

    if is_preference_topic(&m.topic) {
        return Category::Identity;
    }
    if t.contains("decision") || s.contains("decided ") || s.contains("chose ") {
        return Category::Decision;
    }
    if t.contains("constraint")
        || t.contains("rule")
        || t.contains("convention")
        || s.starts_with("always ")
        || s.starts_with("never ")
        || s.starts_with("must ")
    {
        return Category::Constraint;
    }
    if t.contains("error") || t.contains("bug") || s.starts_with("fixed ") {
        return Category::Error;
    }
    if t.contains("milestone") || t.contains("release") || s.contains("shipped") {
        return Category::Milestone;
    }
    Category::Context
}

/// Greedy selection that stops once the accumulated character budget is exceeded.
/// Always includes at least one memory if any candidate exists.
/// Counts user-visible characters (not bytes) so multibyte summaries aren't
/// over-penalized.
///
/// **Smart truncation**: oversized memories are trimmed to a per-memory
/// cap (with `[…]` suffix) instead of being included verbatim or
/// blocking subsequent memories from fitting. Without this, a single
/// 5KB memory at the front of the candidate list either:
///   - filled the entire budget and starved the rest (small budgets), or
///   - was emitted verbatim (large budgets), bloating the wake-up pack.
fn truncate_by_budget(candidates: Vec<ScoredMemory>, max_chars: usize) -> Vec<ScoredMemory> {
    // Per-memory cap: a single memory shouldn't consume more than half
    // the budget. Below 200 chars we keep the cap at 200 so very small
    // budgets still admit a usable head + ellipsis.
    let per_memory_cap = (max_chars / 2).max(200);

    let mut total = 0usize;
    let mut out = Vec::new();
    for mut c in candidates {
        let summary_chars = c.memory.summary.chars().count();
        if summary_chars > per_memory_cap {
            // Truncate at a UTF-8 char boundary. We deliberately don't
            // try to break on word/sentence boundary — the head of the
            // summary is what carries signal; the tail is usually
            // elaboration. Add a `[…]` marker so the user knows.
            let mut truncated: String = c.memory.summary.chars().take(per_memory_cap).collect();
            truncated.push_str(" […]");
            c.memory.summary = truncated;
        }
        // Account for bullet prefix + newline.
        let line_len = c.memory.summary.chars().count().saturating_add(4);
        if !out.is_empty() && total.saturating_add(line_len) > max_chars {
            break;
        }
        total = total.saturating_add(line_len);
        out.push(c);
    }
    out
}

/// Flatten a summary into a single-line safe string: trim surrounding space,
/// replace newlines with spaces so an embedded heading can't break section
/// structure in the rendered Markdown.
fn sanitize_summary(summary: &str) -> String {
    let flattened: String = summary
        .trim()
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    // Collapse runs of spaces that result from the flattening.
    let mut out = String::with_capacity(flattened.len());
    let mut prev_space = false;
    for c in flattened.chars() {
        let is_space = c == ' ';
        if is_space && prev_space {
            continue;
        }
        out.push(c);
        prev_space = is_space;
    }
    out
}

/// Placeholder header written by `build_wake_up*` when no critical/high
/// memories match the options. Exposed as a constant so callers (notably
/// the SessionStart hook) can detect the empty case without coupling to
/// the exact wording of the body.
pub const EMPTY_PACK_HEADER: &str = "# ICM Wake-up (empty)";

fn render(selected: &[ScoredMemory], opts: &WakeUpOptions<'_>) -> String {
    if selected.is_empty() {
        return format!(
            "{EMPTY_PACK_HEADER}\n\n(no critical memories yet — use `icm store` to seed)\n"
        );
    }

    // Group by category, preserving per-group score ordering.
    let mut out = String::new();
    let header = match opts.project {
        Some(p) if !p.is_empty() => format!("# ICM Wake-up (project: {p})"),
        _ => String::from("# ICM Wake-up"),
    };

    // Render body without prepending a token-count header — that count
    // changes whenever the body length changes, which destroys the
    // Anthropic prompt-cache prefix match for SessionStart wake-up
    // injection. Adding/removing one memory used to drop prefix preservation
    // from 100% to ~4%. The header now stays byte-stable across runs.
    let body = render_body(selected, opts.format);

    match opts.format {
        WakeUpFormat::Markdown => {
            out.push_str(&format!("{header}\n\n"));
            out.push_str(&body);
        }
        WakeUpFormat::Plain => {
            out.push_str(&body);
        }
    }

    out
}

fn render_body(selected: &[ScoredMemory], format: WakeUpFormat) -> String {
    let mut out = String::new();
    for cat in Category::all_ordered() {
        let group: Vec<&ScoredMemory> = selected
            .iter()
            .filter(|s| s.category == cat)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        // Sort group by original score (already in selected order, re-sort for safety).
        // id tiebreak: see candidates.sort_by above — same determinism rationale.
        let mut group = group;
        group.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.memory.id.cmp(&b.memory.id))
        });

        match format {
            WakeUpFormat::Markdown => {
                out.push_str(&format!("## {}\n", cat.label()));
                for s in &group {
                    out.push_str(&format!("- {}\n", sanitize_summary(&s.memory.summary)));
                }
                out.push('\n');
            }
            WakeUpFormat::Plain => {
                out.push_str(&format!("[{}]\n", cat.label()));
                for s in &group {
                    out.push_str(&format!("- {}\n", sanitize_summary(&s.memory.summary)));
                }
                out.push('\n');
            }
        }
    }
    // Collapse trailing blank lines to a single newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(topic: &str, summary: &str, imp: Importance) -> Memory {
        Memory::new(topic.to_string(), summary.to_string(), imp)
    }

    #[test]
    fn filters_out_low_and_medium_non_preferences() {
        let memories = vec![
            mem(
                "decisions-icm",
                "Use SQLite with FTS5",
                Importance::Critical,
            ),
            mem("other", "random medium fact", Importance::Medium),
            mem("other", "random low fact", Importance::Low),
        ];
        let pack = build_wake_up_from_memories(memories, &WakeUpOptions::default());
        assert!(pack.contains("SQLite"));
        assert!(!pack.contains("random medium"));
        assert!(!pack.contains("random low"));
    }

    #[test]
    fn includes_preferences_even_at_medium_importance() {
        let memories = vec![mem(
            "preferences",
            "User prefers French",
            Importance::Medium,
        )];
        let pack = build_wake_up_from_memories(memories, &WakeUpOptions::default());
        assert!(pack.contains("French"));
        assert!(pack.contains("Identity"));
    }

    #[test]
    fn respects_project_filter() {
        let memories = vec![
            mem("decisions-icm", "ICM: SQLite backend", Importance::Critical),
            mem(
                "decisions-other",
                "OTHER: Postgres backend",
                Importance::Critical,
            ),
        ];
        let opts = WakeUpOptions {
            project: Some("icm"),
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        assert!(pack.contains("ICM: SQLite"));
        assert!(!pack.contains("OTHER: Postgres"));
    }

    #[test]
    fn preferences_are_global_under_project_filter() {
        let memories = vec![
            mem(
                "decisions-other",
                "OTHER: Postgres backend",
                Importance::Critical,
            ),
            mem("preferences", "User prefers French", Importance::Medium),
        ];
        let opts = WakeUpOptions {
            project: Some("icm"),
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        assert!(
            pack.contains("French"),
            "preferences should bypass project filter"
        );
        assert!(!pack.contains("Postgres"));
    }

    #[test]
    fn respects_token_budget_approximately() {
        // 20 critical memories with long summaries.
        let memories: Vec<Memory> = (0..20)
            .map(|i| {
                mem(
                    "decisions-icm",
                    &format!("Long decision number {i} ").repeat(20),
                    Importance::Critical,
                )
            })
            .collect();
        let opts = WakeUpOptions {
            project: Some("icm"),
            max_tokens: 50, // ~200 chars
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        // Should drop most entries — total body well under 1000 chars.
        assert!(
            pack.len() < 1500,
            "pack length {} should be bounded",
            pack.len()
        );
    }

    #[test]
    fn always_includes_at_least_one_entry_if_possible() {
        // One massive entry far exceeding the budget.
        let memories = vec![mem(
            "decisions-icm",
            &"x".repeat(5000),
            Importance::Critical,
        )];
        let opts = WakeUpOptions {
            max_tokens: 10,
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        assert!(pack.contains("xxxx"), "at least one entry must survive");
    }

    #[test]
    fn empty_store_returns_placeholder() {
        let pack = build_wake_up_from_memories(vec![], &WakeUpOptions::default());
        assert!(pack.contains("no critical memories"));
    }

    #[test]
    fn markdown_format_uses_section_headers() {
        let memories = vec![
            mem("decisions-icm", "Use SQLite", Importance::Critical),
            mem("preferences", "User prefers French", Importance::High),
        ];
        let pack = build_wake_up_from_memories(memories, &WakeUpOptions::default());
        assert!(pack.contains("## Identity"));
        assert!(pack.contains("## Critical decisions"));
        assert!(pack.starts_with("# ICM Wake-up"));
    }

    #[test]
    fn plain_format_uses_bracket_headers() {
        let memories = vec![mem("decisions-icm", "Use SQLite", Importance::Critical)];
        let opts = WakeUpOptions {
            format: WakeUpFormat::Plain,
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        assert!(pack.contains("[Critical decisions]"));
        assert!(!pack.contains("## Critical"));
    }

    #[test]
    fn critical_outranks_high_when_same_age() {
        let memories = vec![
            mem("decisions-a", "ALPHA high fact", Importance::High),
            mem("decisions-b", "BETA critical fact", Importance::Critical),
        ];
        let pack = build_wake_up_from_memories(memories, &WakeUpOptions::default());
        let alpha_pos = pack.find("ALPHA").unwrap();
        let beta_pos = pack.find("BETA").unwrap();
        assert!(
            beta_pos < alpha_pos,
            "critical should be rendered before high"
        );
    }

    #[test]
    fn categorize_detects_errors() {
        let m = mem("errors-resolved", "Fixed cache init", Importance::High);
        assert_eq!(categorize(&m), Category::Error);
    }

    #[test]
    fn categorize_detects_decisions_from_topic() {
        let m = mem("decisions-icm", "Use FTS5", Importance::Critical);
        assert_eq!(categorize(&m), Category::Decision);
    }

    #[test]
    fn is_preference_topic_matches_variants() {
        assert!(is_preference_topic("preferences"));
        assert!(is_preference_topic("preferences-icm"));
        assert!(is_preference_topic("user-profile"));
        assert!(!is_preference_topic("decisions-icm"));
    }

    #[test]
    fn project_matches_segment_aware() {
        // Positive: exact segment across various separators
        assert!(project_matches("decisions-icm", Some("icm")));
        assert!(project_matches("context-icm-core", Some("icm")));
        assert!(project_matches("icm.decisions", Some("icm")));
        assert!(project_matches("project:icm/notes", Some("icm")));
        assert!(project_matches("icm_errors", Some("icm")));

        // Positive: multi-segment project filter where all segments appear
        assert!(project_matches("decisions-icm-core", Some("icm-core")));
        assert!(project_matches("context.icm.core", Some("icm.core")));

        // Negative: unrelated
        assert!(!project_matches("decisions-ramiga", Some("icm")));

        // Negative: substring that would have matched under a naive
        // contains() impl but is not a real segment.
        assert!(
            !project_matches("icmp-notes", Some("icm")),
            "icmp should not match icm"
        );
        assert!(
            !project_matches("picm-rules", Some("icm")),
            "picm should not match icm"
        );

        // Negative: multi-segment project with one missing segment
        assert!(
            !project_matches("decisions-icm", Some("icm-core")),
            "icm-core should not match plain decisions-icm"
        );

        // None filter → pass all
        assert!(project_matches("anything", None));
        // Empty filter → pass all
        assert!(project_matches("anything", Some("")));
    }

    #[test]
    fn include_preferences_false_drops_preference_memories() {
        let memories = vec![
            mem("decisions-icm", "Critical decision", Importance::Critical),
            mem("preferences", "User prefers French", Importance::Medium),
        ];
        let opts = WakeUpOptions {
            include_preferences: false,
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        assert!(pack.contains("Critical decision"));
        assert!(!pack.contains("French"), "preferences should be excluded");
        assert!(!pack.contains("## Identity"));
    }

    #[test]
    fn sanitize_summary_flattens_newlines_and_collapses_spaces() {
        let input = "  line one\nline two\r\n## injected header  ";
        let out = sanitize_summary(input);
        assert_eq!(out, "line one line two ## injected header");
    }

    #[test]
    fn renders_without_breaking_on_multiline_summary() {
        let memories = vec![mem(
            "decisions-icm",
            "First line\n## Fake section header\nSecond line",
            Importance::Critical,
        )];
        let pack = build_wake_up_from_memories(memories, &WakeUpOptions::default());
        // There should be exactly ONE `## ` section header (Critical decisions),
        // never two from the injected fake header.
        let header_count = pack.matches("\n## ").count();
        assert_eq!(
            header_count, 1,
            "injected markdown header leaked into render: {pack}"
        );
    }

    #[test]
    fn respects_token_budget_with_multibyte_chars() {
        // 20 long French summaries with accents (multibyte).
        let memories: Vec<Memory> = (0..20)
            .map(|i| {
                mem(
                    "decisions-icm",
                    &format!("Décision numéro {i} très très importante ").repeat(10),
                    Importance::Critical,
                )
            })
            .collect();
        let opts = WakeUpOptions {
            project: Some("icm"),
            max_tokens: 40,
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        // Budget is 40 tok ≈ 160 chars; with the header and section label
        // we expect the body to be bounded under 600 chars.
        assert!(
            pack.chars().count() < 600,
            "char count {} should be bounded",
            pack.chars().count()
        );
    }

    #[test]
    fn compute_score_uses_last_accessed_when_more_recent() {
        // Create memory "long ago" then update last_accessed to now.
        let mut m = mem("decisions-icm", "Old decision", Importance::Critical);
        let long_ago = Utc::now() - chrono::Duration::days(180);
        m.created_at = long_ago;
        m.last_accessed = Utc::now();

        let fresh_score = compute_score(&m, Utc::now());

        // Same memory, never re-accessed.
        let mut stale = mem("decisions-icm", "Old decision", Importance::Critical);
        stale.created_at = long_ago;
        stale.last_accessed = long_ago;
        let stale_score = compute_score(&stale, Utc::now());

        assert!(
            fresh_score > stale_score,
            "recently-accessed memory should outscore stale peer ({fresh_score} vs {stale_score})"
        );
    }

    #[test]
    fn plain_format_omits_header_and_token_count() {
        let memories = vec![mem("decisions-icm", "Use SQLite", Importance::Critical)];
        let opts = WakeUpOptions {
            format: WakeUpFormat::Plain,
            project: Some("icm"),
            ..Default::default()
        };
        let pack = build_wake_up_from_memories(memories, &opts);
        assert!(!pack.starts_with("# ICM Wake-up"));
        assert!(
            !pack.contains("~"),
            "plain format should not have token count"
        );
        assert!(pack.contains("[Critical decisions]"));
    }
}
