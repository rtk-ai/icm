# Jina v5 Matryoshka Embedding Support — Issue Slices

Generated from plan: `.claude/plans/support-latest-jina-ai-s-fuzzy-crystal.md`
License note: Jina v5 weights are **CC BY-NC 4.0 (non-commercial)**. Local use requires a commercial Jina license for production redistribution.

---

## Track 1 — ICM repo

### S-store — Refactor schema migration to return MigrationStatus [DONE: 36b4030]

**Parent:** PRD plan file

**What to build:**
Add `MigrationStatus { dim_changed, old_dim, new_dim, affected_rows }` to `icm-store/src/lib.rs`. Change `init_db_with_dims` to return `IcmResult<MigrationStatus>` instead of `IcmResult<()>`. Propagate through `SqliteStore::with_dims` and CLI `open_store`. Store remains data-only — no embedder reference.

**Acceptance criteria:**
- [x] `MigrationStatus` exported from `icm-store` public API with no embedder dependency
- [x] `init_db_with_dims` returns `IcmResult<MigrationStatus>`; dim-change path returns all four fields; no-change path returns `default()`
- [x] `open_store` in CLI propagates `MigrationStatus`
- [x] All 125 existing tests in `crates/icm-store` pass
- [x] Zero compile-time reference to `Embedder` inside `icm-store`

**Blocked by:** None
**User stories covered:** US-arch (store stays data-only)

---

### S-1 — Add embedder factory + Jina v5-text-nano backend end-to-end [DONE: 4cf9462, 1995d50, ee977fa]

**Parent:** S-store

**What to build:**
Extend `Embedder` trait with `embed_query`, `embed_document`, `model_name`, `license` default methods. Add `EmbedderBackend` enum to `EmbeddingsConfig`. Implement `JinaV5NanoEmbedder` via `ort` + `tokenizers` + `hf-hub`. Wire asymmetric recall path (`embed_query` in recall, `embed` for store). Add `truncate_and_renorm` Matryoshka helper. Log dim-change warning.

**Acceptance criteria:**
- [x] `EmbedderBackend::Fastembed` and `EmbedderBackend::JinaV5Nano` selectable from config
- [x] First run downloads ONNX from HuggingFace, prints status message
- [x] Trait has `embed_query`, `embed_document`, `model_name`, `license` with defaults; `FastEmbedder` unchanged
- [x] Recall path uses `embed_query`; store path uses `embed`
- [x] Dim-change log message surfaced
- [x] `truncate_and_renorm` unit tests: shape, unit-norm, element-wise correctness, zero-norm edge case
- [x] 291 tests pass

**Blocked by:** S-store
**User stories covered:** US-1, US-2

---

### S-2 — Add Jina v5-text-small (Qwen3) backend

**Parent:** S-1

**What to build:**
Add `EmbedderBackend::JinaV5Small` variant. Implement `JinaV5SmallEmbedder` using `ort` + `tokenizers` + `hf-hub` with Qwen3 architecture: load `jinaai/jina-embeddings-v5-text-small-retrieval` ONNX, apply mean-pool over `last_hidden_state` with attention-mask masking, L2 normalize, Matryoshka truncation. Default dim = 1024; valid truncate dims = {32, 64, 128, 256, 512, 768, 1024}. Reuse `truncate_and_renorm` from S-1.

**Acceptance criteria:**
- [ ] `EmbedderBackend::JinaV5Small` selectable; downloads `jinaai/jina-embeddings-v5-text-small-retrieval` ONNX on first run
- [ ] Mean-pool head applied over `last_hidden_state` with attention-mask masking; output L2-normalized
- [ ] `truncate_and_renorm` from S-1 reused (not duplicated)
- [ ] Round-trip test against an 8K-token document succeeds
- [ ] `cosine(small_full, small_truncated_512) >= 0.93` on canned text (informational, not gated)
- [ ] Dim 1024 (default) stored in schema; migration works from 384→1024 and 768→1024

**Blocked by:** S-1
**User stories covered:** US-4

---

### S-3 — Auto re-embed on dim change (CLI/MCP orchestration)

**Parent:** S-store, S-1

**What to build:**
In `icm-cli/src/main.rs`, after `open_store` returns `MigrationStatus { dim_changed: true }`, call the existing `cmd_embed` batch loop for all NULL-embedding rows. Add `--no-auto-reembed` flag. Wire same check into MCP server startup. Per-row errors log and continue; summary printed always.

**Acceptance criteria:**
- [ ] Prerequisite (from S-store, already done): when the active embedder's `dimensions()` differs from the stored dim in `icm_metadata`, `schema.rs` drops `vec_memories`, sets all `memories.embedding` to NULL, recreates the table, and returns `MigrationStatus { dim_changed: true, affected_rows: N }` — S-3 reads `affected_rows` as the count of rows that need re-embedding
- [ ] CLI detects `MigrationStatus::dim_changed == true` and auto-invokes embed loop
- [ ] MCP server startup path has same check
- [ ] Progress bar identical visual to `cmd_embed`
- [ ] `--no-auto-reembed` flag: skips, warns, exits clean
- [ ] Per-row errors do not abort the pass; summary line always printed
- [ ] Integration test: fresh DB (fastembed 384 dims), 20 memories, switch to jina-v5-nano, run `icm recall`, verify all 20 rows have 768-dim embeddings
- [ ] Test: `--no-auto-reembed` leaves vec_memories empty and prints warning

**Blocked by:** S-store, S-1
**User stories covered:** US-5

---

### S-4 — Enforce asymmetric retrieval paths in v5 backends

**Parent:** S-1, S-2

**What to build:**
Override `embed_query` and `embed_document` in `JinaV5NanoEmbedder` and `JinaV5SmallEmbedder`. `embed_query` prepends `"retrieval.query: "`, `embed_document` prepends `"retrieval.passage: "`, `embed` delegates to `embed_document`. Unit test via `MockEncoder` that captures exact prefix strings.

**Acceptance criteria:**
- [ ] `JinaV5NanoEmbedder::embed_query` passes `"retrieval.query: {text}"` to encoder
- [ ] `JinaV5NanoEmbedder::embed_document` passes `"retrieval.passage: {text}"` to encoder
- [ ] `JinaV5NanoEmbedder::embed` delegates to `embed_document`
- [ ] Same three impl points in `JinaV5SmallEmbedder`
- [ ] `FastEmbedder`: no changes
- [ ] Unit test via `MockEncoder`: captures exact string passed; asserts prefix for query and passage
- [ ] Existing round-trip recall test from S-1 still passes

**Blocked by:** S-1, S-2
**User stories covered:** US-3

---

### S-5 — Docs / UX / license disclosure

**Parent:** S-1..S-4

**What to build:**
Update `README.md` with "Embedder backends" section. Add license warning comment in `config/default.toml`. Make `icm config show` print active embedder type + license tag. Make `icm recall` output header include model name. Add CHANGELOG entry.

**Acceptance criteria:**
- [ ] README has clear non-commercial warning for Jina v5
- [ ] `config/default.toml` has license comment
- [ ] `icm config show` prints `embedder: jina-v5-nano (CC-BY-NC-4.0, non-commercial)` when active
- [ ] `icm recall` output header includes `model: jina-v5-nano`
- [ ] CHANGELOG.md entry under Unreleased
- [ ] Snapshot test of `icm config show` output covers new fields

**Blocked by:** S-1, S-2, S-4
**User stories covered:** US-7

---

## Track 2 — upstream Anush008/fastembed-rs

Note: the actual Rust crate `fastembed` consumed by ICM is published from `Anush008/fastembed-rs`, not `qdrant/fastembed` (which is the Python upstream). PRs target the Rust repo. Issue qdrant/fastembed#607 was filed against the Python upstream; the Rust port has no equivalent issue at the time of writing.

### F-1 — Register `jina-embeddings-v5-text-nano-retrieval` as built-in fastembed model [PATCH READY: docs/issues/jina-v5-fastembed-rs-F1.patch]

**Parent:** qdrant/fastembed#607 (Python upstream)

**What to build:**
Add `EmbeddingModel::JinaEmbeddingsV5TextNano` to fastembed's enum in `src/models/text_embedding.rs`. Add `ModelInfo` entry in `init_models_map()` with HF path `jinaai/jina-embeddings-v5-text-nano-retrieval`, dim = 768, max tokens = 8192, license = "CC BY-NC 4.0". Add pooling mode entry. Add snapshot test.

**Draft patch:**
```rust
// In EmbeddingModel enum (~line 4490):
/// jinaai/jina-embeddings-v5-text-nano-retrieval
JinaEmbeddingsV5TextNano,

// In init_models_map() (~line 4930):
ModelInfo {
    model: EmbeddingModel::JinaEmbeddingsV5TextNano,
    dim: 768,
    description: String::from("Jina embeddings v5 text nano (CC BY-NC 4.0)"),
    model_code: String::from("jinaai/jina-embeddings-v5-text-nano-retrieval"),
    model_file: String::from("onnx/model.onnx"),
    additional_files: Vec::new(),
    output_key: None,
},

// In get_quantization_mode() (~line 6230):
EmbeddingModel::JinaEmbeddingsV5TextNano => Some(Pooling::Mean),

// In verify_embeddings() snapshot test (~line 7850):
EmbeddingModel::JinaEmbeddingsV5TextNano => [a, b, c, d],  // run test to capture values
```

**Acceptance criteria:**
- [x] Enum variant added with rustdoc covering license + Matryoshka dim list
- [x] `ModelInfo` entry registered in `init_models_map()` (dim=768, model_file=`onnx/model.onnx`)
- [x] Pooling registered as `Pooling::Mean` in `get_default_pooling_method()`
- [x] `cargo build` clean against Anush008/fastembed-rs main (verified 2026-04-29)
- [ ] Snapshot test entry in `tests/text-embeddings.rs` — DEFERRED. The catch-all `_ => panic!()` arm signals to CI / maintainer that real expected sums must be captured by running the test once with `ORT_LIB_LOCATION` set. Snapshot capture requires ~50MB ONNX Runtime + ~250MB model download; out of scope for the registration-only patch.
- [ ] PR opened against Anush008/fastembed-rs (HITL — user decides when to fork+push the prepared patch at `docs/issues/jina-v5-fastembed-rs-F1.patch`)

**Apply via:** `git -C <fastembed-rs-fork> am < docs/issues/jina-v5-fastembed-rs-F1.patch`

**Blocked by:** None (patch applies cleanly to Anush008/fastembed-rs main)
**User stories covered:** US-8

---

### F-2 — Register `jina-embeddings-v5-text-small-retrieval` [BLOCKED upstream]

**Parent:** F-1

**Status update (2026-04-29):** v5-text-small is Qwen3-decoder-based with mean-pool head. Anush008/fastembed-rs main does NOT support decoder-style ONNX exports (no `Pooling::LastToken`, no `position_ids` injection, no KV-cache injection). Closed PR #236 (`feat: decoder/quantized model support`) attempted this work but was rejected. F-2 cannot be a registration-only patch like F-1; it requires the architectural prerequisites of #236 to land first. Current path: ICM uses its own ort+tokenizers integration in `icm-core` (already DONE in S-2), which is sufficient for the local-only consumer use case. Track upstream re-attempt separately.

**What to build (when unblocked):**
Same pattern as F-1 but for small (Qwen3-based). Coordinate with fastembed maintainers on whether a new `ModelArchitecture::Qwen3Pooled` variant is needed. If yes, that is a separate commit before F-2.

**Acceptance criteria:**
- [ ] Variant + ModelInfo + pooling + snapshot test
- [ ] Test produces 1024-dim vector for "hello world"
- [ ] If Qwen3 arch needed: clean separate commit

**Blocked by:** F-1
**User stories covered:** US-9

---

### F-3 (optional) — Add `truncate_dim` parameter to fastembed `InitOptions`

**Parent:** F-1

**What to build:**
Add `InitOptions::truncate_dim: Option<usize>` that slices + L2-renormalizes output. No-op when `None` or model doesn't support it.

**Acceptance criteria:**
- [ ] `InitOptions::truncate_dim: Option<usize>`
- [ ] When set: output sliced + L2-renormalized
- [ ] No-op for non-Matryoshka models (or warning)

**Blocked by:** F-1
**User stories covered:** US-10

---

## To create GitHub issues

Run in dependency order (Track 1 first, then Track 2):

```bash
# Track 1 — ICM repo (skip S-store and S-1, already done)
gh issue create --title "feat: Jina v5-text-small (Qwen3) embedder backend" --body "$(cat docs/issues/jina-v5-slices.md | sed -n '/### S-2/,/### S-3/p')"
gh issue create --title "feat: Auto re-embed on embedder dim change" --body "..."
gh issue create --title "feat: Asymmetric retrieval prefixes for Jina v5 backends" --body "..."
gh issue create --title "feat: Docs/UX — license disclosure and embedder surfacing" --body "..."

# Track 2 — fastembed fork
gh issue create --title "feat: register jina-embeddings-v5-text-nano as built-in model" --repo qdrant/fastembed --body "..."
```
