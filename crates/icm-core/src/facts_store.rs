//! Storage trait for the structured-facts layer.

use crate::error::IcmResult;
use crate::facts::{Fact, FactsStats};

/// Read/write API for the `facts` table.
///
/// Implementations enforce one **active** row per `(entity, key)` slot:
/// [`set_fact`] either inserts a new row or supersedes any existing
/// active one. [`get_fact`] returns the active row only;
/// [`history`] returns the full chain of supersessions for an audit
/// trail.
pub trait FactsStore {
    /// Insert or supersede: if an active row exists for the same
    /// `(entity, key)` and its `value` differs, mark it superseded
    /// now and insert a new active row carrying the new value.
    /// If `value` is unchanged, the call is a no-op (returns the
    /// existing row's id). Returns the active row's id after the
    /// operation.
    fn set_fact(&self, entity: &str, key: &str, value: &str, source: &str) -> IcmResult<String>;

    /// Return the active fact for `(entity, key)`, or `None` if no
    /// row matches or the most-recent row was forgotten.
    fn get_fact(&self, entity: &str, key: &str) -> IcmResult<Option<Fact>>;

    /// List **active** facts for an entity, optionally filtered to
    /// keys with the given prefix. Keys are returned alphabetically
    /// so output is stable across runs.
    fn list_facts(&self, entity: &str, key_prefix: Option<&str>) -> IcmResult<Vec<Fact>>;

    /// List the full supersession history for `(entity, key)`,
    /// newest first. Useful for "when did the value change?"
    /// audits.
    fn history(&self, entity: &str, key: &str) -> IcmResult<Vec<Fact>>;

    /// Hard-delete every row (active + history) for `(entity, key)`.
    fn forget_fact(&self, entity: &str, key: &str) -> IcmResult<usize>;

    /// Aggregate stats for the `icm facts stats` command.
    fn facts_stats(&self) -> IcmResult<FactsStats>;
}
