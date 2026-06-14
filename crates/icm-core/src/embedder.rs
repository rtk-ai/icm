use crate::error::IcmResult;

pub trait Embedder: Send + Sync {
    /// Embed a document/passage for storage.
    ///
    /// For instruction-tuned retrieval models (e.g. the multilingual-e5
    /// family), implementations apply the model's *document* prefix here
    /// (`"passage: "` for e5).
    fn embed(&self, text: &str) -> IcmResult<Vec<f32>>;

    /// Embed a search query.
    ///
    /// e5-style models use an asymmetric scheme: queries are prefixed
    /// `"query: "` and documents `"passage: "`. Omitting the prefixes degrades
    /// retrieval (per the intfloat/multilingual-e5 model card), so query
    /// embedding is distinct from [`Embedder::embed`]. The default delegates to
    /// `embed`, which is correct for models with no query/document distinction.
    fn embed_query(&self, text: &str) -> IcmResult<Vec<f32>> {
        self.embed(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
