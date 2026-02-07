use crate::error::IcmResult;

pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> IcmResult<Vec<f32>>;
    fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
