//! Jina v5-text-small (Qwen3-based) embedder — local ONNX inference via `ort` + `tokenizers`.
//!
//! This backend is gated behind the `jina-v5` Cargo feature and is OFF by
//! default. License: CC-BY-NC-4.0 (non-commercial). See:
//! <https://huggingface.co/jinaai/jina-embeddings-v5-text-small-retrieval>

#[cfg(feature = "jina-v5")]
mod inner {
    use hf_hub::api::sync::Api;
    use ndarray::Array2;
    use ort::session::{builder::GraphOptimizationLevel, Session};
    use tokenizers::Tokenizer;

    use crate::embedder::Embedder;
    use crate::error::{IcmError, IcmResult};
    // Reuse the Matryoshka truncation utility from the nano module — do NOT duplicate.
    use crate::jina_v5_nano::truncate_and_renorm;

    const HF_MODEL_ID: &str = "jinaai/jina-embeddings-v5-text-small-retrieval";
    const DEFAULT_DIM: usize = 1024;
    const VALID_DIMS: &[usize] = &[32, 64, 128, 256, 512, 768, 1024];

    pub struct JinaV5SmallEmbedder {
        session: Session,
        tokenizer: Tokenizer,
        truncate_dim: usize,
    }

    impl JinaV5SmallEmbedder {
        pub fn new(truncate_dim: Option<usize>) -> IcmResult<Self> {
            let dim = match truncate_dim {
                Some(d) if VALID_DIMS.contains(&d) => d,
                Some(d) => {
                    return Err(IcmError::Embedding(format!(
                        "invalid truncate_dim {d} for jina-v5-small; valid: {VALID_DIMS:?}"
                    )));
                }
                None => DEFAULT_DIM,
            };

            let api = Api::new().map_err(|e| IcmError::Embedding(e.to_string()))?;
            let repo = api.model(HF_MODEL_ID.to_string());

            eprintln!(
                "Loading jina-v5-text-small-retrieval (downloads on first run, cached thereafter)..."
            );
            let onnx_path = repo
                .get("onnx/model.onnx")
                .map_err(|e| IcmError::Embedding(format!("download ONNX: {e}")))?;
            let tokenizer_path = repo
                .get("tokenizer.json")
                .map_err(|e| IcmError::Embedding(format!("download tokenizer: {e}")))?;

            let intra_threads = std::thread::available_parallelism()
                .map(|n| n.get().min(4))
                .unwrap_or(1);

            let session = Session::builder()
                .map_err(|e| IcmError::Embedding(format!("ort session builder: {e}")))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| IcmError::Embedding(format!("ort opt level: {e}")))?
                .with_intra_threads(intra_threads)
                .map_err(|e| IcmError::Embedding(format!("ort threads: {e}")))?
                .commit_from_file(&onnx_path)
                .map_err(|e| {
                    IcmError::Embedding(format!("load ONNX from {onnx_path:?}: {e}"))
                })?;

            let tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| IcmError::Embedding(e.to_string()))?;

            Ok(Self {
                session,
                tokenizer,
                truncate_dim: dim,
            })
        }

        fn encode_texts(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }

            let encodings = self
                .tokenizer
                .encode_batch(texts.iter().map(|s| s.to_string()).collect(), true)
                .map_err(|e| IcmError::Embedding(e.to_string()))?;

            let batch_size = texts.len();
            let seq_len = encodings
                .iter()
                .map(|e| e.get_ids().len())
                .max()
                .unwrap_or(0);

            if seq_len == 0 {
                return Err(IcmError::Embedding("empty token sequence".into()));
            }

            let mut input_ids = Array2::<i64>::zeros((batch_size, seq_len));
            let mut attention_mask = Array2::<i64>::zeros((batch_size, seq_len));

            for (i, enc) in encodings.iter().enumerate() {
                for (j, &id) in enc.get_ids().iter().enumerate() {
                    input_ids[[i, j]] = id as i64;
                }
                for (j, &m) in enc.get_attention_mask().iter().enumerate() {
                    attention_mask[[i, j]] = m as i64;
                }
            }

            // ort 2.0.0-rc.9: `inputs!` returns `Result<Vec<(Cow<str>, ...)>>`.
            let session_inputs = ort::inputs! {
                "input_ids" => input_ids.view(),
                "attention_mask" => attention_mask.view(),
            }
            .map_err(|e| IcmError::Embedding(format!("ort inputs!: {e}")))?;

            let outputs = self
                .session
                .run(session_inputs)
                .map_err(|e| IcmError::Embedding(format!("ort run: {e}")))?;

            // HF transformer ONNX models commonly emit `last_hidden_state`;
            // some BERT-style exports use `token_embeddings`. Prefer the
            // canonical name and fall back gracefully.
            // Note: Qwen3-based architecture (jina-v5-small) follows the same
            // ONNX export convention as EuroBERT (jina-v5-nano) from our
            // inference perspective — both output `last_hidden_state`.
            let hidden = outputs
                .get("last_hidden_state")
                .or_else(|| outputs.get("token_embeddings"))
                .ok_or_else(|| {
                    IcmError::Embedding("ONNX output key not found".into())
                })?;

            let hidden_view = hidden
                .try_extract_tensor::<f32>()
                .map_err(|e| IcmError::Embedding(format!("extract tensor: {e}")))?;

            let shape = hidden_view.shape();
            if shape.len() != 3 {
                return Err(IcmError::Embedding(format!(
                    "expected last_hidden_state rank 3, got shape {shape:?}"
                )));
            }
            let hidden_dim = shape[2];

            let mut embeddings = Vec::with_capacity(batch_size);
            for i in 0..batch_size {
                let mask = attention_mask.row(i);
                let mut pooled = vec![0f32; hidden_dim];
                let mut count = 0usize;

                for j in 0..seq_len {
                    if mask[j] == 1 {
                        for k in 0..hidden_dim {
                            pooled[k] += hidden_view[[i, j, k]];
                        }
                        count += 1;
                    }
                }

                if count > 0 {
                    let inv = 1.0_f32 / count as f32;
                    for v in &mut pooled {
                        *v *= inv;
                    }
                }

                // L2-normalize the pooled vector.
                let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 1e-8 {
                    let inv = 1.0_f32 / norm;
                    for v in &mut pooled {
                        *v *= inv;
                    }
                }

                // Matryoshka truncation + re-normalization.
                let out = truncate_and_renorm(&pooled, self.truncate_dim);
                embeddings.push(out);
            }

            Ok(embeddings)
        }
    }

    // NOTE: `embed_query` and `embed_document` are intentionally not overridden here.
    // Jina v5 retrieval models use asymmetric instruction prefixes in production:
    //   query: "Represent this sentence for searching relevant passages: {text}"
    //   document: no prefix
    // This prefix injection is implemented in slice S-4. The symmetric fallback used
    // here (inherited default: both call `embed`) is functionally correct for
    // backend infrastructure testing and produces valid (if slightly sub-optimal)
    // retrieval results without the prefix.
    impl Embedder for JinaV5SmallEmbedder {
        fn embed(&self, text: &str) -> IcmResult<Vec<f32>> {
            self.encode_texts(&[text]).map(|mut v| v.remove(0))
        }

        fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
            self.encode_texts(texts)
        }

        fn dimensions(&self) -> usize {
            self.truncate_dim
        }

        fn model_name(&self) -> &str {
            HF_MODEL_ID
        }

        fn license(&self) -> &str {
            "CC-BY-NC-4.0"
        }
    }
}

#[cfg(feature = "jina-v5")]
pub use inner::JinaV5SmallEmbedder;

#[cfg(all(test, feature = "jina-v5"))]
mod tests {
    // `truncate_and_renorm` itself is already tested exhaustively in jina_v5_nano.
    // These tests exercise the dim-validation logic and the small-specific
    // constants (DEFAULT_DIM = 1024, VALID_DIMS includes 1024).
    use crate::jina_v5_nano::truncate_and_renorm;

    /// Truncating to 512 (a valid sub-dimension) produces a 512-dim unit vector.
    #[test]
    fn truncate_correct_dim() {
        // Build a synthetic 1024-dim unit vector.
        let dim = 1024usize;
        let v: Vec<f32> = (0..dim)
            .map(|i| (i as f32).sin())
            .collect::<Vec<_>>()
            .iter()
            .map(|x| x / (dim as f32).sqrt())
            .collect();
        // Truncate to 512.
        let out = truncate_and_renorm(&v, 512);
        assert_eq!(out.len(), 512);
        let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "output must be unit-norm after truncation, got {norm}"
        );
    }

    /// Truncating to 1024 (the maximum = DEFAULT_DIM) is equivalent to no truncation.
    #[test]
    fn truncate_max_dim() {
        let dim = 1024usize;
        // Build a unit vector.
        let raw: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0).recip()).collect();
        let sum_sq: f32 = raw.iter().map(|x| x * x).sum();
        let norm = sum_sq.sqrt();
        let v: Vec<f32> = raw.iter().map(|x| x / norm).collect();

        let out = truncate_and_renorm(&v, 1024);
        assert_eq!(out.len(), 1024);
        let out_norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (out_norm - 1.0).abs() < 1e-5,
            "unit-norm preserved at max dim, got {out_norm}"
        );
    }

    /// Requesting a dimension not in VALID_DIMS must return an error without
    /// attempting any network I/O (dim validation happens before Api::new()).
    #[test]
    fn invalid_dim_rejected() {
        // 999 is not in VALID_DIMS = [32, 64, 128, 256, 512, 768, 1024].
        // Dim validation is the first check in `new()`, so Err is returned
        // before any HF/network access — no offline mitigation needed.
        use crate::error::IcmError;
        let result = super::inner::JinaV5SmallEmbedder::new(Some(999));
        assert!(
            result.is_err(),
            "expected Err for invalid truncate_dim 999, got Ok"
        );
        match result {
            Err(IcmError::Embedding(msg)) => {
                assert!(
                    msg.contains("999"),
                    "error message should mention the invalid dim, got: {msg}"
                );
            }
            Err(other) => panic!("expected IcmError::Embedding, got {other:?}"),
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }
}
