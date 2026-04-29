use crate::error::IcmResult;

pub trait Embedder: Send + Sync {
    // --- required (existing) ---
    fn embed(&self, text: &str) -> IcmResult<Vec<f32>>;
    fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;

    // --- optional with defaults (new) ---

    /// Embed a query string. Default: delegates to `embed`. Override for
    /// asymmetric retrieval models that use different prefixes for queries.
    fn embed_query(&self, text: &str) -> IcmResult<Vec<f32>> {
        self.embed(text)
    }

    /// Embed a document/passage for storage. Default: delegates to `embed`.
    /// Override for asymmetric retrieval models.
    fn embed_document(&self, text: &str) -> IcmResult<Vec<f32>> {
        self.embed(text)
    }

    /// Human-readable model identifier
    /// (e.g. "jina-embeddings-v5-text-nano-retrieval").
    fn model_name(&self) -> &str {
        ""
    }

    /// SPDX license expression for the model weights (e.g. "CC-BY-NC-4.0").
    /// Empty string for open/Apache models. Consumed by `icm config show` (S-5).
    fn license(&self) -> &str {
        ""
    }
}
