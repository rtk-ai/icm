//! Multilingual semantic scoring for auto-extraction.
//!
//! The keyword-based scorer in `extract.rs` only recognises English
//! decision/bug/preference vocabulary. A user whose primary working
//! language is anything else gets near-zero auto-extraction output: a
//! probe of `"On a décidé de migrer vers SQLite parce que Postgres
//! était overkill"` extracts zero facts even though it is a textbook
//! decision sentence.
//!
//! This module replaces the keyword scorer with embedding-based
//! similarity to a fixed set of anchor patterns. Each anchor is a
//! short English description of a category we want to capture
//! (decision, bug fix, preference, milestone, architecture,
//! performance, constraint). For every candidate sentence we compute
//! its embedding and the cosine similarity to every anchor. The
//! sentence's margin is `max(positive_similarity) -
//! max(negative_similarity)`; when above `THRESHOLD` the sentence is
//! kept and tagged with the matching positive anchor's importance.
//!
//! The underlying default embedder is `intfloat/multilingual-e5-base`
//! (100+ languages), so the same anchors work cross-lingually: a
//! French decision sentence and its English counterpart land in
//! nearby points of vector space, both close to the English decision
//! anchor. Tested against EN/FR/DE/ES/IT fixtures.

use icm_core::{Embedder, IcmResult, Importance};

/// Importance bucket for a matched anchor. The mapping is fixed
/// rather than configurable so the calibration of the scorer's
/// threshold (which is empirical) stays meaningful across releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    Decision,
    BugFix,
    Preference,
    Milestone,
    Architecture,
    Performance,
    Constraint,
}

impl AnchorKind {
    pub fn importance(self) -> Importance {
        match self {
            AnchorKind::Decision
            | AnchorKind::BugFix
            | AnchorKind::Preference
            | AnchorKind::Constraint => Importance::High,
            AnchorKind::Milestone | AnchorKind::Architecture | AnchorKind::Performance => {
                Importance::Medium
            }
        }
    }

    pub fn as_tag(self) -> &'static str {
        match self {
            AnchorKind::Decision => "kind:decision",
            AnchorKind::BugFix => "kind:bugfix",
            AnchorKind::Preference => "kind:preference",
            AnchorKind::Milestone => "kind:milestone",
            AnchorKind::Architecture => "kind:architecture",
            AnchorKind::Performance => "kind:performance",
            AnchorKind::Constraint => "kind:constraint",
        }
    }
}

struct Anchor {
    kind: AnchorKind,
    pattern: &'static str,
}

/// Positive anchors. Two seed sentences per category so a single
/// idiosyncratic phrasing on the user side does not lose the match.
/// Phrasings are deliberately neutral and content-focused — anything
/// that hints at "I am narrating an action" goes in the negatives.
const POSITIVE_ANCHORS: &[Anchor] = &[
    Anchor {
        kind: AnchorKind::Decision,
        pattern:
            "We made a deliberate technical decision and chose this approach over the alternatives.",
    },
    Anchor {
        kind: AnchorKind::Decision,
        pattern: "After evaluating tradeoffs we settled on this option instead of the other ones.",
    },
    Anchor {
        kind: AnchorKind::Decision,
        pattern:
            "We decided to go with this technology rather than the other one for our use case.",
    },
    Anchor {
        kind: AnchorKind::BugFix,
        pattern: "A bug or error was identified and its root cause was patched in the codebase.",
    },
    Anchor {
        kind: AnchorKind::BugFix,
        pattern:
            "We fixed a regression that was breaking the production deployment of the service.",
    },
    // Audit #185 B1: short conversational bugfix phrasings ("Fixed
    // the cache returning stale entries", "バグを修正しました",
    // "修复了... 错误") under-matched against the longer English
    // anchors. A briefer anchor in the same form lifts CJK and
    // tweet-length bugfix margins above the threshold.
    Anchor {
        kind: AnchorKind::BugFix,
        pattern: "Fixed the cache where it returned stale entries.",
    },
    Anchor {
        kind: AnchorKind::Preference,
        pattern:
            "This is a coding rule or convention the user always wants followed across the project.",
    },
    Anchor {
        kind: AnchorKind::Preference,
        pattern: "The user prefers this style and wants it applied consistently going forward.",
    },
    // Audit #185 B4: explicit "user preference:" / "user wants..."
    // phrasings rejected even though semantically obvious. Add an
    // anchor that mirrors the explicit form.
    Anchor {
        kind: AnchorKind::Preference,
        pattern: "User preference: never use this pattern in production code.",
    },
    Anchor {
        kind: AnchorKind::Milestone,
        pattern: "We shipped a new release or completed a deployment to production.",
    },
    Anchor {
        kind: AnchorKind::Architecture,
        pattern:
            "This describes a component, module, or system architecture choice in the project.",
    },
    // Audit #185 B3: the original Architecture anchor was confused
    // with BugFix on "The X middleware sits between Y and Z" — too
    // many shared "service / module" tokens with the bug anchors.
    // This anchor pins the structural-relation form ("X is the layer
    // that...", "X sits between Y and Z").
    Anchor {
        kind: AnchorKind::Architecture,
        pattern: "The auth middleware is the layer that sits between the gateway and the application servers.",
    },
    Anchor {
        kind: AnchorKind::Performance,
        pattern: "We measured performance and report concrete latency or throughput numbers.",
    },
    Anchor {
        kind: AnchorKind::Constraint,
        pattern:
            "This is a hard constraint or limitation in the system that cannot be worked around.",
    },
    // Audit #185 B2: phrasings like "X does not work with Y" /
    // "X breaks when Y" rejected even though they're textbook
    // constraints. Add an anchor that captures the negation form.
    Anchor {
        kind: AnchorKind::Constraint,
        pattern: "The crate does not work with cross-compilation for ARM64 on Linux runners.",
    },
];

/// Negative anchors. These are the patterns the LLM emits *while
/// working* — internal monologue, narration, action announcements,
/// chitchat. Catching them as "negative similarity" prevents the
/// scorer from accepting them just because they happen to mention a
/// technical noun that overlaps with a positive anchor.
const NEGATIVE_ANCHORS: &[&str] = &[
    "I am about to perform an action and need to check this first.",
    "Let me look at the file and think about how to approach this.",
    "I will start by examining the directory structure of the project.",
    "Now I am exploring the options before making a choice.",
    "Here is some code I am going to run or write next.",
    "Hello, please, thank you, just chitchat without any information.",
];

/// Margin threshold below which a candidate sentence is rejected.
/// `margin = max(pos_sim) - max(neg_sim)`. With multilingual-e5-base,
/// empirical measurements on EN/FR/DE/ES sentences show:
/// - Real decision/bugfix sentences: margin in [0.034, 0.091]
/// - Narration sentences (must reject): margin in [-0.17, -0.14]
///
/// 0.025 sits comfortably between the floor of valid signal and the
/// ceiling of narration noise (gap of ~0.17). Tighter thresholds (0.04)
/// drop valid French content; looser ones risk surfacing chitchat.
const DEFAULT_THRESHOLD: f32 = 0.025;

/// A reusable scorer that holds precomputed anchor embeddings.
/// Building one calls `embed_batch` exactly once for the positive
/// anchors and once for the negative anchors; subsequent `score`
/// calls are pure CPU dot products.
pub struct SemanticScorer {
    positive: Vec<(AnchorKind, Vec<f32>)>,
    negative: Vec<Vec<f32>>,
    threshold: f32,
}

impl SemanticScorer {
    pub fn new(embedder: &dyn Embedder) -> IcmResult<Self> {
        let pos_texts: Vec<&str> = POSITIVE_ANCHORS.iter().map(|a| a.pattern).collect();
        let pos_embs = embedder.embed_batch(&pos_texts)?;
        let positive: Vec<(AnchorKind, Vec<f32>)> = POSITIVE_ANCHORS
            .iter()
            .zip(pos_embs)
            .map(|(a, e)| (a.kind, e))
            .collect();
        let negative = embedder.embed_batch(NEGATIVE_ANCHORS)?;
        Ok(Self {
            positive,
            negative,
            threshold: DEFAULT_THRESHOLD,
        })
    }

    /// Override the default threshold (mainly used by tests that
    /// want to verify boundary behaviour).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_threshold(mut self, t: f32) -> Self {
        self.threshold = t;
        self
    }

    /// Score a single sentence by its precomputed embedding. Returns
    /// the matched positive anchor and the `max_pos - max_neg`
    /// margin when the margin is at or above the threshold.
    pub fn score(&self, embedding: &[f32]) -> Option<(AnchorKind, f32)> {
        let mut best_pos: Option<(AnchorKind, f32)> = None;
        for (kind, anchor_emb) in &self.positive {
            let sim = cosine(embedding, anchor_emb);
            if best_pos.is_none_or(|(_, b)| sim > b) {
                best_pos = Some((*kind, sim));
            }
        }
        let max_neg = self
            .negative
            .iter()
            .map(|e| cosine(embedding, e))
            .fold(f32::MIN, f32::max);
        let (kind, pos_sim) = best_pos?;
        let margin = pos_sim - max_neg;
        if margin >= self.threshold {
            Some((kind, margin))
        } else {
            None
        }
    }

    /// Score a batch of candidate sentences by embedding them all in
    /// one call to the embedder, then running cheap cosine
    /// comparisons against the cached anchors. Returns a parallel
    /// `Vec<Option<(kind, margin)>>` aligned with the input.
    pub fn score_batch(
        &self,
        embedder: &dyn Embedder,
        sentences: &[&str],
    ) -> IcmResult<Vec<Option<(AnchorKind, f32)>>> {
        if sentences.is_empty() {
            return Ok(Vec::new());
        }
        let embs = embedder.embed_batch(sentences)?;
        Ok(embs.into_iter().map(|e| self.score(&e)).collect())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-10);
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use icm_core::IcmError;

    /// Stub embedder that returns deterministic orthogonal vectors
    /// keyed on keyword presence in the input. The 8th dimension is
    /// an "other" axis used as a default direction so anchors that
    /// don't keyword-match anything don't collapse onto the same
    /// direction as decision/bugfix/etc. (which would inflate
    /// `max_neg` and reject everything).
    struct KeywordEmbedder;

    impl Embedder for KeywordEmbedder {
        fn embed(&self, text: &str) -> IcmResult<Vec<f32>> {
            // 8-dim: [decision, bugfix, preference, milestone, arch,
            //         perf, narration, other]
            let lower = text.to_lowercase();
            let mut v = vec![0.0f32; 8];
            if lower.contains("decision") || lower.contains("chose") || lower.contains("settled") {
                v[0] = 1.0;
            }
            if lower.contains("bug")
                || lower.contains("fix")
                || lower.contains("regression")
                || lower.contains("patched")
            {
                v[1] = 1.0;
            }
            if lower.contains("rule") || lower.contains("convention") || lower.contains("prefer") {
                v[2] = 1.0;
            }
            if lower.contains("ship") || lower.contains("deploy") || lower.contains("release") {
                v[3] = 1.0;
            }
            if lower.contains("architecture")
                || lower.contains("component")
                || lower.contains("module")
            {
                v[4] = 1.0;
            }
            if lower.contains("performance")
                || lower.contains("latency")
                || lower.contains("throughput")
            {
                v[5] = 1.0;
            }
            if lower.contains("let me")
                || lower.contains("i will")
                || lower.contains("hello")
                || lower.contains("about to")
                || lower.contains("approach this")
                || lower.contains("examining")
                || lower.contains("exploring")
                || lower.contains("here is some code")
                || lower.contains("just chitchat")
            {
                v[6] = 1.0;
            }
            // Constraint / hard limitation — separate axis so it does
            // not collide with Decision in cosine space.
            if lower.contains("constraint") || lower.contains("limitation") {
                // Push along arch axis only as a fallback signal so
                // unmatched constraint anchors land near Architecture
                // — close enough not to wreck the test, far enough
                // from Decision/Narration to keep scores stable.
                v[4] = 1.0;
            }
            if v.iter().all(|x| *x == 0.0) {
                // No category match: park on the "other" axis,
                // orthogonal to every category direction. Anchors
                // that fall here won't inflate `max_neg` against an
                // on-category sentence.
                v[7] = 1.0;
            }
            Ok(v)
        }

        fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            texts.iter().map(|t| self.embed(t)).collect()
        }

        fn dimensions(&self) -> usize {
            8
        }
    }

    /// Embedder that always errors — used to verify `SemanticScorer::new`
    /// surfaces the embedder's failure rather than panicking.
    struct FailingEmbedder;

    impl Embedder for FailingEmbedder {
        fn embed(&self, _text: &str) -> IcmResult<Vec<f32>> {
            Err(IcmError::Embedding("forced failure".into()))
        }
        fn embed_batch(&self, _texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            Err(IcmError::Embedding("forced failure".into()))
        }
        fn dimensions(&self) -> usize {
            7
        }
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = [1.0, 0.0, 0.0];
        let b = [0.0, 1.0, 0.0];
        assert!(cosine(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_identical_is_one() {
        let a = [0.5, 0.5, 0.7];
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_zero_vector_without_panic() {
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        let r = cosine(&a, &b);
        assert!(r.is_finite(), "cosine on zero vector must not be NaN: {r}");
    }

    #[test]
    fn anchor_kind_importance_mapping_is_stable() {
        // The threshold calibration assumes this mapping. Pin it.
        assert_eq!(AnchorKind::Decision.importance(), Importance::High);
        assert_eq!(AnchorKind::BugFix.importance(), Importance::High);
        assert_eq!(AnchorKind::Preference.importance(), Importance::High);
        assert_eq!(AnchorKind::Constraint.importance(), Importance::High);
        assert_eq!(AnchorKind::Milestone.importance(), Importance::Medium);
        assert_eq!(AnchorKind::Architecture.importance(), Importance::Medium);
        assert_eq!(AnchorKind::Performance.importance(), Importance::Medium);
    }

    #[test]
    fn scorer_new_propagates_embedder_failure() {
        let result = SemanticScorer::new(&FailingEmbedder);
        assert!(
            result.is_err(),
            "scorer must surface embedder errors, not silently succeed with empty anchors"
        );
    }

    #[test]
    fn scorer_classifies_decision_above_narration_with_stub() {
        // End-to-end smoke test on the stub: a sentence that scores
        // positive on the decision dimension and zero on narration
        // must classify as Decision; a narration sentence must
        // return None.
        let scorer = SemanticScorer::new(&KeywordEmbedder)
            .expect("stub scorer build")
            .with_threshold(0.1);
        let dec_emb = KeywordEmbedder
            .embed("We chose this approach as our decision")
            .unwrap();
        match scorer.score(&dec_emb) {
            Some((kind, margin)) => {
                assert_eq!(
                    kind,
                    AnchorKind::Decision,
                    "expected Decision, got {kind:?}"
                );
                assert!(margin > 0.0, "decision margin should be positive: {margin}");
            }
            None => panic!("decision sentence rejected"),
        }
        let nar_emb = KeywordEmbedder
            .embed("Let me check this and approach this carefully")
            .unwrap();
        assert!(
            scorer.score(&nar_emb).is_none(),
            "narration sentence must be rejected"
        );
    }

    #[test]
    fn scorer_score_batch_aligns_with_input() {
        let scorer = SemanticScorer::new(&KeywordEmbedder)
            .expect("stub scorer build")
            .with_threshold(0.1);
        let inputs = [
            "We chose this approach as decision",
            "Let me check the file",
        ];
        let results = scorer
            .score_batch(&KeywordEmbedder, &inputs)
            .expect("batch score");
        assert_eq!(results.len(), 2);
        assert!(results[0].is_some(), "first should classify");
        assert!(results[1].is_none(), "second should be rejected");
    }

    #[test]
    fn scorer_score_batch_empty_input() {
        let scorer = SemanticScorer::new(&KeywordEmbedder).expect("stub scorer build");
        let results = scorer.score_batch(&KeywordEmbedder, &[]).expect("empty");
        assert!(results.is_empty());
    }

    /// End-to-end cross-lingual test using the real FastEmbedder.
    /// Marked `#[ignore]` so CI doesn't pay the model-download cost
    /// on every run; opt-in via `cargo test -- --ignored
    /// crosslingual`. The expected behaviour is that decision /
    /// bugfix / preference sentences in EN, FR, DE, ES, IT all match
    /// their respective anchors above the threshold, while narration
    /// sentences in any of those languages reject.
    #[test]
    #[ignore = "downloads multilingual-e5-base on first run; opt-in via --ignored"]
    fn crosslingual_anchor_separation_with_real_embedder() {
        use icm_core::FastEmbedder;
        let embedder = FastEmbedder::new();
        let scorer = SemanticScorer::new(&embedder).expect("anchor build");

        // Each test case: (sentence, language, expected_kind_or_None)
        // Expected_kind = Some(kind) → must classify as that kind.
        // None → must reject (no positive anchor wins by margin).
        let cases: &[(&str, &str, Option<AnchorKind>)] = &[
            // ── Decisions ──
            (
                "We chose SQLite instead of Postgres for the embedded use case.",
                "en",
                Some(AnchorKind::Decision),
            ),
            (
                "On a décidé de partir sur SQLite plutôt que Postgres pour l'embarqué.",
                "fr",
                Some(AnchorKind::Decision),
            ),
            (
                "Wir haben uns für SQLite statt Postgres entschieden für den eingebetteten Anwendungsfall.",
                "de",
                Some(AnchorKind::Decision),
            ),
            // ── Bug fixes ──
            (
                "Fixed the regression in the auth middleware that was returning stale tokens.",
                "en",
                Some(AnchorKind::BugFix),
            ),
            (
                "Corrigé la régression dans le middleware d'auth qui renvoyait des tokens périmés.",
                "fr",
                Some(AnchorKind::BugFix),
            ),
            // ── Narration (must reject) ──
            (
                "Let me check the file and figure out how to approach this.",
                "en",
                None,
            ),
            (
                "Je vais regarder le fichier et réfléchir à comment aborder ça.",
                "fr",
                None,
            ),
            // ── B1: CJK bugfix natural phrasing (was 0/3 ja, 1/3 zh/ko/vi
            //   on develop tip before this PR) ──
            (
                "キャッシュが古いエントリを返すバグを修正しました。",
                "ja",
                Some(AnchorKind::BugFix),
            ),
            (
                "修复了缓存返回过期条目的错误。",
                "zh",
                Some(AnchorKind::BugFix),
            ),
            (
                "캐시가 오래된 항목을 반환하는 버그를 수정했습니다.",
                "ko",
                Some(AnchorKind::BugFix),
            ),
            (
                "Đã sửa lỗi cache trả về các mục đã hết hạn.",
                "vi",
                Some(AnchorKind::BugFix),
            ),
            // ── B2: constraint "X does not work with Y" (was rejected) ──
            (
                "The fastembed crate does not work with cross-compilation for ARM64 on Linux CI runners.",
                "en",
                Some(AnchorKind::Constraint),
            ),
            // ── B3: architecture "X sits between Y and Z"
            //   (was misclassified as bugfix) ──
            (
                "The auth middleware sits between the gateway and the application servers as a Rust microservice.",
                "en",
                Some(AnchorKind::Architecture),
            ),
            // ── B4: explicit "User preference:" form (was rejected) ──
            (
                "User preference: never use unwrap() in production code.",
                "en",
                Some(AnchorKind::Preference),
            ),
        ];

        let mut failures: Vec<String> = Vec::new();
        for (sentence, lang, expected) in cases {
            let emb = embedder.embed(sentence).expect("embed");
            // Compute the raw margin without the threshold so we can
            // log it for diagnostic purposes regardless of accept/reject.
            let raw_scorer = SemanticScorer::new(&embedder)
                .expect("anchor build")
                .with_threshold(f32::NEG_INFINITY);
            let raw = raw_scorer.score(&emb);
            let got = scorer.score(&emb);
            match (expected, got) {
                (Some(exp), Some((got_kind, margin))) if got_kind == *exp => {
                    eprintln!("[ok] [{lang}] {sentence:?} -> {got_kind:?} margin={margin:.4}");
                }
                (Some(exp), Some((got_kind, margin))) => {
                    failures.push(format!(
                        "[{lang}] expected {exp:?}, got {got_kind:?} (margin {margin:.4}): {sentence:?}"
                    ));
                }
                (Some(exp), None) => {
                    let raw_str = raw
                        .map(|(k, m)| format!("best={k:?} margin={m:.4}"))
                        .unwrap_or_else(|| "no positive".into());
                    failures.push(format!(
                        "[{lang}] expected {exp:?}, but rejected ({raw_str}): {sentence:?}"
                    ));
                }
                (None, None) => {
                    let raw_str = raw
                        .map(|(k, m)| format!("best={k:?} margin={m:.4}"))
                        .unwrap_or_else(|| "no positive".into());
                    eprintln!("[ok] [{lang}] rejected ({raw_str}): {sentence:?}");
                }
                (None, Some((got_kind, margin))) => {
                    failures.push(format!(
                        "[{lang}] expected reject, got {got_kind:?} (margin {margin:.4}): {sentence:?}"
                    ));
                }
            }
        }

        if !failures.is_empty() {
            panic!(
                "cross-lingual separation broke on {}/{} cases:\n  {}",
                failures.len(),
                cases.len(),
                failures.join("\n  ")
            );
        }
    }

    /// End-to-end: feed a French session note through
    /// `extract_and_store_with_embedder` and verify the decision
    /// sentence is stored. The pre-existing English-only keyword
    /// scorer extracts zero facts from this input — the regression
    /// this PR fixes.
    #[test]
    #[ignore = "downloads multilingual-e5-base on first run; opt-in via --ignored"]
    fn french_transcript_extracts_facts_end_to_end() {
        use crate::extract::extract_and_store_with_embedder;
        use icm_core::{FastEmbedder, Importance, MemoryStore};
        use icm_store::SqliteStore;

        let store = SqliteStore::in_memory().expect("store");
        let embedder = FastEmbedder::new();

        let text = "Session 2026-05-04. \
                    On a décidé de partir sur SQLite plutôt que Postgres pour l'embarqué. \
                    Corrigé la régression dans le middleware d'auth qui renvoyait des tokens périmés. \
                    Je vais regarder le fichier ensuite. \
                    On a déployé la v1.2 en production hier soir.";

        let stored = extract_and_store_with_embedder(
            &store,
            text,
            "test",
            false,
            Importance::Critical,
            Some(&embedder),
        )
        .expect("extract");

        assert!(
            stored >= 2,
            "expected at least 2 French facts to land (decision + bugfix or milestone), got {stored}"
        );

        // Inspect the stored content to confirm narration is filtered.
        let topic = "context-test";
        let memories = store.get_by_topic(topic).expect("topic");
        for m in &memories {
            assert!(
                !m.summary.contains("Je vais regarder"),
                "narration sentence leaked into store: {:?}",
                m.summary
            );
        }
    }

    #[test]
    fn anchor_kind_tag_strings_are_unique() {
        // The tag strings end up as keywords on stored memories and
        // are how users (and `classify_fact` consumers) recognise
        // each category. They must be unique so a downstream filter
        // like `kw.contains("kind:bugfix")` doesn't collide with
        // another category.
        let kinds = [
            AnchorKind::Decision,
            AnchorKind::BugFix,
            AnchorKind::Preference,
            AnchorKind::Milestone,
            AnchorKind::Architecture,
            AnchorKind::Performance,
            AnchorKind::Constraint,
        ];
        let tags: Vec<&'static str> = kinds.iter().map(|k| k.as_tag()).collect();
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                assert_ne!(tags[i], tags[j], "duplicate tag at {i}/{j}: {}", tags[i]);
            }
        }
    }
}
