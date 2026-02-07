use std::sync::{Mutex, OnceLock};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::embedder::Embedder;
use crate::error::{IcmError, IcmResult};

pub struct FastEmbedder {
    model: OnceLock<TextEmbedding>,
    init_lock: Mutex<()>,
}

impl FastEmbedder {
    pub fn new() -> Self {
        Self {
            model: OnceLock::new(),
            init_lock: Mutex::new(()),
        }
    }

    fn get_model(&self) -> IcmResult<&TextEmbedding> {
        if let Some(m) = self.model.get() {
            return Ok(m);
        }
        let _guard = self.init_lock.lock().unwrap();
        if let Some(m) = self.model.get() {
            return Ok(m);
        }
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15Q).with_show_download_progress(true),
        )
        .map_err(|e| IcmError::Embedding(format!("failed to init model: {e}")))?;
        let _ = self.model.set(model);
        Ok(self.model.get().unwrap())
    }
}

impl Default for FastEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for FastEmbedder {
    fn embed(&self, text: &str) -> IcmResult<Vec<f32>> {
        let model = self.get_model()?;
        let results = model
            .embed(vec![text], None)
            .map_err(|e| IcmError::Embedding(e.to_string()))?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| IcmError::Embedding("empty embedding result".into()))
    }

    fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.get_model()?;
        model
            .embed(texts.to_vec(), None)
            .map_err(|e| IcmError::Embedding(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        384
    }
}
