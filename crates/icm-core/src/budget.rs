//! Token-budget selection for the v2 recall engine.
//!
//! A recall can be cut by a token budget instead of a result count: the
//! caller states how much context it can afford and gets as many ranked
//! results as fit. What a result costs is the caller's business (the text
//! it renders for it); this module only counts and selects. Token counts
//! here are an ESTIMATE (four characters per token, the convention
//! `wake_up` already uses), not a tokenizer run: a caller that needs an
//! exact figure must measure the rendered output with its own tokenizer
//! and tune the budget accordingly.

/// Flat per-result allowance for what a renderer adds around a memory's
/// own fields (its score, separators), for callers that charge a memory
/// without knowing the exact rendering.
pub const ITEM_OVERHEAD_TOKENS: usize = 8;

/// Estimated token count of `text`: `ceil(characters / 4)`, 0 when empty.
///
/// Counts Unicode scalar values, not bytes, so accented or CJK text is not
/// overcharged for its UTF-8 width.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Which items of a ranked list were kept under a token budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetSelection {
    /// Indices into the input, in input order.
    pub kept: Vec<usize>,
    /// Sum of the kept costs. Exceeds the budget only when a single
    /// oversized first item was kept.
    pub used_tokens: usize,
    /// Items passed over because they did not fit.
    pub skipped: usize,
}

/// Greedy fill of `max_tokens` over `costs`, walked in ranking order.
///
/// An item that does not fit is skipped and the walk continues, so a long
/// result does not shut out the shorter ones ranked below it. The walk stops
/// once `max_items` are kept. If nothing fits, the first item is kept whole:
/// a non-empty input never yields an empty selection, unless `max_items`
/// is 0.
///
/// Unlike `wake_up`'s budget cut, nothing is truncated and the walk does not
/// stop at the first overflow.
pub fn select_within_budget(
    costs: &[usize],
    max_tokens: usize,
    max_items: usize,
) -> BudgetSelection {
    let mut kept = Vec::new();
    let mut used_tokens = 0usize;
    let mut skipped = 0usize;

    if max_items == 0 {
        return BudgetSelection {
            kept,
            used_tokens,
            skipped,
        };
    }

    for (i, &cost) in costs.iter().enumerate() {
        if kept.len() >= max_items {
            break;
        }
        match used_tokens.checked_add(cost) {
            Some(total) if total <= max_tokens => {
                kept.push(i);
                used_tokens = total;
            }
            _ => skipped += 1,
        }
    }

    if kept.is_empty()
        && let Some(&first) = costs.first()
    {
        kept.push(0);
        used_tokens = first;
        skipped = skipped.saturating_sub(1);
    }

    BudgetSelection {
        kept,
        used_tokens,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_rounds_up_and_handles_empty() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens(&"x".repeat(400)), 100);
    }

    #[test]
    fn estimate_counts_characters_not_bytes() {
        // 8 characters, 16 bytes in UTF-8.
        let accented = "éééééééé";
        assert_eq!(accented.len(), 16);
        assert_eq!(estimate_tokens(accented), 2);
        // 4 characters, 12 bytes.
        assert_eq!(estimate_tokens("日本語文"), 1);
        // 5 characters, 20 bytes.
        assert_eq!(estimate_tokens("🦀🦀🦀🦀🦀"), 2);
    }

    #[test]
    fn skips_what_does_not_fit_then_resumes() {
        // 40 fits, 80 does not (40 + 80 > 100), 30 and 30 do, 10 does not.
        let sel = select_within_budget(&[40, 80, 30, 30, 10], 100, usize::MAX);
        assert_eq!(sel.kept, vec![0, 2, 3]);
        assert_eq!(sel.used_tokens, 100);
        assert_eq!(sel.skipped, 2);
    }

    #[test]
    fn first_item_is_kept_when_nothing_fits() {
        let sel = select_within_budget(&[500, 400, 300], 100, usize::MAX);
        assert_eq!(sel.kept, vec![0]);
        assert_eq!(sel.used_tokens, 500);
        assert_eq!(sel.skipped, 2);
    }

    #[test]
    fn oversized_first_item_does_not_block_smaller_ones() {
        // The first item is too large but a later one fits: the fallback
        // must not fire, and the oversized item stays out.
        let sel = select_within_budget(&[500, 60, 60], 100, usize::MAX);
        assert_eq!(sel.kept, vec![1]);
        assert_eq!(sel.used_tokens, 60);
        assert_eq!(sel.skipped, 2);
    }

    #[test]
    fn stops_at_max_items() {
        let sel = select_within_budget(&[10, 10, 10, 10], 1000, 2);
        assert_eq!(sel.kept, vec![0, 1]);
        assert_eq!(sel.used_tokens, 20);
        // Items past the cap were never considered, so they are not
        // budget skips.
        assert_eq!(sel.skipped, 0);
    }

    #[test]
    fn max_items_counts_kept_items_not_visited_ones() {
        let sel = select_within_budget(&[90, 50, 5, 5, 5], 100, 2);
        assert_eq!(sel.kept, vec![0, 2]);
        assert_eq!(sel.used_tokens, 95);
        assert_eq!(sel.skipped, 1);
    }

    #[test]
    fn zero_budget_still_returns_the_first() {
        let sel = select_within_budget(&[12, 9], 0, usize::MAX);
        assert_eq!(sel.kept, vec![0]);
        assert_eq!(sel.used_tokens, 12);
        assert_eq!(sel.skipped, 1);
    }

    #[test]
    fn exact_fit_is_kept() {
        let sel = select_within_budget(&[25, 25, 50], 100, usize::MAX);
        assert_eq!(sel.kept, vec![0, 1, 2]);
        assert_eq!(sel.used_tokens, 100);
        assert_eq!(sel.skipped, 0);

        // One token over: the last one is out.
        let sel = select_within_budget(&[25, 25, 51], 100, usize::MAX);
        assert_eq!(sel.kept, vec![0, 1]);
        assert_eq!(sel.skipped, 1);
    }

    #[test]
    fn empty_input_and_zero_items() {
        let sel = select_within_budget(&[], 100, 10);
        assert!(sel.kept.is_empty());
        assert_eq!(sel.used_tokens, 0);
        assert_eq!(sel.skipped, 0);

        let sel = select_within_budget(&[10, 10], 100, 0);
        assert!(sel.kept.is_empty());
        assert_eq!(sel.used_tokens, 0);
    }

    #[test]
    fn huge_costs_do_not_overflow() {
        let sel = select_within_budget(&[usize::MAX, usize::MAX, 3], usize::MAX, usize::MAX);
        assert_eq!(sel.kept, vec![0]);
        assert_eq!(sel.used_tokens, usize::MAX);
        assert_eq!(sel.skipped, 2);
    }
}
