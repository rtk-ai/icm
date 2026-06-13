//! Always-on bounded **context snapshot** for SessionStart injection.
//!
//! Unlike [`crate::wake_up`], which mixes critical decisions, errors and
//! milestones into a semantic-ish pack scoped to the project, this module
//! returns the **deterministic baseline**: identity + durable preferences
//! and (optionally) the project-context bullets. It is meant to be
//! injected at SessionStart **separate from semantic recall** so the
//! agent has its baseline regardless of the user's first prompt.
//!
//! Tracking issue: rtk-ai/icm#271 (peer parity with Hermes Agent
//! MEMORY.md/USER.md, OpenClaw layers 1-3, memory-os workspace files).
//!
//! Hermes pattern: instead of silently dropping memories once the
//! budget is exhausted, surface `over_budget` + `dropped` so the caller
//! (CLI or hook) can emit a `consolidate` hint to the user.

use serde::Serialize;

use crate::error::IcmResult;
use crate::memory::{Importance, Memory};
use crate::store::MemoryStore;
use crate::wake_up::is_preference_topic;

/// Output format for the rendered snapshot.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFormat {
    /// Markdown with section headers (default).
    #[default]
    Markdown,
    /// Plain text bullet list, no headers.
    Plain,
}

/// Options for building a context snapshot.
#[derive(Debug, Clone)]
pub struct ContextSnapshotOptions<'a> {
    /// Optional project name (also pulls `contexte-<project>` and
    /// `context-<project>` topics).
    pub project: Option<&'a str>,
    /// Approximate token budget (1 token ≈ 4 chars).
    pub max_tokens: usize,
    /// Output format for the rendered string.
    pub format: SnapshotFormat,
}

impl Default for ContextSnapshotOptions<'_> {
    fn default() -> Self {
        Self {
            project: None,
            max_tokens: 1200,
            format: SnapshotFormat::Markdown,
        }
    }
}

/// One section of the snapshot (e.g. "Identity & preferences").
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotSection {
    pub title: String,
    pub lines: Vec<String>,
}

/// The structured snapshot returned by [`build_context_snapshot`].
///
/// Callers that just want the rendered Markdown can use [`Self::render`].
/// Callers that need to react to the budget (e.g. emit a consolidate
/// hint) inspect [`Self::over_budget`] and [`Self::dropped`].
#[derive(Debug, Clone, Serialize)]
pub struct ContextSnapshot {
    pub sections: Vec<SnapshotSection>,
    /// Total characters in the rendered body (excluding header).
    pub total_chars: usize,
    /// Budget in characters (max_tokens * 4).
    pub max_chars: usize,
    /// True when the selected entries occupy >= 80% of the budget AND at
    /// least one entry was dropped. Caller should surface a consolidate
    /// hint (Hermes pattern: never silently drop without warning).
    pub over_budget: bool,
    /// Number of eligible memories that did not fit the budget.
    pub dropped: usize,
}

impl ContextSnapshot {
    /// Render the snapshot to a string in the requested format. When
    /// [`Self::over_budget`] is true, a `> consolidate ...` hint line
    /// is appended so SessionStart-injected agents see the warning
    /// in-band.
    #[must_use]
    pub fn render(&self, format: SnapshotFormat) -> String {
        render(self, format)
    }

    /// True when no section has any line. Lets callers skip injection
    /// entirely instead of emitting an empty header.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(|s| s.lines.is_empty())
    }
}

/// Build the snapshot by reading all memories from the store. See
/// [`build_context_snapshot_from_memories`] for the pure variant used by
/// tests and benchmarks.
pub fn build_context_snapshot<S: MemoryStore + ?Sized>(
    store: &S,
    opts: &ContextSnapshotOptions<'_>,
) -> IcmResult<ContextSnapshot> {
    let all = store.list_all()?;
    Ok(build_context_snapshot_from_memories(all, opts))
}

/// Pure variant: build a snapshot from an in-memory list. Useful for
/// unit tests and benchmarks where touching the store is overhead.
#[must_use]
pub fn build_context_snapshot_from_memories(
    memories: Vec<Memory>,
    opts: &ContextSnapshotOptions<'_>,
) -> ContextSnapshot {
    let max_chars = opts.max_tokens.saturating_mul(4);

    let (mut prefs, mut ctx): (Vec<Memory>, Vec<Memory>) = memories
        .into_iter()
        .filter(|m| is_relevant(m, opts.project))
        .partition(|m| is_preference_topic(&m.topic));

    let order_key = |m: &Memory| {
        (
            importance_rank(&m.importance),
            // Ties broken by id (ULID-monotonic) so the snapshot stays
            // byte-stable across runs with equal-importance memories.
            // Prompt-cache prefix match depends on this.
            std::cmp::Reverse(m.id.clone()),
        )
    };
    prefs.sort_by_key(order_key);
    ctx.sort_by_key(order_key);

    let total_eligible = prefs.len() + ctx.len();

    // Greedy fill: identity first (it's the baseline), then project context.
    let (identity_lines, identity_used) = fill_section(&prefs, max_chars, 0);
    let (context_lines, context_used) = fill_section(&ctx, max_chars, identity_used);

    let used = identity_used + context_used;
    let kept = identity_lines.len() + context_lines.len();
    let dropped = total_eligible.saturating_sub(kept);

    let mut sections = Vec::new();
    if !identity_lines.is_empty() {
        sections.push(SnapshotSection {
            title: "Identity & preferences".to_string(),
            lines: identity_lines,
        });
    }
    if !context_lines.is_empty() {
        sections.push(SnapshotSection {
            title: "Project context".to_string(),
            lines: context_lines,
        });
    }

    let over_budget = dropped > 0 && used.saturating_mul(10) >= max_chars.saturating_mul(8);

    ContextSnapshot {
        sections,
        total_chars: used,
        max_chars,
        over_budget,
        dropped,
    }
}

/// Eligible iff the memory is a preference/identity OR a project-context
/// topic for the active project filter.
fn is_relevant(m: &Memory, project: Option<&str>) -> bool {
    if is_preference_topic(&m.topic) {
        // Identity/preferences are always eligible (cross-project baseline).
        return true;
    }
    let Some(proj) = project else {
        // No project filter — only the identity layer survives. Project
        // context without a project is undefined.
        return false;
    };
    if proj.is_empty() {
        return false;
    }
    let lower = m.topic.to_lowercase();
    let proj_lower = proj.to_lowercase();
    // Match the two French/English spellings we use in the codebase plus
    // the literal `<project>` form for users who store under bare project
    // names.
    lower == proj_lower
        || lower == format!("contexte-{proj_lower}")
        || lower == format!("context-{proj_lower}")
        || lower.starts_with(&format!("contexte-{proj_lower}-"))
        || lower.starts_with(&format!("context-{proj_lower}-"))
}

fn importance_rank(imp: &Importance) -> u8 {
    match imp {
        Importance::Critical => 0,
        Importance::High => 1,
        Importance::Medium => 2,
        Importance::Low => 3,
    }
}

/// Per-memory cap so a single oversized summary cannot starve the
/// snapshot. Mirrors `wake_up::truncate_by_budget`.
fn per_memory_cap(max_chars: usize) -> usize {
    (max_chars / 2).max(200)
}

/// Greedy budget fill. Returns the rendered bullet lines and the number
/// of characters consumed (line bytes + leading "- " + trailing newline
/// accounted for so the caller's running total matches what `render`
/// will emit).
///
/// The "always include at least one entry" exception only triggers when
/// `already_used == 0` — i.e. the very first section. Once another
/// section has consumed some of the global budget, subsequent sections
/// strictly respect the cap to keep `total_chars <= max_chars`.
fn fill_section(
    candidates: &[Memory],
    max_chars: usize,
    already_used: usize,
) -> (Vec<String>, usize) {
    let cap = per_memory_cap(max_chars);
    let mut out = Vec::new();
    let mut used = 0usize;
    let first_section = already_used == 0;
    for m in candidates {
        let mut summary = sanitize(&m.summary);
        if summary.chars().count() > cap {
            let head: String = summary.chars().take(cap).collect();
            summary = format!("{head} […]");
        }
        // 4 = `- ` prefix (2) + newline (1) + 1 char headroom for the
        // header line. Caller's total budget is `max_chars`.
        let line_len = summary.chars().count().saturating_add(4);
        let projected = already_used.saturating_add(used).saturating_add(line_len);
        // Always include the first bullet of the first section so an
        // oversized lone memory still surfaces. For subsequent sections
        // the budget is hard.
        let force_take = first_section && out.is_empty();
        if !force_take && projected > max_chars {
            break;
        }
        used = used.saturating_add(line_len);
        out.push(summary);
    }
    (out, used)
}

fn sanitize(s: &str) -> String {
    let flattened: String = s
        .trim()
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
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

/// Header constant matching the wake_up convention. Stable across runs
/// so the SessionStart prompt-cache prefix doesn't break.
pub const SNAPSHOT_HEADER: &str = "# ICM Context Snapshot";

fn render(snap: &ContextSnapshot, format: SnapshotFormat) -> String {
    if snap.is_empty() {
        // No sections → don't emit a dangling header. Callers check
        // `Snapshot::is_empty()` to decide whether to skip the block.
        return String::new();
    }
    let mut out = String::new();
    if matches!(format, SnapshotFormat::Markdown) {
        out.push_str(SNAPSHOT_HEADER);
        out.push_str("\n\n");
    }
    for sec in &snap.sections {
        match format {
            SnapshotFormat::Markdown => {
                out.push_str(&format!("## {}\n", sec.title));
                for line in &sec.lines {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
            SnapshotFormat::Plain => {
                out.push_str(&format!("[{}]\n", sec.title));
                for line in &sec.lines {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
        }
    }
    if snap.over_budget {
        // In-band hint: visible to the human and the agent both. The
        // `>` Markdown blockquote keeps it visually distinct from the
        // bullets above.
        out.push_str(&format!(
            "> snapshot at {}/{} chars with {} entries dropped — \
             run `icm consolidate --topic preferences` to keep the baseline lean.\n",
            snap.total_chars, snap.max_chars, snap.dropped,
        ));
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
    fn includes_preferences_without_project_filter() {
        let memories = vec![
            mem("preferences", "User prefers French", Importance::High),
            mem("decisions-icm", "Use SQLite", Importance::Critical),
        ];
        let snap = build_context_snapshot_from_memories(memories, &Default::default());
        let body = snap.render(SnapshotFormat::Markdown);
        assert!(body.contains("French"));
        // Decisions are NOT in the snapshot — they go through wake_up.
        assert!(!body.contains("SQLite"));
    }

    #[test]
    fn includes_project_context_only_with_filter() {
        let memories = vec![
            mem("contexte-icm", "ICM uses Turso", Importance::High),
            mem("contexte-other", "Other uses Postgres", Importance::High),
        ];
        let opts_no_filter = ContextSnapshotOptions::default();
        let snap_unfiltered =
            build_context_snapshot_from_memories(memories.clone(), &opts_no_filter);
        assert!(snap_unfiltered.is_empty(), "no project = no context block");

        let opts = ContextSnapshotOptions {
            project: Some("icm"),
            ..Default::default()
        };
        let snap = build_context_snapshot_from_memories(memories, &opts);
        let body = snap.render(SnapshotFormat::Markdown);
        assert!(body.contains("Turso"));
        assert!(!body.contains("Postgres"));
    }

    #[test]
    fn english_and_french_context_topics_both_accepted() {
        let memories = vec![
            mem("context-icm", "english spelling", Importance::High),
            mem("contexte-icm", "french spelling", Importance::High),
            mem("context-icm-store", "sub-namespace", Importance::High),
        ];
        let opts = ContextSnapshotOptions {
            project: Some("icm"),
            ..Default::default()
        };
        let snap = build_context_snapshot_from_memories(memories, &opts);
        let body = snap.render(SnapshotFormat::Markdown);
        assert!(body.contains("english"));
        assert!(body.contains("french"));
        assert!(body.contains("sub-namespace"));
    }

    #[test]
    fn over_budget_flag_set_when_eighty_percent_full_and_drops() {
        // 30 preferences of 60 chars each → ~1800 chars, way past a
        // 400-char budget.
        let memories: Vec<Memory> = (0..30)
            .map(|i| {
                mem(
                    "preferences",
                    &format!("preference number {i:02} with some words"),
                    Importance::High,
                )
            })
            .collect();
        let opts = ContextSnapshotOptions {
            max_tokens: 100, // ~400 chars
            ..Default::default()
        };
        let snap = build_context_snapshot_from_memories(memories, &opts);
        assert!(snap.dropped > 0, "must drop something");
        assert!(snap.over_budget, "should flag the over-budget condition");
        let body = snap.render(SnapshotFormat::Markdown);
        assert!(body.contains("consolidate"), "must emit hint: {body}");
    }

    #[test]
    fn over_budget_flag_clear_when_everything_fits() {
        let memories = vec![mem("preferences", "small", Importance::High)];
        let snap = build_context_snapshot_from_memories(memories, &Default::default());
        assert!(!snap.over_budget);
        assert_eq!(snap.dropped, 0);
        let body = snap.render(SnapshotFormat::Markdown);
        assert!(!body.contains("consolidate"));
    }

    #[test]
    fn empty_input_is_empty_snapshot() {
        let snap = build_context_snapshot_from_memories(vec![], &ContextSnapshotOptions::default());
        assert!(snap.is_empty());
        assert_eq!(snap.dropped, 0);
        assert!(!snap.over_budget);
    }

    #[test]
    fn deterministic_ordering_on_equal_importance() {
        let mut a = mem("preferences", "alpha", Importance::High);
        a.id = "01HZZZ0000000000000000000A".into();
        let mut b = mem("preferences", "beta", Importance::High);
        b.id = "01HZZZ0000000000000000000B".into();
        let snap1 = build_context_snapshot_from_memories(
            vec![a.clone(), b.clone()],
            &ContextSnapshotOptions::default(),
        );
        let snap2 =
            build_context_snapshot_from_memories(vec![b, a], &ContextSnapshotOptions::default());
        assert_eq!(
            snap1.render(SnapshotFormat::Markdown),
            snap2.render(SnapshotFormat::Markdown),
            "snapshot must be input-order-independent for cache stability",
        );
    }

    #[test]
    fn critical_outranks_high_in_section() {
        let memories = vec![
            mem("preferences", "ALPHA high", Importance::High),
            mem("preferences", "BETA critical", Importance::Critical),
        ];
        let snap =
            build_context_snapshot_from_memories(memories, &ContextSnapshotOptions::default());
        let body = snap.render(SnapshotFormat::Markdown);
        let alpha_pos = body.find("ALPHA").unwrap();
        let beta_pos = body.find("BETA").unwrap();
        assert!(beta_pos < alpha_pos);
    }

    #[test]
    fn oversized_summary_is_truncated_with_marker() {
        let big = mem("preferences", &"x".repeat(5000), Importance::Critical);
        let opts = ContextSnapshotOptions {
            max_tokens: 100,
            ..Default::default()
        };
        let snap = build_context_snapshot_from_memories(vec![big], &opts);
        let body = snap.render(SnapshotFormat::Markdown);
        assert!(body.contains("[…]"), "must mark truncation: {body}");
    }

    #[test]
    fn plain_format_omits_main_header_but_keeps_section_brackets() {
        let memories = vec![mem("preferences", "User prefers French", Importance::High)];
        let snap =
            build_context_snapshot_from_memories(memories, &ContextSnapshotOptions::default());
        let plain = snap.render(SnapshotFormat::Plain);
        assert!(!plain.starts_with("# ICM"));
        assert!(plain.contains("[Identity & preferences]"));
        assert!(plain.contains("French"));
    }

    /// Issue #271 perf target: snapshot build must stay <50ms even with
    /// 10k memories (the always-on injection budget). Run with
    /// `cargo test -p icm-core --release perf_snapshot` for honest
    /// release timings; the unit-test invariant simply asserts the
    /// debug-mode budget so we get a regression bell.
    #[test]
    fn perf_snapshot_10k_memories_under_budget() {
        let mut memories: Vec<Memory> = Vec::with_capacity(10_000);
        for i in 0..10_000 {
            // Mix of preferences, project context, and noise — realistic.
            let (topic, imp) = match i % 3 {
                0 => ("preferences", Importance::High),
                1 => ("context-icm", Importance::High),
                _ => ("decisions-other", Importance::Critical),
            };
            memories.push(mem(
                topic,
                &format!("entry {i} with some descriptive text"),
                imp,
            ));
        }
        let opts = ContextSnapshotOptions {
            project: Some("icm"),
            max_tokens: 1200,
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let snap = build_context_snapshot_from_memories(memories, &opts);
        let elapsed = start.elapsed();
        // Debug mode is ~3-5x slower than release; pick 250ms so we
        // catch true regressions without flaking on shared CI runners.
        assert!(
            elapsed.as_millis() < 250,
            "10k snapshot took {}ms (debug-mode budget 250ms)",
            elapsed.as_millis()
        );
        // Sanity check: result is bounded and identity section landed.
        assert!(snap.total_chars <= snap.max_chars);
        assert!(snap
            .sections
            .iter()
            .any(|s| s.title.starts_with("Identity")));
    }

    #[test]
    fn multiline_summary_is_flattened() {
        let m = mem(
            "preferences",
            "First line\n## Fake header\nSecond line",
            Importance::High,
        );
        let snap =
            build_context_snapshot_from_memories(vec![m], &ContextSnapshotOptions::default());
        let body = snap.render(SnapshotFormat::Markdown);
        // Exactly one `## ` from our own section header — the embedded
        // `## Fake header` must have been flattened.
        let header_count = body.matches("\n## ").count();
        assert_eq!(header_count, 1, "embedded header leaked: {body}");
    }
}
