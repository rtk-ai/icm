//! The v2 recall pipeline: one entry point shared by every surface (HTTP,
//! MCP, CLI) so a ranking or budget change is made once.
//!
//! [`recall_v2`] embeds the query, ranks deep candidates by rank fusion
//! (`Store::search_ranked`), applies the caller's filters before the cut,
//! cuts by token budget or by count, optionally appends `related_ids`
//! neighbors in the room that is left, and records the access.
//!
//! The budget is spent on what the caller renders, not on the summaries:
//! a surface that knows its output passes a cost function to
//! [`RecallRequest::run_with_cost`]; [`recall_v2`] charges the largest
//! rendering there is, the full JSON record.
//!
//! v2 is the default engine. The legacy path (`search_hybrid` plus
//! per-surface filtering) stays available on request, to roll back or to
//! compare. [`RecallEngine::resolve`] decides which one a request gets.

use std::collections::HashSet;

use chrono::{DateTime, Local, Utc};

use icm_core::temporal::parse_query_window_at;
use icm_core::{
    estimate_tokens, is_preference_topic, keyword_matches, parse_query_window, project_matches,
    select_within_budget, topic_matches, Embedder, IcmError, IcmResult, Memory, MemoryStore,
    RankedHit, RankedQuery, TimeWindow, ITEM_OVERHEAD_TOKENS,
};

use crate::backend::Store;

/// Selects the engine when the request does not name one.
const ENGINE_ENV: &str = "ICM_RECALL_ENGINE";
/// Overrides the number of candidates fetched per arm.
const DEPTH_ENV: &str = "ICM_RECALL_DEPTH";

/// Most results one recall may return, budget or not.
const MAX_LIMIT: usize = 500;
/// Default candidate depth is `limit * 4`, kept inside these bounds.
const MIN_DEPTH: usize = 100;
const MAX_DEPTH: usize = 2000;
/// Lowest depth `$ICM_RECALL_DEPTH` may request: `search_ranked`'s own floor.
const MIN_DEPTH_OVERRIDE: usize = 20;
/// Score multiplier for a memory reached through `related_ids`.
const NEIGHBOR_DISCOUNT: f32 = 0.5;
/// Neighbor ids fetched per free place: some are filtered out or too large.
const NEIGHBOR_FETCH_FACTOR: usize = 4;
/// Shortest query word the substring fallback searches for: shorter ones
/// ("a", "do", "to") are inside almost every text.
const SUBSTRING_MIN_CHARS: usize = 3;
/// Memories the substring fallback fetches before ordering them; the most
/// `search_by_keywords` returns.
const SUBSTRING_POOL: usize = 100;
/// Hits whose access is recorded, best first. A budgeted recall returns
/// hundreds of memories; counting each as "used" would hold back the decay
/// of everything a broad query touches. 20 is the most the legacy MCP
/// recall ever recorded.
const ACCESS_RECORD_MAX: usize = 20;

/// Which recall implementation serves a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallEngine {
    /// `search_hybrid` and the per-surface code around it, unchanged.
    /// Only on request.
    Legacy,
    /// [`recall_v2`], the default.
    V2,
}

impl RecallEngine {
    /// Engine for a request: its explicit `engine` value, else
    /// `$ICM_RECALL_ENGINE`, else `V2`.
    ///
    /// `IcmError::InvalidInput` when the request names an unknown engine,
    /// and when `legacy` is selected together with a budget: the legacy
    /// path cuts by count and would drop the budget without a word. An
    /// unknown `$ICM_RECALL_ENGINE` is logged and ignored, so a typo in
    /// the environment does not fail every recall.
    pub fn resolve(explicit: Option<&str>, has_budget: bool) -> IcmResult<RecallEngine> {
        let env_value = std::env::var(ENGINE_ENV).ok();
        resolve_from(explicit, env_value.as_deref(), has_budget)
    }

    fn parse(value: &str) -> Option<RecallEngine> {
        match value.to_ascii_lowercase().as_str() {
            "legacy" => Some(RecallEngine::Legacy),
            "v2" => Some(RecallEngine::V2),
            _ => None,
        }
    }
}

/// Pure core of [`RecallEngine::resolve`]. A blank value counts as absent.
fn resolve_from(
    explicit: Option<&str>,
    env_value: Option<&str>,
    has_budget: bool,
) -> IcmResult<RecallEngine> {
    let present = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(String::from);

    let chosen = if let Some(v) = present(explicit) {
        let engine = RecallEngine::parse(&v).ok_or_else(|| {
            IcmError::InvalidInput(format!(
                "unknown recall engine {v:?} (engine); expected one of: legacy, v2"
            ))
        })?;
        Some((engine, "`engine`"))
    } else if let Some(v) = present(env_value) {
        let engine = RecallEngine::parse(&v);
        if engine.is_none() {
            tracing::warn!(
                value = %v,
                "unknown ${ENGINE_ENV} ignored; expected one of: legacy, v2"
            );
        }
        engine.map(|e| (e, "$ICM_RECALL_ENGINE"))
    } else {
        None
    };

    match chosen {
        Some((RecallEngine::Legacy, origin)) if has_budget => Err(IcmError::InvalidInput(format!(
            "max_tokens needs the v2 engine, but {origin} selects legacy: \
                 drop one of the two"
        ))),
        Some((engine, _)) => Ok(engine),
        None => Ok(RecallEngine::V2),
    }
}

/// Candidates fetched per arm for an already clamped `limit`. A positive
/// integer in `$ICM_RECALL_DEPTH` replaces the default; anything else is
/// ignored.
fn depth_from(limit: usize, env_value: Option<&str>) -> usize {
    match env_value.and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(d) if d > 0 => d.clamp(MIN_DEPTH_OVERRIDE, MAX_DEPTH),
        _ => limit.saturating_mul(4).clamp(MIN_DEPTH, MAX_DEPTH),
    }
}

/// One recall request, surface-independent.
#[derive(Debug, Clone, Copy)]
pub struct RecallRequest<'a> {
    pub query: &'a str,
    /// Most results returned; clamped to `[1, 500]`.
    pub limit: usize,
    /// Token budget over the rendered hits (estimated, see
    /// `icm_core::budget`). `None` cuts by `limit` alone.
    pub max_tokens: Option<usize>,
    pub topic: Option<&'a str>,
    pub keyword: Option<&'a str>,
    /// `None` or `Some("")` disables the project filter.
    pub project: Option<&'a str>,
    /// Anchor for date expressions in the query, days cut in UTC. `None`
    /// is the wall clock, days cut in the local time zone.
    pub now: Option<DateTime<Utc>>,
    /// Append `related_ids` neighbors of the hits when room is left.
    pub expand_neighbors: bool,
}

impl RecallRequest<'_> {
    /// Run this request, charging each hit against the budget what `cost`
    /// returns for it: the estimated tokens of that memory, with that
    /// score, as the caller is about to render it. As long as `cost` does
    /// not undercount, the rendered hits fit in `max_tokens`.
    ///
    /// Bookkeeping writes (auto-decay, access counts) are best effort:
    /// under write contention they are dropped and the recall still
    /// succeeds. Without an embedder, or when embedding the query fails,
    /// the ranking is lexical only. Backends without a ranked search fall
    /// back to their existing one inside `Store::search_ranked`.
    pub fn run_with_cost(
        &self,
        store: &Store,
        embedder: Option<&dyn Embedder>,
        cost: &dyn Fn(&Memory, f32) -> usize,
    ) -> IcmResult<RecallOutcome> {
        let depth_env = std::env::var(DEPTH_ENV).ok();
        recall_with_depth(store, embedder, self, cost, depth_env.as_deref(), true)
    }

    /// [`Self::run_with_cost`] without its writes: no auto-decay, no access
    /// recorded. For a recall nobody asked for, such as the one a prompt
    /// hook runs on every message: it must not take a write lock, and
    /// what it happens to surface must not count as "used" and stop
    /// decaying.
    pub fn run_read_only(
        &self,
        store: &Store,
        embedder: Option<&dyn Embedder>,
        cost: &dyn Fn(&Memory, f32) -> usize,
    ) -> IcmResult<RecallOutcome> {
        let depth_env = std::env::var(DEPTH_ENV).ok();
        recall_with_depth(store, embedder, self, cost, depth_env.as_deref(), false)
    }
}

/// One returned memory.
#[derive(Debug, Clone)]
pub struct RecallHit {
    pub memory: Memory,
    /// Fused score in (0, 1]. A neighbor gets half its parent's score,
    /// capped at the lowest ranked hit's. 0 for a last-resort substring
    /// match, which has no rank.
    pub score: f32,
    /// What this hit was charged against the budget.
    pub est_tokens: usize,
    /// 1-based rank in each arm; all `None` for a neighbor or a substring
    /// match.
    pub fts_rank: Option<u32>,
    pub vec_rank: Option<u32>,
    pub time_rank: Option<u32>,
}

/// Result of [`recall_v2`].
#[derive(Debug, Clone)]
pub struct RecallOutcome {
    /// Ranked hits, best first, then any neighbors.
    pub hits: Vec<RecallHit>,
    /// Sum of the hits' `est_tokens`. Exceeds the budget only when the
    /// single hit returned is larger than the whole budget.
    pub used_tokens: usize,
    /// Ranked candidates passed over because they did not fit the budget.
    pub skipped_for_budget: usize,
    /// Ranked candidates considered for the cut.
    pub candidates: usize,
    /// Time window read from the query, if any.
    pub window: Option<TimeWindow>,
}

/// Run a recall on the default (v2) engine for a caller that does not say
/// how it renders the hits.
///
/// Each hit is charged as its full JSON record, the largest of the
/// renderings: whatever the caller prints, the budget holds, at the price
/// of returning fewer hits than a compact rendering could afford. A
/// surface that knows its output uses [`RecallRequest::run_with_cost`].
pub fn recall_v2(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    req: &RecallRequest<'_>,
) -> IcmResult<RecallOutcome> {
    req.run_with_cost(store, embedder, &full_record_cost)
}

/// Estimated tokens of a memory as one element of a pretty-printed JSON
/// array (every field but the embedding), plus an allowance for the score
/// a renderer adds.
fn full_record_cost(memory: &Memory, _score: f32) -> usize {
    let mut record = memory.clone();
    record.embedding = None;
    let tokens = match serde_json::to_string_pretty(std::slice::from_ref(&record)) {
        Ok(json) => estimate_tokens(&json),
        // Not reachable for a `Memory`; make the item too large to fit
        // rather than let it through for free.
        Err(_) => usize::MAX,
    };
    tokens.saturating_add(ITEM_OVERHEAD_TOKENS)
}

/// The pipeline with the `$ICM_RECALL_DEPTH` value passed in, so tests do
/// not depend on the process environment. `bookkeeping` enables the
/// best-effort writes (auto-decay before, access record after).
fn recall_with_depth(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    req: &RecallRequest<'_>,
    cost: &dyn Fn(&Memory, f32) -> usize,
    depth_env: Option<&str>,
    bookkeeping: bool,
) -> IcmResult<RecallOutcome> {
    if req.query.trim().is_empty() {
        return Err(IcmError::InvalidInput("query must not be empty".into()));
    }
    if bookkeeping {
        if let Err(e) = store.maybe_auto_decay() {
            tracing::warn!(error = %e, "auto-decay failed during recall");
        }
    }

    let limit = req.limit.clamp(1, MAX_LIMIT);
    let depth = depth_from(limit, depth_env);

    let embedding = embedder.and_then(|e| match e.embed_query(req.query) {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!(error = %err, "query embedding failed, lexical recall only");
            None
        }
    });
    // A caller-supplied anchor says nothing of where the caller is: its
    // days are cut in UTC. Without one, "yesterday" is the user's own.
    let window = match req.now {
        Some(now) => parse_query_window(req.query, now),
        None => parse_query_window_at(req.query, Utc::now(), *Local::now().offset()),
    };

    let keep = |m: &Memory| passes_filters(req, m);
    let query = RankedQuery {
        text: req.query,
        embedding: embedding.as_deref(),
        // Under a budget the budget does the cutting, so ask for every
        // fused candidate.
        limit: if req.max_tokens.is_some() {
            depth
        } else {
            limit
        },
        depth,
        window,
    };
    let mut ranked = store.search_ranked(&query, &keep)?;
    if ranked.is_empty() {
        ranked = substring_matches(store, req.query, limit, &keep);
    }

    let mut outcome = cut(ranked, req.max_tokens, limit, window, cost);
    if req.expand_neighbors {
        append_neighbors(store, &mut outcome, req.max_tokens, limit, &keep, cost);
    }

    if bookkeeping {
        let ids: Vec<&str> = outcome
            .hits
            .iter()
            .take(ACCESS_RECORD_MAX)
            .map(|h| h.memory.id.as_str())
            .collect();
        let _ = store.batch_update_access(&ids);
    }

    Ok(outcome)
}

/// Last resort when no arm matched anything: memories that contain a query
/// word as a substring of their topic, summary or keywords.
///
/// The lexical index matches whole words and does not stem, so without a
/// vector arm "deploy" finds nothing in a store that only says
/// "deployment". The legacy paths fell back to this same `LIKE` search;
/// here the matches are ordered by how many query words they contain
/// (then by weight, the store's order) and carry a score of 0. A failed
/// search is an empty list, like any other no-match.
fn substring_matches(
    store: &Store,
    query: &str,
    limit: usize,
    keep: &dyn Fn(&Memory) -> bool,
) -> Vec<RankedHit> {
    let mut words: Vec<String> = Vec::new();
    for word in query.split(|c: char| !c.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.chars().count() >= SUBSTRING_MIN_CHARS && !words.contains(&word) {
            words.push(word);
        }
    }
    if words.is_empty() {
        return Vec::new();
    }
    let refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let mut found = match store.search_by_keywords(&refs, SUBSTRING_POOL) {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "substring fallback failed during recall");
            return Vec::new();
        }
    };
    found.retain(|m| keep(m));

    let matched = |m: &Memory| -> usize {
        let text = format!("{} {} {}", m.topic, m.summary, m.keywords.join(" ")).to_lowercase();
        words.iter().filter(|w| text.contains(w.as_str())).count()
    };
    // Stable: equal counts keep the store's weight order.
    found.sort_by_cached_key(|m| std::cmp::Reverse(matched(m)));
    found.truncate(limit);
    found
        .into_iter()
        .map(|memory| RankedHit {
            memory,
            score: 0.0,
            fts_rank: None,
            vec_rank: None,
            time_rank: None,
        })
        .collect()
}

/// Project, topic and keyword predicates of the legacy recall paths:
/// preference topics pass any project filter.
fn passes_filters(req: &RecallRequest<'_>, m: &Memory) -> bool {
    let project_ok = match req.project {
        None | Some("") => true,
        Some(p) => is_preference_topic(&m.topic) || project_matches(&m.topic, Some(p)),
    };
    project_ok
        && req.topic.is_none_or(|t| topic_matches(&m.topic, t))
        && req.keyword.is_none_or(|k| keyword_matches(&m.keywords, k))
}

/// Cut a ranked list by token budget (at most `limit` hits), or by `limit`
/// alone when there is no budget.
fn cut(
    mut ranked: Vec<RankedHit>,
    max_tokens: Option<usize>,
    limit: usize,
    window: Option<TimeWindow>,
    cost: &dyn Fn(&Memory, f32) -> usize,
) -> RecallOutcome {
    let candidates = ranked.len();
    if max_tokens.is_none() {
        ranked.truncate(limit);
    }
    let costs: Vec<usize> = ranked.iter().map(|h| cost(&h.memory, h.score)).collect();

    let (kept, used_tokens, skipped_for_budget) = match max_tokens {
        Some(budget) => {
            let sel = select_within_budget(&costs, budget, limit);
            (sel.kept, sel.used_tokens, sel.skipped)
        }
        None => {
            let used = costs.iter().fold(0usize, |acc, c| acc.saturating_add(*c));
            ((0..ranked.len()).collect(), used, 0)
        }
    };

    let mut slots: Vec<Option<RankedHit>> = ranked.into_iter().map(Some).collect();
    let hits = kept
        .into_iter()
        .filter_map(|i| {
            let hit = slots.get_mut(i)?.take()?;
            Some(RecallHit {
                memory: hit.memory,
                score: hit.score,
                est_tokens: costs.get(i).copied().unwrap_or(0),
                fts_rank: hit.fts_rank,
                vec_rank: hit.vec_rank,
                time_rank: hit.time_rank,
            })
        })
        .collect();

    RecallOutcome {
        hits,
        used_tokens,
        skipped_for_budget,
        candidates,
        window,
    }
}

/// Append one-hop `related_ids` neighbors of the hits, after them.
///
/// A neighbor was not matched by the query, so it never takes the place of
/// a ranked hit: it only fills the places `limit` leaves free and the
/// tokens the budget has left, and its score (half its parent's) is capped
/// at the lowest ranked score. On fused scores, half of the best hit would
/// otherwise outrank a genuine hit found by a single arm.
///
/// Neighbors are fetched by id, so they bypass the filter `search_ranked`
/// applied: `keep` is applied here. A failed fetch leaves the hits as
/// they are.
fn append_neighbors(
    store: &Store,
    outcome: &mut RecallOutcome,
    max_tokens: Option<usize>,
    limit: usize,
    keep: &dyn Fn(&Memory) -> bool,
    cost: &dyn Fn(&Memory, f32) -> usize,
) {
    let Some(floor) = outcome.hits.last().map(|h| h.score) else {
        return;
    };
    let places = limit
        .saturating_sub(outcome.hits.len())
        .min((limit / 3).max(1));
    let mut tokens_left = max_tokens.map(|b| b.saturating_sub(outcome.used_tokens));
    if places == 0 || tokens_left == Some(0) {
        return;
    }

    // Neighbor ids in the rank order of the hit that links to them.
    let wanted_max = places.saturating_mul(NEIGHBOR_FETCH_FACTOR);
    let mut seen: HashSet<&str> = outcome.hits.iter().map(|h| h.memory.id.as_str()).collect();
    let mut wanted: Vec<(String, f32)> = Vec::new();
    'hits: for hit in &outcome.hits {
        for id in &hit.memory.related_ids {
            if wanted.len() >= wanted_max {
                break 'hits;
            }
            if seen.insert(id.as_str()) {
                wanted.push((id.clone(), hit.score));
            }
        }
    }
    if wanted.is_empty() {
        return;
    }

    let ids: Vec<&str> = wanted.iter().map(|(id, _)| id.as_str()).collect();
    let mut fetched = match store.get_many(&ids) {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "neighbor fetch failed during recall");
            return;
        }
    };

    let mut added = 0usize;
    for (id, parent_score) in wanted {
        if added >= places {
            break;
        }
        let Some(memory) = fetched.remove(&id) else {
            continue;
        };
        if !keep(&memory) {
            continue;
        }
        let score = (parent_score * NEIGHBOR_DISCOUNT).min(floor);
        let est_tokens = cost(&memory, score);
        if let Some(left) = tokens_left.as_mut() {
            if est_tokens > *left {
                continue;
            }
            *left -= est_tokens;
        }
        outcome.used_tokens = outcome.used_tokens.saturating_add(est_tokens);
        outcome.hits.push(RecallHit {
            memory,
            score,
            est_tokens,
            fts_rank: None,
            vec_rank: None,
            time_rank: None,
        });
        added += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icm_core::Importance;

    fn mem(topic: &str, summary: &str) -> Memory {
        Memory::new(topic.into(), summary.into(), Importance::Medium)
    }

    fn ranked_hit(summary: &str, score: f32) -> RankedHit {
        RankedHit {
            memory: mem("t", summary),
            score,
            fts_rank: Some(1),
            vec_rank: None,
            time_rank: None,
        }
    }

    fn request(query: &str) -> RecallRequest<'_> {
        RecallRequest {
            query,
            limit: 5,
            max_tokens: None,
            topic: None,
            keyword: None,
            project: None,
            now: None,
            expand_neighbors: false,
        }
    }

    // --- engine resolution -------------------------------------------------

    #[test]
    fn resolve_explicit_wins_over_env() {
        assert_eq!(
            resolve_from(Some("legacy"), Some("v2"), false).unwrap(),
            RecallEngine::Legacy
        );
        assert_eq!(
            resolve_from(Some("v2"), Some("legacy"), false).unwrap(),
            RecallEngine::V2
        );
        // An explicit v2 with a budget is not affected by a legacy
        // environment either.
        assert_eq!(
            resolve_from(Some("v2"), Some("legacy"), true).unwrap(),
            RecallEngine::V2
        );
    }

    #[test]
    fn resolve_env_applies_when_the_request_names_no_engine() {
        assert_eq!(
            resolve_from(None, Some("legacy"), false).unwrap(),
            RecallEngine::Legacy
        );
        assert_eq!(
            resolve_from(None, Some("v2"), false).unwrap(),
            RecallEngine::V2
        );
        assert_eq!(
            resolve_from(None, Some("v2"), true).unwrap(),
            RecallEngine::V2
        );
    }

    #[test]
    fn resolve_budget_implies_v2() {
        assert_eq!(resolve_from(None, None, true).unwrap(), RecallEngine::V2);
    }

    #[test]
    fn resolve_defaults_to_v2() {
        assert_eq!(resolve_from(None, None, false).unwrap(), RecallEngine::V2);
    }

    /// The previous engine stays one word away, from the request or from
    /// the environment.
    #[test]
    fn resolve_legacy_is_still_selectable() {
        assert_eq!(
            resolve_from(Some("legacy"), None, false).unwrap(),
            RecallEngine::Legacy
        );
        assert_eq!(
            resolve_from(None, Some("legacy"), false).unwrap(),
            RecallEngine::Legacy
        );
    }

    /// The legacy path cuts by count: it used to drop the budget silently.
    #[test]
    fn resolve_refuses_legacy_with_a_budget() {
        let err = resolve_from(Some("legacy"), None, true).unwrap_err();
        assert!(matches!(err, IcmError::InvalidInput(_)));
        assert!(err.to_string().contains("`engine`"), "{err}");
        assert!(err.to_string().contains("max_tokens"), "{err}");

        let err = resolve_from(None, Some("legacy"), true).unwrap_err();
        assert!(matches!(err, IcmError::InvalidInput(_)));
        assert!(err.to_string().contains(ENGINE_ENV), "{err}");

        // Explicit legacy over an environment v2 is still legacy, so the
        // budget is still refused.
        assert!(resolve_from(Some("legacy"), Some("v2"), true).is_err());
    }

    #[test]
    fn resolve_rejects_an_unknown_engine_in_the_request() {
        let err = resolve_from(Some("turbo"), None, false).unwrap_err();
        assert!(matches!(err, IcmError::InvalidInput(_)));
        assert!(err.to_string().contains("turbo"));
        // Even when the environment holds a valid one.
        assert!(resolve_from(Some("turbo"), Some("v2"), false).is_err());
    }

    /// A typo in the environment must not fail every recall: the value is
    /// ignored, as if the variable were unset.
    #[test]
    fn resolve_ignores_an_unknown_engine_in_the_environment() {
        assert_eq!(
            resolve_from(None, Some("v3"), false).unwrap(),
            RecallEngine::V2
        );
        assert_eq!(
            resolve_from(None, Some("legcay"), true).unwrap(),
            RecallEngine::V2
        );
        assert_eq!(
            resolve_from(Some("v2"), Some("v3"), false).unwrap(),
            RecallEngine::V2
        );
    }

    #[test]
    fn resolve_ignores_case_whitespace_and_blank_values() {
        assert_eq!(
            resolve_from(Some(" V2 "), None, false).unwrap(),
            RecallEngine::V2
        );
        assert_eq!(
            resolve_from(Some(""), Some("  "), false).unwrap(),
            RecallEngine::V2
        );
        assert_eq!(
            resolve_from(Some(""), Some(" Legacy "), false).unwrap(),
            RecallEngine::Legacy
        );
        assert_eq!(
            resolve_from(Some(""), Some("v2"), false).unwrap(),
            RecallEngine::V2
        );
    }

    // --- depth -------------------------------------------------------------

    #[test]
    fn depth_defaults_to_four_times_limit_within_bounds() {
        assert_eq!(depth_from(1, None), 100);
        assert_eq!(depth_from(5, None), 100);
        assert_eq!(depth_from(50, None), 200);
        assert_eq!(depth_from(200, None), 800);
        assert_eq!(depth_from(500, None), 2000);
    }

    #[test]
    fn depth_env_override_is_clamped_and_garbage_is_ignored() {
        assert_eq!(depth_from(5, Some("300")), 300);
        assert_eq!(depth_from(5, Some(" 300 ")), 300);
        assert_eq!(depth_from(5, Some("5")), 20);
        assert_eq!(depth_from(5, Some("999999")), 2000);
        assert_eq!(depth_from(5, Some("0")), 100);
        assert_eq!(depth_from(5, Some("deep")), 100);
        assert_eq!(depth_from(5, Some("-3")), 100);
        assert_eq!(depth_from(5, Some("")), 100);
    }

    // --- filters -----------------------------------------------------------

    #[test]
    fn filters_project_topic_keyword() {
        let mut m = mem("decisions-icm", "use sqlite");
        m.keywords = vec!["storage".into()];

        let mut req = request("q");
        assert!(passes_filters(&req, &m));

        req.project = Some("icm");
        assert!(passes_filters(&req, &m));
        req.project = Some("other");
        assert!(!passes_filters(&req, &m));
        // Empty project disables the filter.
        req.project = Some("");
        assert!(passes_filters(&req, &m));
        // Preference topics pass any project filter.
        req.project = Some("other");
        assert!(passes_filters(&req, &mem("preferences", "tabs")));

        req.project = None;
        req.topic = Some("decisions");
        assert!(passes_filters(&req, &m));
        req.topic = Some("errors");
        assert!(!passes_filters(&req, &m));

        req.topic = None;
        req.keyword = Some("stor");
        assert!(passes_filters(&req, &m));
        req.keyword = Some("network");
        assert!(!passes_filters(&req, &m));
    }

    // --- cut (no store) ----------------------------------------------------

    /// A summary of exactly `tokens * 4` characters.
    fn summary_of(tokens: usize) -> String {
        "x".repeat(tokens * 4)
    }

    /// A caller that renders the summary and eight tokens of framing.
    fn summary_cost(m: &Memory, _score: f32) -> usize {
        estimate_tokens(&m.summary) + 8
    }

    #[test]
    fn cut_charges_what_the_cost_function_says() {
        let ranked = vec![ranked_hit(&summary_of(10), 1.0)];
        let out = cut(ranked, Some(1000), 5, None, &summary_cost);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].est_tokens, 18);
        assert_eq!(out.used_tokens, 18);

        // The cost sees the score the hit is rendered with.
        let ranked = vec![ranked_hit("a", 0.25), ranked_hit("b", 0.75)];
        let by_score = |_: &Memory, score: f32| (score * 100.0) as usize;
        let out = cut(ranked, None, 5, None, &by_score);
        let charged: Vec<usize> = out.hits.iter().map(|h| h.est_tokens).collect();
        assert_eq!(charged, vec![25, 75]);
    }

    #[test]
    fn cut_by_budget_skips_and_resumes_in_rank_order() {
        // Costs with overhead: 48, 88, 38, 38.
        let ranked = vec![
            ranked_hit(&summary_of(40), 1.0),
            ranked_hit(&summary_of(80), 0.9),
            ranked_hit(&summary_of(30), 0.8),
            ranked_hit(&summary_of(30), 0.7),
        ];
        let out = cut(ranked, Some(130), 500, None, &summary_cost);
        let scores: Vec<f32> = out.hits.iter().map(|h| h.score).collect();
        assert_eq!(scores, vec![1.0, 0.8, 0.7]);
        assert_eq!(out.used_tokens, 48 + 38 + 38);
        assert_eq!(
            out.used_tokens,
            out.hits.iter().map(|h| h.est_tokens).sum::<usize>()
        );
        assert!(out.used_tokens <= 130);
        assert_eq!(out.skipped_for_budget, 1);
        assert_eq!(out.candidates, 4);
    }

    #[test]
    fn cut_keeps_a_single_oversized_hit() {
        let ranked = vec![
            ranked_hit(&summary_of(900), 1.0),
            ranked_hit(&summary_of(800), 0.9),
        ];
        let out = cut(ranked, Some(100), 500, None, &summary_cost);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].score, 1.0);
        assert!(out.used_tokens > 100);
        assert_eq!(out.skipped_for_budget, 1);
    }

    #[test]
    fn cut_budget_also_honors_limit() {
        let ranked = (0..10).map(|_| ranked_hit(&summary_of(2), 1.0)).collect();
        let out = cut(ranked, Some(10_000), 3, None, &summary_cost);
        assert_eq!(out.hits.len(), 3);
        assert_eq!(out.skipped_for_budget, 0);
        assert_eq!(out.candidates, 10);
    }

    #[test]
    fn cut_without_budget_truncates_to_limit() {
        let ranked = (0..10)
            .map(|i| ranked_hit(&summary_of(100), 1.0 - i as f32 * 0.05))
            .collect();
        let out = cut(ranked, None, 4, None, &summary_cost);
        assert_eq!(out.hits.len(), 4);
        assert_eq!(out.hits[0].score, 1.0);
        assert_eq!(out.used_tokens, 4 * 108);
        assert_eq!(out.candidates, 10);
        assert_eq!(out.skipped_for_budget, 0);
    }

    #[test]
    fn full_record_cost_counts_every_rendered_field() {
        let mut m = mem("errors-resolved", "short summary");
        let bare = full_record_cost(&m, 1.0);
        // Field names, id, three timestamps: far more than the summary.
        assert!(bare > 80, "{bare}");

        m.raw_excerpt = Some("e".repeat(4000));
        let with_raw = full_record_cost(&m, 1.0);
        assert!(with_raw >= bare + 990, "{with_raw} vs {bare}");

        m.keywords = vec!["k".repeat(400)];
        assert!(full_record_cost(&m, 1.0) >= with_raw + 100);

        // The embedding is never rendered, so it is not charged.
        let before = full_record_cost(&m, 1.0);
        m.embedding = Some(vec![0.5; 1024]);
        assert_eq!(full_record_cost(&m, 1.0), before);
    }

    #[test]
    fn cut_of_nothing_is_empty() {
        let out = cut(Vec::new(), Some(100), 5, None, &summary_cost);
        assert!(out.hits.is_empty());
        assert_eq!(out.used_tokens, 0);
        assert_eq!(out.candidates, 0);
    }
}

/// End-to-end pipeline tests. They need the SQLite ranked search, so they
/// are compiled only with that backend.
#[cfg(all(test, feature = "backend-sqlite"))]
mod pipeline_tests {
    use super::*;
    use chrono::TimeZone;
    use icm_core::Importance;

    const DIMS: usize = 64;

    /// Deterministic bag-of-words embedder: each word lights one of 64
    /// buckets, so texts sharing words are close.
    struct WordEmbedder;

    fn bucket(word: &str) -> usize {
        word.bytes().fold(7usize, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(usize::from(b))
        }) % DIMS
    }

    impl Embedder for WordEmbedder {
        fn embed(&self, text: &str) -> IcmResult<Vec<f32>> {
            let mut v = vec![0.0_f32; DIMS];
            for word in text.split(|c: char| !c.is_alphanumeric()) {
                if !word.is_empty() {
                    v[bucket(&word.to_lowercase())] += 1.0;
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
        fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            texts.iter().map(|t| self.embed(t)).collect()
        }
        fn dimensions(&self) -> usize {
            DIMS
        }
    }

    struct FailingEmbedder;
    impl Embedder for FailingEmbedder {
        fn embed(&self, _text: &str) -> IcmResult<Vec<f32>> {
            Err(IcmError::Embedding("model unavailable".into()))
        }
        fn embed_batch(&self, _texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            Err(IcmError::Embedding("model unavailable".into()))
        }
        fn dimensions(&self) -> usize {
            DIMS
        }
    }

    fn store() -> Store {
        Store::in_memory_with_dims(DIMS).unwrap()
    }

    fn add(store: &Store, topic: &str, summary: &str, embedder: Option<&dyn Embedder>) -> String {
        let mut m = Memory::new(topic.into(), summary.into(), Importance::Medium);
        if let Some(e) = embedder {
            m.embedding = Some(e.embed(&m.embed_text()).unwrap());
        }
        store.store(m).unwrap()
    }

    /// The pipeline without the environment override.
    fn run(
        store: &Store,
        embedder: Option<&dyn Embedder>,
        req: &RecallRequest<'_>,
    ) -> IcmResult<RecallOutcome> {
        recall_with_depth(store, embedder, req, &full_record_cost, None, true)
    }

    /// Every hit costs 100 tokens: makes budget arithmetic exact.
    fn flat_cost(_: &Memory, _: f32) -> usize {
        100
    }

    fn request(query: &str) -> RecallRequest<'_> {
        RecallRequest {
            query,
            limit: 5,
            max_tokens: None,
            topic: None,
            keyword: None,
            project: None,
            now: None,
            expand_neighbors: false,
        }
    }

    /// 120 memories that all match "deploy", each about 50 tokens long.
    fn seed_120(store: &Store, embedder: Option<&dyn Embedder>) {
        for i in 0..120 {
            let summary = format!(
                "deploy note {i:03}: the release pipeline step {i} needs a manual check \
                 before the rollout continues, see the runbook entry number {i} for the \
                 exact order of the commands and the rollback."
            );
            add(store, "ops-notes", &summary, embedder);
        }
    }

    /// What `icm recall -f json` prints for these hits: a pretty array of
    /// records, embedding dropped, `score` first. Built by hand because the
    /// CLI's row type is not visible from this crate.
    fn json_rendering(hits: &[RecallHit]) -> String {
        let rows: Vec<String> = hits
            .iter()
            .map(|h| {
                let mut record = h.memory.clone();
                record.embedding = None;
                let json = serde_json::to_string_pretty(&record).unwrap();
                let score = serde_json::to_string(&h.score).unwrap();
                let with_score = json.replacen("{\n", &format!("{{\n  \"score\": {score},\n"), 1);
                with_score
                    .lines()
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect();
        format!("[\n{}\n]", rows.join(",\n"))
    }

    #[test]
    fn budget_lifts_the_forty_cap_and_bounds_the_full_json_rendering() {
        let s = store();
        let emb = WordEmbedder;
        seed_120(&s, Some(&emb));

        let mut req = request("deploy release pipeline");
        req.limit = 200;
        req.max_tokens = Some(12_000);
        let out = recall_v2(&s, Some(&emb), &req).unwrap();

        assert!(
            out.hits.len() > 40,
            "legacy pool cap must be gone, got {}",
            out.hits.len()
        );
        let sum: usize = out.hits.iter().map(|h| h.est_tokens).sum();
        assert_eq!(sum, out.used_tokens);
        assert!(sum <= 12_000, "charged {sum} tokens, over budget");
        // The budget, not the candidate count, is what stopped the fill.
        assert!(out.skipped_for_budget > 0);
        assert_eq!(out.candidates, 120);

        // Not the estimator against itself: the largest rendering of the
        // returned hits is measured, and it fits.
        let rendered = estimate_tokens(&json_rendering(&out.hits));
        assert!(rendered <= 12_000, "rendered {rendered} tokens");
        // And the budget is used, not just respected.
        assert!(rendered * 10 >= 12_000 * 9, "rendered {rendered} tokens");
    }

    /// `raw_excerpt` and keywords are rendered, so they are charged.
    #[test]
    fn default_cost_bounds_the_rendering_of_memories_with_raw_excerpts() {
        let s = store();
        for i in 0..60 {
            let mut m = Memory::new(
                "errors-resolved".into(),
                format!("budget probe entry number {i} about the deploy pipeline"),
                Importance::Medium,
            );
            m.keywords = vec!["deploy".into(), "pipeline".into(), "budget".into()];
            m.raw_excerpt = Some(format!(
                "{i} {}",
                "error: \"connection refused\"\n at step; ".repeat(40)
            ));
            s.store(m).unwrap();
        }
        let mut req = request("budget probe entry");
        req.limit = 200;
        req.max_tokens = Some(4000);
        let out = recall_v2(&s, None, &req).unwrap();

        assert!(out.hits.len() >= 2, "{} hits", out.hits.len());
        assert!(out.hits.len() < 60);
        let rendered = estimate_tokens(&json_rendering(&out.hits));
        assert!(rendered <= 4000, "rendered {rendered} tokens");
    }

    #[test]
    fn the_callers_cost_function_drives_the_cut() {
        let s = store();
        seed_120(&s, None);

        let mut req = request("deploy");
        req.limit = 200;
        req.max_tokens = Some(1050);
        let out = recall_with_depth(&s, None, &req, &flat_cost, None, true).unwrap();
        assert_eq!(out.hits.len(), 10);
        assert_eq!(out.used_tokens, 1000);
        assert!(out.hits.iter().all(|h| h.est_tokens == 100));

        // The public entry point takes the same function.
        let out = req.run_with_cost(&s, None, &flat_cost).unwrap();
        assert_eq!(out.used_tokens, 1000);
    }

    #[test]
    fn single_result_larger_than_budget_is_still_returned() {
        let s = store();
        add(
            &s,
            "notes",
            &format!("deploy {}", "word ".repeat(400)),
            None,
        );

        let mut req = request("deploy");
        req.max_tokens = Some(10);
        let out = run(&s, None, &req).unwrap();

        assert_eq!(out.hits.len(), 1);
        assert!(out.used_tokens > 10);
    }

    #[test]
    fn without_budget_cuts_to_limit() {
        let s = store();
        seed_120(&s, None);

        let mut req = request("deploy");
        req.limit = 7;
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 7);
        assert_eq!(out.skipped_for_budget, 0);

        // A zero limit is raised to one, not treated as "nothing".
        req.limit = 0;
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 1);
    }

    #[test]
    fn filters_are_applied_before_the_cut() {
        let s = store();
        // 60 better-matching memories of another project, then 5 of ours.
        for i in 0..60 {
            add(
                &s,
                "decisions-other",
                &format!("cache cache cache eviction policy {i}"),
                None,
            );
        }
        let mut ours = Vec::new();
        for i in 0..5 {
            ours.push(add(
                &s,
                "decisions-icm",
                &format!("cache eviction in the icm store, variant {i}"),
                None,
            ));
        }
        add(&s, "preferences", "cache answers in French", None);

        let mut req = request("cache eviction");
        req.limit = 5;
        req.project = Some("icm");
        req.topic = Some("decisions");
        let out = run(&s, None, &req).unwrap();
        let mut got: Vec<String> = out.hits.iter().map(|h| h.memory.id.clone()).collect();
        got.sort();
        ours.sort();
        assert_eq!(got, ours);

        // Project filter alone lets the preference memory through.
        let mut req = request("cache");
        req.limit = 50;
        req.project = Some("icm");
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 6);
        assert!(out.hits.iter().any(|h| h.memory.topic == "preferences"));
        assert!(out.hits.iter().all(|h| h.memory.topic != "decisions-other"));
    }

    #[test]
    fn keyword_filter_selects_by_stored_keywords() {
        let s = store();
        let mut tagged = Memory::new(
            "notes".into(),
            "rotate the api token monthly".into(),
            Importance::Medium,
        );
        tagged.keywords = vec!["security".into()];
        let tagged_id = s.store(tagged).unwrap();
        add(&s, "notes", "the api token lives in the vault", None);

        let mut req = request("api token");
        req.keyword = Some("security");
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].memory.id, tagged_id);
    }

    #[test]
    fn now_anchors_the_query_time_window() {
        let s = store();
        add(&s, "notes", "we decided to ship on friday", None);
        let now = Utc.with_ymd_and_hms(2023, 5, 10, 12, 0, 0).unwrap();

        let mut req = request("what did we decide yesterday");
        req.now = Some(now);
        let out = run(&s, None, &req).unwrap();
        let window = out.window.expect("'yesterday' must produce a window");
        assert_eq!(
            window.start,
            Utc.with_ymd_and_hms(2023, 5, 9, 0, 0, 0).unwrap()
        );
        assert_eq!(
            window.end,
            Utc.with_ymd_and_hms(2023, 5, 10, 0, 0, 0).unwrap()
        );

        let mut req = request("what did we decide");
        req.now = Some(now);
        let out = run(&s, None, &req).unwrap();
        assert!(out.window.is_none());
    }

    #[test]
    fn a_memory_dated_in_the_window_is_ranked_first() {
        let s = store();
        let now = Utc.with_ymd_and_hms(2023, 5, 10, 12, 0, 0).unwrap();
        for i in 0..6 {
            add(&s, "notes", &format!("standup summary number {i}"), None);
        }
        let mut dated = Memory::new(
            "notes".into(),
            "standup summary about the outage".into(),
            Importance::Medium,
        );
        dated.created_at = Utc.with_ymd_and_hms(2023, 5, 9, 9, 0, 0).unwrap();
        let dated_id = s.store(dated).unwrap();

        let mut req = request("standup summary yesterday");
        req.now = Some(now);
        req.limit = 10;
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits[0].memory.id, dated_id);
        assert_eq!(out.hits[0].time_rank, Some(1));
    }

    #[test]
    fn access_counts_are_bumped_for_returned_hits_only() {
        let s = store();
        let hit = add(&s, "notes", "deploy checklist for the api", None);
        let miss = add(&s, "notes", "grocery list", None);

        let out = run(&s, None, &request("deploy checklist")).unwrap();
        assert_eq!(out.hits.len(), 1);

        assert_eq!(s.get(&hit).unwrap().unwrap().access_count, 1);
        assert_eq!(s.get(&miss).unwrap().unwrap().access_count, 0);
    }

    /// A budgeted recall returns hundreds of memories: recording an
    /// access for each would hold back the decay of all of them.
    #[test]
    fn access_is_recorded_for_the_best_hits_only() {
        let s = store();
        seed_120(&s, None);

        let mut req = request("deploy");
        req.limit = 200;
        req.max_tokens = Some(1_000_000);
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 120);

        for (i, hit) in out.hits.iter().enumerate() {
            let count = s.get(&hit.memory.id).unwrap().unwrap().access_count;
            let expected = u32::from(i < ACCESS_RECORD_MAX);
            assert_eq!(count, expected, "hit {i}");
        }
    }

    #[test]
    fn works_without_an_embedder_and_with_a_failing_one() {
        let s = store();
        let id = add(&s, "notes", "deploy checklist for the api", None);

        let out = run(&s, None, &request("deploy")).unwrap();
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].memory.id, id);
        assert!(out.hits[0].fts_rank.is_some());
        assert!(out.hits[0].vec_rank.is_none());

        let failing = FailingEmbedder;
        let out = run(&s, Some(&failing), &request("deploy")).unwrap();
        assert_eq!(out.hits.len(), 1);
        assert!(out.hits[0].vec_rank.is_none());
    }

    #[test]
    fn embedder_adds_the_vector_arm() {
        let s = store();
        let emb = WordEmbedder;
        let id = add(&s, "notes", "deploy checklist for the api", Some(&emb));
        add(&s, "notes", "grocery list for the weekend", Some(&emb));

        let out = run(&s, Some(&emb), &request("deploy checklist")).unwrap();
        assert_eq!(out.hits[0].memory.id, id);
        assert_eq!(out.hits[0].fts_rank, Some(1));
        assert_eq!(out.hits[0].vec_rank, Some(1));
    }

    #[test]
    fn neighbors_are_expanded_then_filtered() {
        let s = store();
        // The neighbor shares no word with the query.
        let neighbor = add(&s, "decisions-other", "postgres pool sizing", None);
        let mut primary = Memory::new(
            "decisions-icm".into(),
            "deploy checklist for the api".into(),
            Importance::Medium,
        );
        primary.related_ids = vec![neighbor.clone()];
        let primary_id = s.store(primary).unwrap();

        // Off: the neighbor is not reachable.
        let out = run(&s, None, &request("deploy checklist")).unwrap();
        assert_eq!(out.hits.len(), 1);

        // On: it follows the primary hit at half its score, with no ranks.
        let mut req = request("deploy checklist");
        req.expand_neighbors = true;
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 2);
        assert_eq!(out.hits[0].memory.id, primary_id);
        assert_eq!(out.hits[1].memory.id, neighbor);
        assert!((out.hits[1].score - out.hits[0].score * 0.5).abs() < 1e-6);
        assert!(out.hits[1].fts_rank.is_none());
        assert_eq!(out.hits[0].fts_rank, Some(1));

        // On with a project filter: the cross-project neighbor is dropped.
        req.project = Some("icm");
        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].memory.id, primary_id);
    }

    /// On RRF scores a neighbor at half its parent's
    /// score (0.5) outranked a genuine single-arm hit at rank 2 (0.4919)
    /// and took its place.
    #[test]
    fn a_neighbor_never_takes_the_place_of_a_ranked_hit() {
        let s = store();
        let emb = WordEmbedder;
        // Shares no word with the query.
        let neighbor = add(&s, "notes", "postgres pool sizing", None);
        // Best hit, embedded, linked to the neighbor.
        let mut top = Memory::new(
            "notes".into(),
            "deploy checklist for the api".into(),
            Importance::Medium,
        );
        top.embedding = Some(emb.embed(&top.embed_text()).unwrap());
        top.related_ids = vec![neighbor.clone()];
        let top_id = s.store(top).unwrap();
        // Genuine lexical hits without a vector (partially embedded store).
        let second = add(&s, "notes", "deploy checklist for the worker", None);
        let third = add(&s, "notes", "deploy steps for the cron", None);

        let ids = |out: &RecallOutcome| -> Vec<String> {
            out.hits.iter().map(|h| h.memory.id.clone()).collect()
        };

        // Three places, three ranked hits: no room for the neighbor.
        let mut req = request("deploy checklist");
        req.expand_neighbors = true;
        req.limit = 3;
        let out = run(&s, Some(&emb), &req).unwrap();
        assert_eq!(
            ids(&out),
            vec![top_id.clone(), second.clone(), third.clone()]
        );

        // Two places: still the two best ranked hits.
        req.limit = 2;
        let out = run(&s, Some(&emb), &req).unwrap();
        assert_eq!(ids(&out), vec![top_id.clone(), second.clone()]);

        // A fourth place is free: the neighbor takes it, last, and does
        // not score above any ranked hit.
        req.limit = 4;
        let out = run(&s, Some(&emb), &req).unwrap();
        assert_eq!(
            ids(&out),
            vec![
                top_id.clone(),
                second.clone(),
                third.clone(),
                neighbor.clone()
            ]
        );
        assert!(out.hits[3].score <= out.hits[2].score);
        assert!(out.hits[3].fts_rank.is_none() && out.hits[3].vec_rank.is_none());
    }

    #[test]
    fn a_neighbor_only_enters_the_budget_that_is_left() {
        let s = store();
        let neighbor = add(&s, "notes", "postgres pool sizing", None);
        let mut primary = Memory::new(
            "notes".into(),
            "deploy checklist for the api".into(),
            Importance::Medium,
        );
        primary.related_ids = vec![neighbor.clone()];
        s.store(primary).unwrap();
        add(&s, "notes", "deploy checklist for the worker", None);

        let mut req = request("deploy checklist");
        req.expand_neighbors = true;
        req.limit = 10;

        // Two ranked hits at 100 tokens each leave 50: not enough.
        req.max_tokens = Some(250);
        let out = recall_with_depth(&s, None, &req, &flat_cost, None, true).unwrap();
        assert_eq!(out.hits.len(), 2);
        assert!(out.hits.iter().all(|h| h.memory.id != neighbor));
        assert_eq!(out.used_tokens, 200);

        // 100 left: the neighbor fits, after the ranked hits.
        req.max_tokens = Some(300);
        let out = recall_with_depth(&s, None, &req, &flat_cost, None, true).unwrap();
        assert_eq!(out.hits.len(), 3);
        assert_eq!(out.hits[2].memory.id, neighbor);
        assert_eq!(out.used_tokens, 300);

        // A single oversized hit already overran the budget: nothing more.
        req.max_tokens = Some(40);
        let out = recall_with_depth(&s, None, &req, &flat_cost, None, true).unwrap();
        assert_eq!(out.hits.len(), 1);
    }

    #[test]
    fn depth_override_narrows_the_candidates() {
        let s = store();
        seed_120(&s, None);

        let mut req = request("deploy");
        req.limit = 200;
        req.max_tokens = Some(100_000);
        let out = recall_with_depth(&s, None, &req, &flat_cost, Some("20"), true).unwrap();
        assert_eq!(out.candidates, 20);
        assert_eq!(out.hits.len(), 20);

        let out = run(&s, None, &req).unwrap();
        assert_eq!(out.hits.len(), 120);
    }

    /// The lexical index matches whole words: without a vector arm, a
    /// query word that is only part of a stored word used to find nothing
    /// on v2 where the legacy keyword fallback found it.
    #[test]
    fn substring_fallback_when_no_arm_matches() {
        let s = store();
        let both = add(&s, "notes", "the deployment broke the workers", None);
        let one = add(&s, "notes", "deployment checklist", None);
        add(&s, "notes", "grocery list", None);

        let out = run(&s, None, &request("deploy worker")).unwrap();
        let ids: Vec<&str> = out.hits.iter().map(|h| h.memory.id.as_str()).collect();
        // Most query words matched first.
        assert_eq!(ids, vec![both.as_str(), one.as_str()]);
        assert!(out.hits.iter().all(|h| h.score == 0.0));
        assert!(out.hits.iter().all(|h| h.fts_rank.is_none()));

        // Filters and limit apply to the fallback too.
        let mut req = request("deploy worker");
        req.topic = Some("other");
        assert!(run(&s, None, &req).unwrap().hits.is_empty());
        let mut req = request("deploy worker");
        req.limit = 1;
        assert_eq!(run(&s, None, &req).unwrap().hits.len(), 1);

        // Words under three characters are not searched, though "st" is
        // inside "grocery list".
        assert!(run(&s, None, &request("st")).unwrap().hits.is_empty());
    }

    /// A whole-word match never goes through the fallback.
    #[test]
    fn substring_fallback_is_not_used_when_the_index_matches() {
        let s = store();
        let exact = add(&s, "notes", "deploy checklist", None);
        add(&s, "notes", "deployment broke", None);

        let out = run(&s, None, &request("deploy")).unwrap();
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].memory.id, exact);
        assert_eq!(out.hits[0].fts_rank, Some(1));
    }

    #[test]
    fn read_only_run_writes_nothing() {
        let s = store();
        let id = add(&s, "notes", "deploy checklist for the api", None);

        let out = request("deploy")
            .run_read_only(&s, None, &full_record_cost)
            .unwrap();
        assert_eq!(out.hits.len(), 1);
        let after = s.get(&id).unwrap().unwrap();
        assert_eq!(after.access_count, 0);
        assert_eq!(after.weight, 1.0);
        assert_eq!(s.get_metadata_str("last_decay_at").unwrap(), None);

        // The ordinary run does both.
        run(&s, None, &request("deploy")).unwrap();
        assert_eq!(s.get(&id).unwrap().unwrap().access_count, 1);
        assert!(s.get_metadata_str("last_decay_at").unwrap().is_some());
    }

    #[test]
    fn blank_query_is_rejected() {
        let s = store();
        let err = run(&s, None, &request("   ")).unwrap_err();
        assert!(matches!(err, IcmError::InvalidInput(_)));
    }

    #[test]
    fn no_match_gives_an_empty_outcome() {
        let s = store();
        add(&s, "notes", "deploy checklist", None);
        let out = run(&s, None, &request("zebra")).unwrap();
        assert!(out.hits.is_empty());
        assert_eq!(out.used_tokens, 0);
        assert_eq!(out.candidates, 0);
    }
}
