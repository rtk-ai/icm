//! Rank fusion for the v2 recall engine.
//!
//! Reciprocal Rank Fusion merges several ranked id lists (lexical, vector,
//! temporal) using positions only, never raw scores: BM25 ranks and cosine
//! similarities live on unrelated scales, and blending them by weight lets
//! whichever arm happens to score higher dominate. With ranks, a memory
//! that has no embedding competes on equal terms with one that has.
//!
//! The backend-facing query/hit types live here too because `backend.rs`
//! in icm-store is compiled without the SQLite module (Postgres-only
//! builds), so they cannot be defined next to `SqliteStore`.

use std::collections::HashMap;

use crate::memory::Memory;
use crate::temporal::TimeWindow;

/// Standard RRF damping constant (Cormack et al., 2009).
pub const RRF_K: f32 = 60.0;

/// One ranked list of memory ids, best first.
#[derive(Debug, Clone, Copy)]
pub struct RankedList<'a> {
    /// Label for diagnostics ("fts", "vec", "time"); not used in scoring.
    pub name: &'static str,
    pub ids: &'a [String],
    /// Multiplier on this list's contribution. Non-finite or negative
    /// weights count as 0.
    pub weight: f32,
}

/// One fused result.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedHit {
    pub id: String,
    /// Normalized to (0, 1]: 1.0 means first in every non-empty list.
    pub score: f32,
    /// 1-based rank in each input list, in input order; `None` = absent.
    pub ranks: Vec<Option<u32>>,
}

/// Parameters of a ranked (v2) search.
#[derive(Debug, Clone, Copy)]
pub struct RankedQuery<'a> {
    pub text: &'a str,
    pub embedding: Option<&'a [f32]>,
    /// Results returned after fusion.
    pub limit: usize,
    /// Candidates requested from each arm.
    pub depth: usize,
    /// Enables the temporal arm.
    pub window: Option<TimeWindow>,
}

/// One ranked (v2) search result, with its 1-based rank in each arm.
#[derive(Debug, Clone)]
pub struct RankedHit {
    pub memory: Memory,
    pub score: f32,
    pub fts_rank: Option<u32>,
    pub vec_rank: Option<u32>,
    pub time_rank: Option<u32>,
}

fn effective_weight(weight: f32) -> f64 {
    if weight.is_finite() && weight > 0.0 {
        f64::from(weight)
    } else {
        0.0
    }
}

/// Fuse ranked lists by Reciprocal Rank Fusion.
///
/// Raw score of an id = sum over lists of `weight / (k + rank)`, rank
/// starting at 1. The returned score is that sum divided by the best
/// achievable one, `sum(weight / (k + 1))` over the non-empty lists: an
/// empty list (no embedder, no time window) is left out so it does not
/// squash every score. If every non-empty list has zero weight, all
/// scores are 0.
///
/// Order: score descending, then best rank across lists ascending, then
/// id ascending, so the output is fully deterministic. An id repeated
/// within a list counts at its first position; later ids keep their
/// position in the list as given. `k` non-finite or <= 0 falls back to
/// [`RRF_K`]. Never panics.
pub fn rrf_fuse(lists: &[RankedList<'_>], k: f32) -> Vec<FusedHit> {
    let k = if k.is_finite() && k > 0.0 {
        f64::from(k)
    } else {
        f64::from(RRF_K)
    };

    struct Acc<'a> {
        id: &'a str,
        raw: f64,
        ranks: Vec<Option<u32>>,
    }

    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut accs: Vec<Acc<'_>> = Vec::new();
    let mut best_possible = 0.0_f64;

    for (list_idx, list) in lists.iter().enumerate() {
        if list.ids.is_empty() {
            continue;
        }
        let weight = effective_weight(list.weight);
        best_possible += weight / (k + 1.0);

        for (pos, id) in list.ids.iter().enumerate() {
            let rank = u32::try_from(pos + 1).unwrap_or(u32::MAX);
            let slot = *index.entry(id.as_str()).or_insert_with(|| {
                accs.push(Acc {
                    id: id.as_str(),
                    raw: 0.0,
                    ranks: vec![None; lists.len()],
                });
                accs.len() - 1
            });
            let acc = &mut accs[slot];
            if acc.ranks[list_idx].is_some() {
                continue; // duplicate within this list: first position wins
            }
            acc.ranks[list_idx] = Some(rank);
            acc.raw += weight / (k + f64::from(rank));
        }
    }

    let mut hits: Vec<FusedHit> = accs
        .into_iter()
        .map(|acc| {
            let score = if best_possible > 0.0 {
                ((acc.raw / best_possible) as f32).min(1.0)
            } else {
                0.0
            };
            FusedHit {
                id: acc.id.to_string(),
                score,
                ranks: acc.ranks,
            }
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| best_rank(a).cmp(&best_rank(b)))
            .then_with(|| a.id.cmp(&b.id))
    });
    hits
}

fn best_rank(hit: &FusedHit) -> u32 {
    hit.ranks
        .iter()
        .flatten()
        .copied()
        .min()
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn list<'a>(name: &'static str, ids: &'a [String]) -> RankedList<'a> {
        RankedList {
            name,
            ids,
            weight: 1.0,
        }
    }

    fn order(hits: &[FusedHit]) -> Vec<&str> {
        hits.iter().map(|h| h.id.as_str()).collect()
    }

    #[test]
    fn present_in_two_lists_beats_first_of_one() {
        let fts = ids(&["solo", "both"]);
        let vec = ids(&["other", "both"]);
        let hits = rrf_fuse(&[list("fts", &fts), list("vec", &vec)], RRF_K);

        assert_eq!(hits[0].id, "both");
        assert_eq!(hits[0].ranks, vec![Some(2), Some(2)]);
        // 2/(60+2) against 1/(60+1)
        assert!(hits[0].score > hits[1].score);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn first_everywhere_scores_exactly_one() {
        let fts = ids(&["a", "b", "c"]);
        let vec = ids(&["a", "c", "b"]);
        let time = ids(&["a"]);
        let hits = rrf_fuse(
            &[list("fts", &fts), list("vec", &vec), list("time", &time)],
            RRF_K,
        );

        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[0].score, 1.0);
        for h in &hits {
            assert!(h.score > 0.0 && h.score <= 1.0, "{h:?}");
        }
    }

    #[test]
    fn empty_list_is_left_out_of_normalization() {
        let fts = ids(&["a", "b"]);
        let none: Vec<String> = Vec::new();
        let hits = rrf_fuse(&[list("fts", &fts), list("vec", &none)], RRF_K);

        // Without the exclusion "a" would score 0.5.
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[0].score, 1.0);
        assert_eq!(hits[0].ranks, vec![Some(1), None]);
        let expected_b = (1.0_f64 / 62.0) / (1.0 / 61.0);
        assert!((f64::from(hits[1].score) - expected_b).abs() < 1e-6);
    }

    #[test]
    fn no_lists_or_all_empty_gives_nothing() {
        assert!(rrf_fuse(&[], RRF_K).is_empty());
        let none: Vec<String> = Vec::new();
        assert!(rrf_fuse(&[list("fts", &none), list("vec", &none)], RRF_K).is_empty());
    }

    #[test]
    fn ties_break_on_best_rank_then_id() {
        // "b" and "a" are symmetric (ranks 1 and 2, swapped): same score,
        // same best rank, so id decides.
        let fts = ids(&["b", "a"]);
        let vec = ids(&["a", "b"]);
        let hits = rrf_fuse(&[list("fts", &fts), list("vec", &vec)], RRF_K);
        assert_eq!(hits[0].score, hits[1].score);
        assert_eq!(order(&hits), vec!["a", "b"]);

        // Two single-list ids at the same rank: exact tie again.
        let left = ids(&["z"]);
        let right = ids(&["y"]);
        let hits = rrf_fuse(&[list("fts", &left), list("vec", &right)], RRF_K);
        assert_eq!(hits[0].score, hits[1].score);
        assert_eq!(order(&hits), vec!["y", "z"]);
    }

    #[test]
    fn equal_score_prefers_better_best_rank() {
        // k = 1: "top" scores 1/2 from one list at rank 1; "mid" scores
        // 1/4 + 1/4 from two lists at rank 3. Equal sums, "top" has the
        // better best rank and must come first despite sorting after "mid".
        let fts = ids(&["top", "x1", "mid"]);
        let vec = ids(&["x2", "x3", "mid"]);
        let hits = rrf_fuse(&[list("fts", &fts), list("vec", &vec)], 1.0);
        let top = hits.iter().position(|h| h.id == "top");
        let mid = hits.iter().position(|h| h.id == "mid");
        let (Some(top), Some(mid)) = (top, mid) else {
            panic!("both ids must be present: {hits:?}");
        };
        assert_eq!(hits[top].score, hits[mid].score);
        assert!(top < mid, "{hits:?}");
    }

    #[test]
    fn duplicate_id_counts_at_first_position() {
        let dup = ids(&["a", "b", "a", "c"]);
        let hits = rrf_fuse(&[list("fts", &dup)], RRF_K);

        assert_eq!(order(&hits), vec!["a", "b", "c"]);
        assert_eq!(hits[0].ranks, vec![Some(1)]);
        assert_eq!(hits[0].score, 1.0); // not 1/61 + 1/63
        assert_eq!(hits[2].ranks, vec![Some(4)]);
    }

    #[test]
    fn invalid_k_falls_back_to_default() {
        let fts = ids(&["a", "b", "c"]);
        let vec = ids(&["c", "a"]);
        let lists = [list("fts", &fts), list("vec", &vec)];
        let reference = rrf_fuse(&lists, RRF_K);

        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(rrf_fuse(&lists, bad), reference, "k = {bad}");
        }
        // A valid k other than the default does change the scores.
        assert_ne!(rrf_fuse(&lists, 1.0), reference);
    }

    #[test]
    fn weights_shift_the_order() {
        let fts = ids(&["lex", "sem"]);
        let vec = ids(&["sem", "lex"]);
        let weighted = |w_fts: f32, w_vec: f32| {
            rrf_fuse(
                &[
                    RankedList {
                        name: "fts",
                        ids: &fts,
                        weight: w_fts,
                    },
                    RankedList {
                        name: "vec",
                        ids: &vec,
                        weight: w_vec,
                    },
                ],
                RRF_K,
            )
        };

        assert_eq!(weighted(3.0, 1.0)[0].id, "lex");
        assert_eq!(weighted(1.0, 3.0)[0].id, "sem");

        // The heavier list's leader is no longer at 1.0: it is not first
        // everywhere.
        let hits = weighted(3.0, 1.0);
        let expected = (3.0_f64 / 61.0 + 1.0 / 62.0) / (4.0 / 61.0);
        assert!((f64::from(hits[0].score) - expected).abs() < 1e-6);
    }

    #[test]
    fn invalid_weights_count_as_zero_without_panicking() {
        let fts = ids(&["a", "b"]);
        let vec = ids(&["b", "a"]);
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let hits = rrf_fuse(
                &[
                    list("fts", &fts),
                    RankedList {
                        name: "vec",
                        ids: &vec,
                        weight: bad,
                    },
                ],
                RRF_K,
            );
            // Only the valid list drives the order; ranks are still reported.
            assert_eq!(order(&hits), vec!["a", "b"], "weight = {bad}");
            assert_eq!(hits[0].score, 1.0);
            assert_eq!(hits[0].ranks, vec![Some(1), Some(2)]);
            assert!(hits.iter().all(|h| h.score.is_finite()));
        }

        // Nothing carries weight: scores are 0, order falls back to rank.
        let hits = rrf_fuse(
            &[RankedList {
                name: "fts",
                ids: &fts,
                weight: 0.0,
            }],
            RRF_K,
        );
        assert_eq!(order(&hits), vec!["a", "b"]);
        assert!(hits.iter().all(|h| h.score == 0.0));
    }
}
