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
            max_tokens: 200,
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

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Token budget: 1 token ≈ 4 characters (rough approximation).
    let max_chars = opts.max_tokens.saturating_mul(4);
    let selected = truncate_by_budget(candidates, max_chars);

    render(&selected, opts)
}

/// Return true when a topic looks like it holds identity/preference data.
fn is_preference_topic(topic: &str) -> bool {
    let lower = topic.to_lowercase();
    lower == "preferences"
        || lower == "identity"
        || lower.starts_with("preferences-")
        || lower.starts_with("preferences.")
        || lower.contains("preference")
        || lower.starts_with("user-")
        || lower.starts_with("user.")
}

/// Return true if a topic matches the project filter (substring match),
/// or if the filter is absent.
fn project_matches(topic: &str, project: Option<&str>) -> bool {
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
    let lower_topic = topic.to_lowercase();
    let lower_proj = proj.to_lowercase();
    lower_topic.contains(&lower_proj)
}

fn compute_score(m: &Memory, now: DateTime<Utc>) -> f32 {
    let importance_weight = match m.importance {
        Importance::Critical => 10.0,
        Importance::High => 5.0,
        Importance::Medium => 2.0,
        Importance::Low => 0.5,
    };
    let days = (now - m.created_at).num_days().max(0) as f32;
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
fn truncate_by_budget(candidates: Vec<ScoredMemory>, max_chars: usize) -> Vec<ScoredMemory> {
    let mut total = 0usize;
    let mut out = Vec::new();
    for c in candidates {
        // Account for bullet prefix + newline.
        let line_len = c.memory.summary.len().saturating_add(4);
        if !out.is_empty() && total.saturating_add(line_len) > max_chars {
            break;
        }
        total = total.saturating_add(line_len);
        out.push(c);
    }
    out
}

fn approx_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn render(selected: &[ScoredMemory], opts: &WakeUpOptions<'_>) -> String {
    if selected.is_empty() {
        return String::from(
            "# ICM Wake-up\n\n(no critical memories yet — use `icm store` to seed)\n",
        );
    }

    // Group by category, preserving per-group score ordering.
    let mut out = String::new();
    let header = match opts.project {
        Some(p) if !p.is_empty() => format!("# ICM Wake-up (project: {p})"),
        _ => String::from("# ICM Wake-up"),
    };

    // Pre-compute body to get token count, then prepend header with count.
    let body = render_body(selected, opts.format);
    let tok = approx_tokens(&body);

    match opts.format {
        WakeUpFormat::Markdown => {
            out.push_str(&format!("{header} · ~{tok} tok\n\n"));
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
        let mut group = group;
        group.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        match format {
            WakeUpFormat::Markdown => {
                out.push_str(&format!("## {}\n", cat.label()));
                for s in &group {
                    out.push_str(&format!("- {}\n", s.memory.summary.trim()));
                }
                out.push('\n');
            }
            WakeUpFormat::Plain => {
                out.push_str(&format!("[{}]\n", cat.label()));
                for s in &group {
                    out.push_str(&format!("- {}\n", s.memory.summary.trim()));
                }
                out.push('\n');
            }
        }
    }
    // Sort the categories output (we iterate in `all_ordered` already).
    // Ensure we don't end with multiple blank lines.
    while out.ends_with("\n\n\n") {
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
    fn project_matches_substring() {
        assert!(project_matches("decisions-icm", Some("icm")));
        assert!(project_matches("context-icm-core", Some("icm")));
        assert!(!project_matches("decisions-ramiga", Some("icm")));
        assert!(project_matches("anything", None));
    }
}
