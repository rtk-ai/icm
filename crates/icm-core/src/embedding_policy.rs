//! Which embedding model a process may use against an existing database.
//!
//! The vectors already stored in a database were produced by one model at
//! one dimension. A process that opens it with another dimension cannot use
//! them, and writing vectors from another model of the *same* dimension
//! silently mixes two incompatible vector spaces. Both happen as soon as
//! the default model, or `[embeddings].model`, changes between two
//! releases. [`decide`] is the single place that arbitrates: the model
//! recorded in the database wins over the configuration, and when the
//! process cannot follow it, it runs keyword-only instead of touching the
//! vectors. Changing model is an explicit migration, never a side effect
//! of opening the database.
//!
//! Pure logic, no I/O: the caller reads the [`EmbeddingState`] from the
//! store and applies the [`EmbeddingDecision`].

/// `icm_metadata` key holding the name of the model that produced the
/// stored vectors (e.g. `"intfloat/multilingual-e5-base"`).
pub const META_EMBEDDING_MODEL: &str = "embedding_model";

/// What a database says about its own embeddings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingState {
    /// Dimension of the vector index (`icm_metadata.embedding_dims`).
    pub dims: Option<usize>,
    /// Model recorded under [`META_EMBEDDING_MODEL`]. Absent on databases
    /// written before the key existed.
    pub model: Option<String>,
    /// True when at least one memory carries a vector.
    pub has_vectors: bool,
}

/// Outcome of [`decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingDecision {
    /// Use the requested model and open at `dims`. `record_model` asks the
    /// caller to write the requested model name into the database.
    Proceed { dims: usize, record_model: bool },
    /// The database records another model, which this binary can load: use
    /// that one instead of the requested model.
    UseStoredModel { model: String, dims: usize },
    /// The database belongs to a model this process cannot match. Run
    /// without an embedder and open at `stored_dims` so nothing is rewritten.
    KeywordOnly { stored_dims: usize, reason: String },
}

/// Arbitrate between the model a process was asked to use and the one the
/// database was written with.
///
/// - `state`: `None` when there is no database yet.
/// - `requested_model` / `requested_dims`: what the configuration resolves to.
/// - `stored_model_dims`: the dimension this binary resolves for
///   `state.model`, or `None` when it cannot load that model (unknown name,
///   or no model recorded).
///
/// A recorded model is never overruled, whether or not vectors exist yet:
/// `icm embed --migrate` empties the index and records the new model before
/// it re-embeds, and a process that then followed its own configuration
/// would recreate the index under the migration. Model names compare ASCII
/// case-insensitively, the way the embedder resolves them.
///
/// Without a recorded model the configuration is followed whenever that
/// cannot cost a vector: same dimension (the model is then recorded, on
/// the assumption that it produced the vectors), or no vector at all.
pub fn decide(
    state: Option<&EmbeddingState>,
    requested_model: &str,
    requested_dims: usize,
    stored_model_dims: Option<usize>,
) -> EmbeddingDecision {
    let proceed = |record_model| EmbeddingDecision::Proceed {
        dims: requested_dims,
        record_model,
    };
    let Some(state) = state else {
        return proceed(true);
    };
    let Some(stored_dims) = state.dims else {
        return proceed(true);
    };

    let Some(model) = state.model.as_deref() else {
        if stored_dims == requested_dims || !state.has_vectors {
            return proceed(true);
        }
        return EmbeddingDecision::KeywordOnly {
            stored_dims,
            reason: format!(
                "the database holds {stored_dims}-dimension vectors from an unrecorded model, \
                 and the configured model '{requested_model}' produces {requested_dims}"
            ),
        };
    };

    if stored_dims == requested_dims && model.eq_ignore_ascii_case(requested_model) {
        return proceed(false);
    }
    match stored_model_dims {
        Some(dims) if dims == stored_dims => EmbeddingDecision::UseStoredModel {
            model: model.to_string(),
            dims: stored_dims,
        },
        Some(dims) => EmbeddingDecision::KeywordOnly {
            stored_dims,
            reason: format!(
                "the database records model '{model}' at {stored_dims} dimensions, \
                 but this binary resolves it to {dims}"
            ),
        },
        None => EmbeddingDecision::KeywordOnly {
            stored_dims,
            reason: format!(
                "the database records model '{model}' ({stored_dims} dimensions), \
                 which this binary cannot load"
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQ: &str = "model-a";
    const OTHER: &str = "model-b";

    fn state(dims: Option<usize>, model: Option<&str>, has_vectors: bool) -> EmbeddingState {
        EmbeddingState {
            dims,
            model: model.map(str::to_string),
            has_vectors,
        }
    }

    fn proceed(dims: usize, record_model: bool) -> EmbeddingDecision {
        EmbeddingDecision::Proceed { dims, record_model }
    }

    fn is_keyword_only(d: &EmbeddingDecision, dims: usize) -> bool {
        matches!(d, EmbeddingDecision::KeywordOnly { stored_dims, .. } if *stored_dims == dims)
    }

    // Row 1: no database, or a database without a recorded dimension.
    #[test]
    fn no_database_or_no_dims_proceeds_and_records() {
        assert_eq!(decide(None, REQ, 1024, None), proceed(1024, true));
        let s = state(None, None, false);
        assert_eq!(decide(Some(&s), REQ, 1024, None), proceed(1024, true));
        // Even a recorded model and present vectors do not matter without a
        // dimension: there is no index to protect.
        let s = state(None, Some(OTHER), true);
        assert_eq!(decide(Some(&s), REQ, 1024, Some(768)), proceed(1024, true));
    }

    // Row 2: same dimension, no model recorded -> adopt and record.
    #[test]
    fn same_dims_without_model_records_it() {
        for has_vectors in [false, true] {
            let s = state(Some(1024), None, has_vectors);
            assert_eq!(decide(Some(&s), REQ, 1024, None), proceed(1024, true));
        }
    }

    // Row 3: same dimension, same model -> nothing to do.
    #[test]
    fn same_dims_same_model_is_a_no_op() {
        for has_vectors in [false, true] {
            let s = state(Some(1024), Some(REQ), has_vectors);
            assert_eq!(
                decide(Some(&s), REQ, 1024, Some(1024)),
                proceed(1024, false)
            );
            // Names compare the way the embedder resolves them.
            assert_eq!(
                decide(Some(&s), "MODEL-A", 1024, Some(1024)),
                proceed(1024, false)
            );
        }
    }

    // Rows 4 and 5: same dimension, another model recorded. The recorded
    // model wins with or without vectors: an index emptied by a migration
    // in progress has none, and must not be handed back to the old model.
    #[test]
    fn same_dims_other_recorded_model_follows_the_database() {
        for has_vectors in [false, true] {
            let s = state(Some(1024), Some(OTHER), has_vectors);
            assert_eq!(
                decide(Some(&s), REQ, 1024, Some(1024)),
                EmbeddingDecision::UseStoredModel {
                    model: OTHER.to_string(),
                    dims: 1024
                }
            );
            // Stored model unknown to this binary, or resolved to another
            // dimension: never mix vector spaces.
            assert!(is_keyword_only(&decide(Some(&s), REQ, 1024, None), 1024));
            assert!(is_keyword_only(
                &decide(Some(&s), REQ, 1024, Some(384)),
                1024
            ));
        }
    }

    // Row 6: other dimension, no model recorded, no vectors -> nothing to
    // lose and nobody to overrule.
    #[test]
    fn other_dims_without_model_or_vectors_proceeds_at_requested_dims() {
        let s = state(Some(768), None, false);
        assert_eq!(decide(Some(&s), REQ, 1024, None), proceed(1024, true));
    }

    // Row 7: other dimension, model recorded, with or without vectors.
    #[test]
    fn other_dims_with_recorded_model_follows_the_database() {
        for has_vectors in [false, true] {
            let s = state(Some(768), Some(OTHER), has_vectors);
            assert_eq!(
                decide(Some(&s), REQ, 1024, Some(768)),
                EmbeddingDecision::UseStoredModel {
                    model: OTHER.to_string(),
                    dims: 768
                }
            );
            assert!(is_keyword_only(&decide(Some(&s), REQ, 1024, None), 768));
            assert!(is_keyword_only(
                &decide(Some(&s), REQ, 1024, Some(384)),
                768
            ));
            // The recorded name equals the requested one but the dimension
            // does not (a model redefined between releases): still the
            // database's.
            let s = state(Some(768), Some(REQ), has_vectors);
            assert!(is_keyword_only(
                &decide(Some(&s), REQ, 1024, Some(1024)),
                768
            ));
        }
    }

    // A recorded model is never replaced by the configured one: no state
    // carrying a different model yields `record_model: true`.
    #[test]
    fn a_recorded_model_is_never_overruled() {
        for dims in [768, 1024] {
            for has_vectors in [false, true] {
                for stored_model_dims in [None, Some(768), Some(1024)] {
                    let s = state(Some(dims), Some(OTHER), has_vectors);
                    let d = decide(Some(&s), REQ, 1024, stored_model_dims);
                    assert!(
                        !matches!(d, EmbeddingDecision::Proceed { .. }),
                        "{s:?} / {stored_model_dims:?} -> {d:?}"
                    );
                }
            }
        }
    }

    // Row 8: other dimension, no model recorded, vectors present. This is
    // the 768 -> 1024 default-model change on a pre-existing database.
    #[test]
    fn other_dims_without_model_with_vectors_is_keyword_only() {
        let s = state(Some(768), None, true);
        let d = decide(Some(&s), REQ, 1024, None);
        match d {
            EmbeddingDecision::KeywordOnly {
                stored_dims,
                reason,
            } => {
                assert_eq!(stored_dims, 768);
                assert!(
                    reason.contains("768") && reason.contains("1024"),
                    "{reason}"
                );
            }
            other => panic!("expected KeywordOnly, got {other:?}"),
        }
    }
}
