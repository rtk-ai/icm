#![deny(missing_docs)]
#![cfg_attr(feature="ureq", doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")))]
#![cfg_attr(
    not(feature = "ureq"),
    doc = "Documentation is meant to be compiled with default features (at least ureq)"
)]
use std::io::Write;
use std::path::PathBuf;

/// The actual Api to interact with the hub.
#[cfg(any(feature = "tokio", feature = "ureq"))]
pub mod api;

const HF_HOME: &str = "HF_HOME";

/// The type of repo to interact with
#[derive(Debug, Clone, Copy)]
pub enum RepoType {
    /// This is a model, usually it consists of weight files and some configuration
    /// files
    Model,
    /// This is a dataset, usually contains data within parquet files
    Dataset,
    /// This is a space, usually a demo showcashing a given model or dataset
    Space,
}

/// A local struct used to fetch information from the cache folder.
#[derive(Clone, Debug)]
pub struct Cache {
    path: PathBuf,
}

impl Cache {
    /// Creates a new cache object location
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Creates cache from environment variable HF_HOME (if defined) otherwise
    /// defaults to [`home_dir`]/.cache/huggingface/
    pub fn from_env() -> Self {
        match std::env::var(HF_HOME) {
            Ok(home) => {
                let mut path: PathBuf = home.into();
                path.push("hub");
                Self::new(path)
            }
            Err(_) => Self::default(),
        }
    }

    /// Creates a new cache object location
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Returns the location of the token file
    pub fn token_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        // Remove `"hub"`
        path.pop();
        path.push("token");
        path
    }

    /// Returns the token value if it exists in the cache
    /// Use `huggingface-cli login` to set it up.
    pub fn token(&self) -> Option<String> {
        let token_filename = self.token_path();
        if token_filename.exists() {
            log::info!("Using token file found {token_filename:?}");
        }
        match std::fs::read_to_string(token_filename) {
            Ok(token_content) => {
                let token_content = token_content.trim();
                if token_content.is_empty() {
                    None
                } else {
                    Some(token_content.to_string())
                }
            }
            Err(_) => None,
        }
    }

    /// Creates a new handle [`CacheRepo`] which contains operations
    /// on a particular [`Repo`]
    pub fn repo(&self, repo: Repo) -> CacheRepo {
        CacheRepo::new(self.clone(), repo)
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{Cache, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let cache = Cache::new("/tmp/".into());
    /// let cache = cache.repo(Repo::new(model_id, RepoType::Model));
    /// ```
    pub fn model(&self, model_id: String) -> CacheRepo {
        self.repo(Repo::new(model_id, RepoType::Model))
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{Cache, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let cache = Cache::new("/tmp/".into());
    /// let cache = cache.repo(Repo::new(model_id, RepoType::Dataset));
    /// ```
    pub fn dataset(&self, model_id: String) -> CacheRepo {
        self.repo(Repo::new(model_id, RepoType::Dataset))
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{Cache, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let cache = Cache::new("/tmp/".into());
    /// let cache = cache.repo(Repo::new(model_id, RepoType::Space));
    /// ```
    pub fn space(&self, model_id: String) -> CacheRepo {
        self.repo(Repo::new(model_id, RepoType::Space))
    }
}

/// Shorthand for accessing things within a particular repo
#[derive(Debug)]
pub struct CacheRepo {
    cache: Cache,
    repo: Repo,
}

impl CacheRepo {
    fn new(cache: Cache, repo: Repo) -> Self {
        Self { cache, repo }
    }

    /// This will get the location of the file within the cache for the remote
    /// `filename`. Will return `None` if file is not already present in cache.
    pub fn get(&self, filename: &str) -> Option<PathBuf> {
        let commit_path = self.ref_path();
        let commit_hash = std::fs::read_to_string(commit_path).ok()?;
        let mut pointer_path = self.pointer_path(&commit_hash);
        pointer_path.push(filename);
        if pointer_path.exists() {
            Some(pointer_path)
        } else {
            None
        }
    }

    fn path(&self) -> PathBuf {
        let mut ref_path = self.cache.path.clone();
        ref_path.push(self.repo.folder_name());
        ref_path
    }

    fn ref_path(&self) -> PathBuf {
        let mut ref_path = self.path();
        ref_path.push("refs");
        ref_path.push(self.repo.revision());
        ref_path
    }

    /// Creates a reference in the cache directory that points branches to the correct
    /// commits within the blobs.
    pub fn create_ref(&self, commit_hash: &str) -> Result<(), std::io::Error> {
        let ref_path = self.ref_path();
        // Needs to be done like this because revision might contain `/` creating subfolders here.
        std::fs::create_dir_all(ref_path.parent().unwrap())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&ref_path)?;
        file.write_all(commit_hash.trim().as_bytes())?;
        Ok(())
    }

    /// Get the path of the blob with the given etag.
    #[cfg(any(feature = "tokio", feature = "ureq"))]
    pub fn blob_path(&self, etag: &str) -> PathBuf {
        let mut blob_path = self.path();
        blob_path.push("blobs");
        blob_path.push(etag);
        blob_path
    }


    /// Get the path of the snapshot with the given commit hash.
    ///
    /// This path contains symlink pointers to the files for this commit.
    pub fn pointer_path(&self, commit_hash: &str) -> PathBuf {
        let mut pointer_path = self.path();
        pointer_path.push("snapshots");
        pointer_path.push(commit_hash);
        pointer_path
    }
}

impl Default for Cache {
    fn default() -> Self {
        let mut path = dirs::home_dir().expect("Cache directory cannot be found");
        path.push(".cache");
        path.push("huggingface");
        path.push("hub");
        Self::new(path)
    }
}

/// The representation of a repo on the hub.
#[derive(Clone, Debug)]
pub struct Repo {
    repo_id: String,
    repo_type: RepoType,
    revision: String,
}

impl Repo {
    /// Repo with the default branch ("main").
    pub fn new(repo_id: String, repo_type: RepoType) -> Self {
        Self::with_revision(repo_id, repo_type, "main".to_string())
    }

    /// fully qualified Repo
    pub fn with_revision(repo_id: String, repo_type: RepoType, revision: String) -> Self {
        Self {
            repo_id,
            repo_type,
            revision,
        }
    }

    /// Shortcut for [`Repo::new`] with [`RepoType::Model`]
    pub fn model(repo_id: String) -> Self {
        Self::new(repo_id, RepoType::Model)
    }

    /// Shortcut for [`Repo::new`] with [`RepoType::Dataset`]
    pub fn dataset(repo_id: String) -> Self {
        Self::new(repo_id, RepoType::Dataset)
    }

    /// Shortcut for [`Repo::new`] with [`RepoType::Space`]
    pub fn space(repo_id: String) -> Self {
        Self::new(repo_id, RepoType::Space)
    }

    /// The normalized folder nameof the repo within the cache directory
    pub fn folder_name(&self) -> String {
        let prefix = match self.repo_type {
            RepoType::Model => "models",
            RepoType::Dataset => "datasets",
            RepoType::Space => "spaces",
        };
        format!("{prefix}--{}", self.repo_id).replace('/', "--")
    }

    /// The revision
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The actual URL part of the repo
    #[cfg(any(feature = "tokio", feature = "ureq"))]
    pub fn url(&self) -> String {
        match self.repo_type {
            RepoType::Model => self.repo_id.to_string(),
            RepoType::Dataset => {
                format!("datasets/{}", self.repo_id)
            }
            RepoType::Space => {
                format!("spaces/{}", self.repo_id)
            }
        }
    }

    /// Revision needs to be url escaped before being used in a URL
    #[cfg(any(feature = "tokio", feature = "ureq"))]
    pub fn url_revision(&self) -> String {
        self.revision.replace('/', "%2F")
    }

    /// Used to compute the repo's url part when accessing the metadata of the repo
    #[cfg(any(feature = "tokio", feature = "ureq"))]
    pub fn api_url(&self) -> String {
        let prefix = match self.repo_type {
            RepoType::Model => "models",
            RepoType::Dataset => "datasets",
            RepoType::Space => "spaces",
        };
        format!("{prefix}/{}/revision/{}", self.repo_id, self.url_revision())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Internal macro used to show cleaners errors
    /// on the payloads received from the hub.
    #[macro_export]
    macro_rules! assert_no_diff {
        ($left: expr, $right: expr) => {
            let left = serde_json::to_string_pretty(&$left).unwrap();
            let right = serde_json::to_string_pretty(&$right).unwrap();
            if left != right {
                use rand::Rng;
                use std::io::Write;
                use std::process::Command;
                let rand_string: String = rand::rng()
                    .sample_iter(&rand::distr::Alphanumeric)
                    .take(6)
                    .map(char::from)
                    .collect();
                let left_filename = format!("/tmp/left-{rand_string}.txt");
                let mut file = std::fs::File::create(&left_filename).unwrap();
                file.write_all(left.as_bytes()).unwrap();
                let right_filename = format!("/tmp/right-{rand_string}.txt");
                let mut file = std::fs::File::create(&right_filename).unwrap();
                file.write_all(right.as_bytes()).unwrap();
                let output = Command::new("diff")
                    // Reverse order seems to be more appropriate for how we set up the tests.
                    .args(["-U5", &right_filename, &left_filename])
                    .output()
                    .expect("Failed to diff")
                    .stdout;
                let diff = String::from_utf8(output).expect("Invalid utf-8 diff output");
                // eprintln!("assertion `left == right` failed\n{diff}");
                assert!(false, "{diff}")
            };
        };
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn token_path() {
        let cache = Cache::from_env();
        let token_path = cache.token_path().to_str().unwrap().to_string();
        if let Ok(hf_home) = std::env::var(HF_HOME) {
            assert_eq!(token_path, format!("{hf_home}/token"));
        } else {
            let n = "huggingface/token".len();
            assert_eq!(&token_path[token_path.len() - n..], "huggingface/token");
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn token_path() {
        let cache = Cache::from_env();
        let token_path = cache.token_path().to_str().unwrap().to_string();
        if let Ok(hf_home) = std::env::var(HF_HOME) {
            assert_eq!(token_path, format!("{hf_home}\\token"));
        } else {
            let n = "huggingface/token".len();
            assert_eq!(&token_path[token_path.len() - n..], "huggingface\\token");
        }
    }
}
