//! Jina v5-text-nano embedder — local ONNX inference via `ort` + `tokenizers`.
//!
//! This backend is gated behind the `jina-v5` Cargo feature and is OFF by
//! default. License: CC-BY-NC-4.0 (non-commercial). See:
//! <https://huggingface.co/jinaai/jina-embeddings-v5-text-nano-retrieval>

#[cfg(feature = "jina-v5")]
mod inner {
    use hf_hub::api::sync::Api;
    use ndarray::Array2;
    use ort::session::{builder::GraphOptimizationLevel, Session};
    use tokenizers::Tokenizer;

    use crate::embedder::Embedder;
    use crate::error::{IcmError, IcmResult};

    const HF_MODEL_ID: &str = "jinaai/jina-embeddings-v5-text-nano-retrieval";
    const DEFAULT_DIM: usize = 768;
    const VALID_DIMS: &[usize] = &[32, 64, 128, 256, 512, 768];

    pub struct JinaV5NanoEmbedder {
        session: Session,
        tokenizer: Tokenizer,
        truncate_dim: usize,
    }

    impl JinaV5NanoEmbedder {
        pub fn new(truncate_dim: Option<usize>) -> IcmResult<Self> {
            let dim = match truncate_dim {
                Some(d) if VALID_DIMS.contains(&d) => d,
                Some(d) => {
                    return Err(IcmError::Embedding(format!(
                        "invalid truncate_dim {d} for jina-v5-nano; valid: {VALID_DIMS:?}"
                    )));
                }
                None => DEFAULT_DIM,
            };

            let api = Api::new().map_err(|e| IcmError::Embedding(e.to_string()))?;
            let repo = api.model(HF_MODEL_ID.to_string());

            eprintln!(
                "Loading jina-v5-text-nano-retrieval (downloads on first run, cached thereafter)..."
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
    impl Embedder for JinaV5NanoEmbedder {
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

    /// Slice `v` to its first `n` dims (or `v.len()` if smaller) and L2-renormalize.
    /// Used for Matryoshka representation truncation.
    pub fn truncate_and_renorm(v: &[f32], n: usize) -> Vec<f32> {
        let take = n.min(v.len());
        let sliced = &v[..take];
        let norm: f32 = sliced.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-8 {
            let inv = 1.0_f32 / norm;
            sliced.iter().map(|x| x * inv).collect()
        } else {
            sliced.to_vec()
        }
    }
}

#[cfg(feature = "jina-v5")]
pub use inner::truncate_and_renorm;
#[cfg(feature = "jina-v5")]
pub use inner::JinaV5NanoEmbedder;

#[cfg(all(test, feature = "jina-v5"))]
mod tests {
    use super::inner::truncate_and_renorm;

    #[test]
    fn truncate_and_renorm_shape_and_unit_norm() {
        // Input: a known unnormalized 4-dim vector.
        let input = [3.0f32, 4.0, 0.0, 0.0]; // L2 norm = 5.0
        // First L2-normalize it (simulating model output).
        let normalized: Vec<f32> = input.iter().map(|x| x / 5.0).collect();
        // Truncate to 2 dims.
        let out = truncate_and_renorm(&normalized, 2);
        assert_eq!(out.len(), 2);
        let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-6,
            "output must be unit-norm, got {norm}"
        );
        // Expected: normalize([0.6, 0.8]) = [0.6, 0.8] / 1.0 = [0.6, 0.8].
        assert!((out[0] - 0.6).abs() < 1e-6);
        assert!((out[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn truncate_and_renorm_n_equals_len() {
        let v = vec![1.0f32 / 3.0f32.sqrt(); 3];
        let out = truncate_and_renorm(&v, 3);
        assert_eq!(out.len(), 3);
        let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn truncate_and_renorm_n_larger_than_len_is_clamped() {
        let v = vec![0.6_f32, 0.8];
        let out = truncate_and_renorm(&v, 8);
        assert_eq!(out.len(), 2);
        let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn truncate_and_renorm_zero_vector_passthrough() {
        let v = vec![0.0_f32; 4];
        let out = truncate_and_renorm(&v, 2);
        assert_eq!(out, vec![0.0, 0.0]);
    }
}
