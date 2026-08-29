use std::path::PathBuf;
use std::sync::Mutex;

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

/// Resolve the onnxruntime library name `ort`'s load-dynamic backend would use:
/// `ORT_DYLIB_PATH` if set, else the platform default name.
#[cfg(all(any(unix, windows), feature = "embeddings-dynamic"))]
fn onnxruntime_lib_name() -> String {
    let default_name = if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else {
        "libonnxruntime.so"
    };
    match std::env::var("ORT_DYLIB_PATH") {
        Ok(p) if !p.is_empty() => p,
        _ => default_name.to_string(),
    }
}

/// Best-effort check that the onnxruntime shared library is loadable, mirroring
/// how `ort`'s load-dynamic backend resolves it. Used only by the load-dynamic
/// build (issue #345) to avoid ort's panic-on-missing-runtime under
/// `panic = "abort"` — we pre-flight the load and degrade to keyword-only rather
/// than let the process abort.
#[cfg(all(unix, feature = "embeddings-dynamic"))]
fn onnxruntime_dylib_available() -> bool {
    use std::ffi::CString;
    let Ok(cname) = CString::new(onnxruntime_lib_name()) else {
        return false;
    };
    // SAFETY: `cname` is a valid NUL-terminated C string for the duration of the
    // call; the returned handle is only tested for null and immediately closed.
    unsafe {
        let handle = libc::dlopen(cname.as_ptr(), libc::RTLD_LAZY);
        if handle.is_null() {
            false
        } else {
            libc::dlclose(handle);
            true
        }
    }
}

/// Windows equivalent of the pre-flight, via raw `kernel32` `LoadLibraryW`
/// (kernel32 is always linked on MSVC, so no extra crate is needed).
#[cfg(all(windows, feature = "embeddings-dynamic"))]
fn onnxruntime_dylib_available() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut core::ffi::c_void;
        fn FreeLibrary(h_lib_module: *mut core::ffi::c_void) -> i32;
    }

    let wide: Vec<u16> = OsStr::new(&onnxruntime_lib_name())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the duration of
    // the call; the returned handle is only tested for null and immediately freed.
    unsafe {
        let handle = LoadLibraryW(wide.as_ptr());
        if handle.is_null() {
            false
        } else {
            FreeLibrary(handle);
            true
        }
    }
}

/// On exotic platforms with no dlopen/LoadLibrary probe, don't block: let ort
/// resolve the runtime itself (these targets are not shipped, issue #345).
#[cfg(all(not(any(unix, windows)), feature = "embeddings-dynamic"))]
fn onnxruntime_dylib_available() -> bool {
    true
}

/// Register execution providers on `opts` beyond onnxruntime's default CPU
/// provider.
///
/// CUDA (NVIDIA GPU) when `embeddings-cuda` is compiled in, on every
/// platform — falls back to CPU automatically if no CUDA-capable GPU or
/// toolkit is present, so it's safe to enable in a build that might run on
/// GPU-less machines. CoreML (Apple Neural Engine / GPU) when
/// `embeddings-coreml` is compiled in on macOS — see that feature's doc
/// comment in `icm-core/Cargo.toml` for why it's not recommended (measured
/// slower than plain CPU, GPU never actually engaged on the host it was
/// tested on). `ort` is only a dependency at all when one of these
/// features is enabled (it's `optional`), so the fallback body can't
/// reference any `ort` type — hence two same-signature functions selected
/// by `cfg` rather than one function with a feature-gated body.
/// Registering an EP does not remove the CPU fallback — `ort` tries each
/// provider in order and falls back automatically if one can't handle a
/// given op or fails to initialize.
#[cfg(any(feature = "embeddings-cuda", feature = "embeddings-coreml"))]
fn with_execution_providers(opts: InitOptions) -> InitOptions {
    let mut eps = Vec::new();
    #[cfg(feature = "embeddings-cuda")]
    eps.push(ort::ep::CUDA::default().build());
    #[cfg(all(feature = "embeddings-coreml", target_os = "macos"))]
    eps.push(
        ort::ep::CoreML::default()
            .with_compute_units(ort::ep::coreml::ComputeUnits::All)
            .build(),
    );
    if eps.is_empty() {
        opts
    } else {
        opts.with_execution_providers(eps)
    }
}

#[cfg(not(any(feature = "embeddings-cuda", feature = "embeddings-coreml")))]
fn with_execution_providers(opts: InitOptions) -> InitOptions {
    opts
}

pub struct FastEmbedder {
    // fastembed 6's `TextEmbedding::embed` takes `&mut self`; a single
    // mutex both lazily initializes the model on first use and serializes
    // the `&mut` access `embed`/`embed_query`/`embed_batch` need, while
    // `Embedder`'s trait methods stay `&self` (matches every other
    // `MemoryStore`-adjacent trait in this codebase, which assumes a
    // shared, thread-safe embedder).
    model: Mutex<Option<TextEmbedding>>,
    model_name: String,
    dims: usize,
}

/// Default model: multilingual-e5-large (1024d). Unlike -small/-base,
/// fastembed's `model_code` for the large variant is the Qdrant ONNX
/// conversion, not an `intfloat/...` path — confirmed via
/// `TextEmbedding::list_supported_models()` (and matches the existing
/// `e5_models_use_instruction_prefixes` test, which already covers this
/// exact string). Using `intfloat/multilingual-e5-large` here silently
/// failed every embed call (`resolve_model` returned `Unknown embedding
/// model`), degrading every store to no-embedding and every recall to the
/// FTS/keyword fallback — caught by a 0.0% LoCoMo pilot recall@5 that had
/// no business being that low.
///
/// LoCoMo benchmark pilot (2026-08-29, conv-26, 419 turns, 28 questions,
/// evidence recall@5): multilingual-e5-**base** (768d) scored 56.2%;
/// swapping to BAAI/bge-m3 (1024d, MTEB-competitive, but tuned for
/// long-document retrieval with an 8192-token context) *regressed* to
/// 50.9% on this short-dialogue-turn workload — a higher general
/// leaderboard rank didn't transfer to this task. e5-large is the same
/// architecture/training recipe that already measured well, one size up.
///
/// `pub` and re-exported so `icm-cli`'s `EmbeddingsConfig::default()` can
/// reference this instead of duplicating the string: the two used to drift
/// (config.rs hardcoded its own default and kept pointing at the previous
/// choice after this constant changed, so nothing here actually took
/// effect for a real `icm store`/`icm recall` run until config.rs was
/// fixed too).
pub const DEFAULT_MODEL: &str = "Qdrant/multilingual-e5-large-onnx";

/// Resolve a model string (e.g. `"intfloat/multilingual-e5-base"`,
/// `"BAAI/bge-m3"`) to (EmbeddingModel, dimensions).
///
/// Deliberately not `str::parse::<EmbeddingModel>()`: fastembed 6 changed
/// `FromStr` to match the enum variant's `Debug` representation
/// (`"MultilingualE5Large"`) instead of its HuggingFace-style `model_code`
/// (`"Qdrant/multilingual-e5-large-onnx"`, still what `ModelInfo` and every
/// fastembed doc/example use for display) — silently breaking every
/// HuggingFace-style name this codebase's config, CLI docs and tests use
/// (caught by `e5_models_use_instruction_prefixes` failing on the v4→v6
/// bump). Look up by `model_code` directly via `list_supported_models`
/// instead, so those names keep working regardless of how fastembed's own
/// `FromStr` behaves release to release.
fn resolve_model(name: &str) -> IcmResult<(EmbeddingModel, usize)> {
    TextEmbedding::list_supported_models()
        .into_iter()
        .find(|info| info.model_code.eq_ignore_ascii_case(name))
        .map(|info| (info.model, info.dim))
        .ok_or_else(|| IcmError::Embedding(format!("Unknown embedding model: {name}")))
}

impl FastEmbedder {
    /// Create with the default model (see `DEFAULT_MODEL`).
    pub fn new() -> Self {
        Self::with_model(DEFAULT_MODEL)
    }

    /// Create with a specific model by name (e.g. "intfloat/multilingual-e5-small").
    pub fn with_model(model_name: &str) -> Self {
        let dims = resolve_model(model_name).map(|(_, d)| d).unwrap_or(384);
        Self {
            model: Mutex::new(None),
            model_name: model_name.to_string(),
            dims,
        }
    }

    /// Run `f` against the lazily-initialized model, holding the lock for
    /// the duration of the call (fastembed 6's `embed` needs `&mut self`).
    fn with_loaded_model<R>(
        &self,
        f: impl FnOnce(&mut TextEmbedding) -> IcmResult<R>,
    ) -> IcmResult<R> {
        let mut guard = self.model.lock().unwrap();
        if guard.is_none() {
            let (emb_model, _) = resolve_model(&self.model_name)?;
            let cache = cache_dir();
            std::fs::create_dir_all(&cache)
                .and_then(|()| cachedir::ensure_tag(&cache))
                .unwrap_or_else(|e| tracing::warn!("could not tag cache dir: {e}"));
            // With the load-dynamic ort backend (issue #345) onnxruntime is
            // resolved at runtime. If it's absent, ort *panics* inside init —
            // and since the release profile is `panic = "abort"`, that would
            // kill the process rather than unwind (so catch_unwind can't
            // help). Pre-flight the dylib instead: if it can't be dlopen'd,
            // return a clean error (→ keyword-only search) before ort ever
            // initializes. Static builds link onnxruntime in, so this check
            // is compiled out there.
            #[cfg(feature = "embeddings-dynamic")]
            if !onnxruntime_dylib_available() {
                return Err(IcmError::Embedding(
                    "onnxruntime runtime not found for this load-dynamic build; \
                     install onnxruntime (or set ORT_DYLIB_PATH), or run with \
                     --no-embeddings for keyword-only search"
                        .to_string(),
                ));
            }
            let init_opts = with_execution_providers(
                InitOptions::new(emb_model)
                    .with_show_download_progress(true)
                    .with_cache_dir(cache),
            );
            let model = TextEmbedding::try_new(init_opts)
                .map_err(|e| IcmError::Embedding(format!("failed to init model: {e}")))?;
            *guard = Some(model);
        }
        f(guard.as_mut().unwrap())
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
        let prefixed: String;
        let input: &str = if prefix.is_empty() {
            text
        } else {
            prefixed = format!("{prefix}{text}");
            &prefixed
        };
        self.with_loaded_model(|model| {
            let results = model
                .embed(vec![input], None)
                .map_err(|e| IcmError::Embedding(e.to_string()))?;
            results
                .into_iter()
                .next()
                .ok_or_else(|| IcmError::Embedding("empty embedding result".into()))
        })
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
        let (_, passage) = self.instruction_prefixes();
        self.with_loaded_model(|model| {
            if passage.is_empty() {
                model
                    .embed(texts, None)
                    .map_err(|e| IcmError::Embedding(e.to_string()))
            } else {
                let prefixed: Vec<String> = texts.iter().map(|t| format!("{passage}{t}")).collect();
                model
                    .embed(prefixed, None)
                    .map_err(|e| IcmError::Embedding(e.to_string()))
            }
        })
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

    // The load-dynamic pre-flight (issue #345) must report a bogus dylib path as
    // unavailable — this is what lets a load-dynamic build return a clean error
    // (→ keyword-only) instead of ort aborting under `panic = "abort"`. Run with
    // `cargo test -p icm-core --features embeddings-dynamic`.
    #[cfg(all(any(unix, windows), feature = "embeddings-dynamic"))]
    #[test]
    fn missing_onnxruntime_dylib_is_reported_unavailable() {
        let bogus = if cfg!(windows) {
            r"C:\nonexistent\icm-test\onnxruntime-does-not-exist.dll"
        } else {
            "/nonexistent/icm-test/libonnxruntime-does-not-exist.so"
        };
        let prev = std::env::var("ORT_DYLIB_PATH").ok();
        std::env::set_var("ORT_DYLIB_PATH", bogus);
        assert!(
            !onnxruntime_dylib_available(),
            "a non-existent ORT_DYLIB_PATH must be detected as unavailable"
        );
        match prev {
            Some(v) => std::env::set_var("ORT_DYLIB_PATH", v),
            None => std::env::remove_var("ORT_DYLIB_PATH"),
        }
    }
}
