use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use directories::ProjectDirs;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::embedder::Embedder;
use crate::error::{IcmError, IcmResult};

/// Répertoire cache pour les modèles d'embedding (multi-OS via `directories`).
/// macOS: ~/Library/Caches/dev.icm.icm/models/
/// Linux: ~/.cache/icm/models/
/// Windows: C:\Users\<user>\AppData\Local\icm\icm\cache\models\
fn cache_dir() -> PathBuf {
    ProjectDirs::from("dev", "icm", "icm")
        .map(|dirs| dirs.cache_dir().join("models"))
        .unwrap_or_else(|| {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home)
                .join(".cache")
                .join("icm")
                .join("models")
        })
}

pub struct FastEmbedder {
    model: OnceLock<TextEmbedding>,
    init_lock: Mutex<()>,
    model_name: String,
    dims: usize,
}

/// Default model: multilingual-e5-small (384d, supports 100+ languages)
const DEFAULT_MODEL: &str = "intfloat/multilingual-e5-base";

/// Resolve a model string to (EmbeddingModel, dimensions).
fn resolve_model(name: &str) -> IcmResult<(EmbeddingModel, usize)> {
    let model: EmbeddingModel = name.parse().map_err(|e: String| IcmError::Embedding(e))?;
    let dims = model_dimensions(&model);
    Ok((model, dims))
}

/// Known dimensions for fastembed models.
fn model_dimensions(model: &EmbeddingModel) -> usize {
    match model {
        EmbeddingModel::AllMiniLML6V2
        | EmbeddingModel::AllMiniLML6V2Q
        | EmbeddingModel::AllMiniLML12V2
        | EmbeddingModel::AllMiniLML12V2Q
        | EmbeddingModel::BGESmallENV15
        | EmbeddingModel::BGESmallENV15Q
        | EmbeddingModel::MultilingualE5Small
        | EmbeddingModel::ParaphraseMLMiniLML12V2
        | EmbeddingModel::ParaphraseMLMiniLML12V2Q => 384,

        EmbeddingModel::BGEBaseENV15
        | EmbeddingModel::BGEBaseENV15Q
        | EmbeddingModel::MultilingualE5Base
        | EmbeddingModel::ParaphraseMLMpnetBaseV2
        | EmbeddingModel::BGESmallZHV15
        | EmbeddingModel::GTEBaseENV15
        | EmbeddingModel::GTEBaseENV15Q
        | EmbeddingModel::JinaEmbeddingsV2BaseCode => 768,

        EmbeddingModel::BGELargeENV15
        | EmbeddingModel::BGELargeENV15Q
        | EmbeddingModel::MultilingualE5Large
        | EmbeddingModel::MxbaiEmbedLargeV1
        | EmbeddingModel::MxbaiEmbedLargeV1Q
        | EmbeddingModel::BGELargeZHV15
        | EmbeddingModel::GTELargeENV15
        | EmbeddingModel::GTELargeENV15Q
        | EmbeddingModel::ModernBertEmbedLarge => 1024,

        EmbeddingModel::NomicEmbedTextV1
        | EmbeddingModel::NomicEmbedTextV15
        | EmbeddingModel::NomicEmbedTextV15Q => 768,

        EmbeddingModel::ClipVitB32 => 512,
    }
}

impl FastEmbedder {
    /// Create with default model (multilingual-e5-small).
    pub fn new() -> Self {
        Self::with_model(DEFAULT_MODEL)
    }

    /// Create with a specific model by name (e.g. "intfloat/multilingual-e5-small").
    pub fn with_model(model_name: &str) -> Self {
        let dims = resolve_model(model_name).map(|(_, d)| d).unwrap_or(384);
        Self {
            model: OnceLock::new(),
            init_lock: Mutex::new(()),
            model_name: model_name.to_string(),
            dims,
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
        let (emb_model, _) = resolve_model(&self.model_name)?;
        let cache = cache_dir();
        std::fs::create_dir_all(&cache)
            .and_then(|()| cachedir::ensure_tag(&cache))
            .unwrap_or_else(|e| tracing::warn!("could not tag cache dir: {e}"));
        let model = TextEmbedding::try_new(
            InitOptions::new(emb_model)
                .with_show_download_progress(true)
                .with_cache_dir(cache),
        )
        .map_err(|e| IcmError::Embedding(format!("failed to init model: {e}")))?;
        let _ = self.model.set(model);
        Ok(self.model.get().unwrap())
    }

    /// e5-family instruction prefixes as `(query_prefix, passage_prefix)`.
    ///
    /// The multilingual-e5 models are trained to expect `"query: "` on search
    /// queries and `"passage: "` on stored documents; omitting them degrades
    /// retrieval quality (per the intfloat/multilingual-e5 model card). Every
    /// other model family is left unprefixed so its behaviour is unchanged.
    fn instruction_prefixes(&self) -> (&'static str, &'static str) {
        match resolve_model(&self.model_name) {
            Ok((
                EmbeddingModel::MultilingualE5Small
                | EmbeddingModel::MultilingualE5Base
                | EmbeddingModel::MultilingualE5Large,
                _,
            )) => ("query: ", "passage: "),
            _ => ("", ""),
        }
    }

    /// Embed a single text, optionally prepending an instruction `prefix`.
    fn embed_one(&self, prefix: &str, text: &str) -> IcmResult<Vec<f32>> {
        let model = self.get_model()?;
        let prefixed: String;
        let input: &str = if prefix.is_empty() {
            text
        } else {
            prefixed = format!("{prefix}{text}");
            &prefixed
        };
        let results = model
            .embed(vec![input], None)
            .map_err(|e| IcmError::Embedding(e.to_string()))?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| IcmError::Embedding("empty embedding result".into()))
    }
}

impl Default for FastEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for FastEmbedder {
    /// Embed a document for storage, applying the model's passage prefix.
    fn embed(&self, text: &str) -> IcmResult<Vec<f32>> {
        let (_, passage) = self.instruction_prefixes();
        self.embed_one(passage, text)
    }

    /// Embed a search query, applying the model's query prefix.
    fn embed_query(&self, text: &str) -> IcmResult<Vec<f32>> {
        let (query, _) = self.instruction_prefixes();
        self.embed_one(query, text)
    }

    fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.get_model()?;
        let (_, passage) = self.instruction_prefixes();
        if passage.is_empty() {
            model
                .embed(texts.to_vec(), None)
                .map_err(|e| IcmError::Embedding(e.to_string()))
        } else {
            let prefixed: Vec<String> = texts.iter().map(|t| format!("{passage}{t}")).collect();
            model
                .embed(prefixed, None)
                .map_err(|e| IcmError::Embedding(e.to_string()))
        }
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e5_models_use_instruction_prefixes() {
        for model in [
            "Qdrant/multilingual-e5-large-onnx",
            "intfloat/multilingual-e5-base",
            "intfloat/multilingual-e5-small",
        ] {
            let embedder = FastEmbedder::with_model(model);
            assert_eq!(
                embedder.instruction_prefixes(),
                ("query: ", "passage: "),
                "expected e5 instruction prefixes for {model}"
            );
        }
    }

    #[test]
    fn non_e5_models_are_left_unprefixed() {
        for model in [
            "Xenova/bge-small-en-v1.5",
            "Qdrant/all-MiniLM-L6-v2-onnx",
            "Alibaba-NLP/gte-large-en-v1.5",
        ] {
            let embedder = FastEmbedder::with_model(model);
            assert_eq!(
                embedder.instruction_prefixes(),
                ("", ""),
                "expected no instruction prefix for {model}"
            );
        }
    }
}
