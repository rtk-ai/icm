use super::Progress as SyncProgress;
use super::{RepoInfo, HF_ENDPOINT};
use crate::{Cache, Repo, RepoType};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indicatif::ProgressBar;
use rand::Rng;
use reqwest::{
    header::{
        HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue, ToStrError, AUTHORIZATION,
        CONTENT_RANGE, LOCATION, RANGE, USER_AGENT,
    },
    redirect::Policy,
    Client, Error as ReqwestError, RequestBuilder,
};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::num::ParseIntError;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::{AcquireError, Semaphore, TryAcquireError};
use tokio::task::JoinError;

/// Current version (used in user-agent)
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Current name (used in user-agent)
const NAME: &str = env!("CARGO_PKG_NAME");

const EXTENSION: &str = "sync.part";

/// This trait is used by users of the lib
/// to implement custom behavior during file downloads
pub trait Progress {
    /// At the start of the download
    /// The size is the total size in bytes of the file.
    fn init(&mut self, size: usize, filename: &str)
        -> impl std::future::Future<Output = ()> + Send;
    /// This function is called whenever `size` bytes have been
    /// downloaded in the temporary file
    fn update(&mut self, size: usize) -> impl std::future::Future<Output = ()> + Send;
    /// This is called at the end of the download
    fn finish(&mut self) -> impl std::future::Future<Output = ()> + Send;
}

impl Progress for ProgressBar {
    async fn init(&mut self, size: usize, filename: &str) {
        <ProgressBar as SyncProgress>::init(self, size, filename);
    }
    async fn finish(&mut self) {
        <ProgressBar as SyncProgress>::finish(self);
    }
    async fn update(&mut self, size: usize) {
        <ProgressBar as SyncProgress>::update(self, size);
    }
}

impl Progress for () {
    async fn init(&mut self, _size: usize, _filename: &str) {}
    async fn finish(&mut self) {}
    async fn update(&mut self, _size: usize) {}
}

struct Handle {
    file: tokio::fs::File,
}

impl Drop for Handle {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

async fn lock_file(mut path: PathBuf) -> Result<Handle, ApiError> {
    path.set_extension("lock");

    let file = tokio::fs::File::create(path.clone()).await?;
    let mut res = lock(&file);
    for _ in 0..5 {
        if res == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        res = lock(&file);
    }
    if res != 0 {
        Err(ApiError::LockAcquisition(path))
    } else {
        Ok(Handle { file })
    }
}

#[cfg(target_family = "unix")]
mod unix {
    use std::os::fd::AsRawFd;

    pub(crate) fn lock(file: &tokio::fs::File) -> i32 {
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
    }
    pub(crate) fn unlock(file: &tokio::fs::File) -> i32 {
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) }
    }
}
#[cfg(target_family = "unix")]
use unix::{lock, unlock};

#[cfg(target_family = "windows")]
mod windows {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };

    pub(crate) fn lock(file: &tokio::fs::File) -> i32 {
        unsafe {
            let mut overlapped = std::mem::zeroed();
            let flags = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY;
            let res = LockFileEx(
                file.as_raw_handle() as HANDLE,
                flags,
                0,
                !0,
                !0,
                &mut overlapped,
            );
            1 - res
        }
    }
    pub(crate) fn unlock(file: &tokio::fs::File) -> i32 {
        unsafe { UnlockFile(file.as_raw_handle() as HANDLE, 0, 0, !0, !0) }
    }
}
#[cfg(target_family = "windows")]
use windows::{lock, unlock};

#[cfg(not(any(target_family = "unix", target_family = "windows")))]
mod other {
    pub(crate) fn lock(file: &tokio::fs::File) -> i32 {
        0
    }
    pub(crate) fn unlock(file: &tokio::fs::File) -> i32 {
        0
    }
}
#[cfg(not(any(target_family = "unix", target_family = "windows")))]
use other::{lock, unlock};

#[derive(Debug, Error)]
/// All errors the API can throw
pub enum ApiError {
    /// Api expects certain header to be present in the results to derive some information
    #[error("Header {0} is missing")]
    MissingHeader(HeaderName),

    /// The header exists, but the value is not conform to what the Api expects.
    #[error("Header {0} is invalid")]
    InvalidHeader(HeaderName),

    /// The value cannot be used as a header during request header construction
    #[error("Invalid header value {0}")]
    InvalidHeaderValue(#[from] InvalidHeaderValue),

    /// The header value is not valid utf-8
    #[error("header value is not a string")]
    ToStr(#[from] ToStrError),

    /// Error in the request
    #[error("request error: {0}")]
    RequestError(#[from] ReqwestError),

    /// Error parsing some range value
    #[error("Cannot parse int")]
    ParseIntError(#[from] ParseIntError),

    /// I/O Error
    #[error("I/O error {0}")]
    IoError(#[from] std::io::Error),

    /// We tried to download chunk too many times
    #[error("Too many retries: {0}")]
    TooManyRetries(Box<ApiError>),

    /// Semaphore cannot be acquired
    #[error("Try acquire: {0}")]
    TryAcquireError(#[from] TryAcquireError),

    /// Semaphore cannot be acquired
    #[error("Acquire: {0}")]
    AcquireError(#[from] AcquireError),
    // /// Semaphore cannot be acquired
    // #[error("Invalid Response: {0:?}")]
    // InvalidResponse(Response),
    /// Join failed
    #[error("Join: {0}")]
    Join(#[from] JoinError),

    /// We failed to acquire lock for file `f`. Meaning
    /// Someone else is writing/downloading said file
    #[error("Lock acquisition failed: {0}")]
    LockAcquisition(PathBuf),
}

/// Helper to create [`Api`] with all the options.
#[derive(Debug)]
pub struct ApiBuilder {
    endpoint: String,
    cache: Cache,
    token: Option<String>,
    max_files: usize,
    chunk_size: Option<usize>,
    parallel_failures: usize,
    max_retries: usize,
    progress: bool,
    user_agent: Vec<(String, String)>,
}

impl Default for ApiBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiBuilder {
    /// Default api builder
    /// ```
    /// use hf_hub::api::tokio::ApiBuilder;
    /// let api = ApiBuilder::new().build().unwrap();
    /// ```
    pub fn new() -> Self {
        let cache = Cache::default();
        Self::from_cache(cache)
    }

    /// Creates API with values potentially from environment variables.
    /// HF_HOME decides the location of the cache folder
    /// HF_ENDPOINT modifies the URL for the huggingface location
    /// to download files from.
    /// ```
    /// use hf_hub::api::tokio::ApiBuilder;
    /// let api = ApiBuilder::from_env().build().unwrap();
    /// ```
    pub fn from_env() -> Self {
        let cache = Cache::from_env();
        let mut builder = Self::from_cache(cache);
        if let Ok(endpoint) = std::env::var(HF_ENDPOINT) {
            builder = builder.with_endpoint(endpoint);
        }
        builder
    }

    /// High CPU download
    ///
    /// This may cause issues on regular desktops as it will saturate
    /// CPUs by multiplexing the downloads.
    /// However on high CPU machines on the cloud, this may help
    /// saturate the bandwidth (>500MB/s) better.
    /// ```
    /// use hf_hub::api::tokio::ApiBuilder;
    /// let api = ApiBuilder::new().high().build().unwrap();
    /// ```
    pub fn high(self) -> Self {
        self.with_max_files(num_cpus::get())
            .with_chunk_size(Some(10_000_000))
    }

    /// From a given cache
    /// ```
    /// use hf_hub::{api::tokio::ApiBuilder, Cache};
    /// let path = std::path::PathBuf::from("/tmp");
    /// let cache = Cache::new(path);
    /// let api = ApiBuilder::from_cache(cache).build().unwrap();
    /// ```
    pub fn from_cache(cache: Cache) -> Self {
        let token = cache.token();

        let progress = true;

        let user_agent = vec![
            ("unknown".to_string(), "None".to_string()),
            (NAME.to_string(), VERSION.to_string()),
            ("rust".to_string(), "unknown".to_string()),
        ];

        Self {
            endpoint: "https://huggingface.co".to_string(),
            cache,
            token,
            max_files: 1,
            // We need to have some chunk size for things to be able to resume.
            chunk_size: Some(10_000_000),
            parallel_failures: 0,
            max_retries: 0,
            progress,
            user_agent,
        }
    }

    /// Whether to show a progressbar
    pub fn with_progress(mut self, progress: bool) -> Self {
        self.progress = progress;
        self
    }

    /// Changes the endpoint of the API. Default is `https://huggingface.co`.
    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Changes the location of the cache directory. Defaults is `~/.cache/huggingface/`.
    pub fn with_cache_dir(mut self, cache_dir: PathBuf) -> Self {
        self.cache = Cache::new(cache_dir);
        self
    }

    /// Sets the token to be used in the API
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    /// Sets the number of open files
    pub fn with_max_files(mut self, max_files: usize) -> Self {
        self.max_files = max_files;
        self
    }

    /// Sets the size of each chunk
    pub fn with_chunk_size(mut self, chunk_size: Option<usize>) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Adds custom fields to headers user-agent
    pub fn with_user_agent(mut self, key: &str, value: &str) -> Self {
        self.user_agent.push((key.to_string(), value.to_string()));
        self
    }

    fn build_headers(&self) -> Result<HeaderMap, ApiError> {
        let mut headers = HeaderMap::new();
        let user_agent = self
            .user_agent
            .iter()
            .map(|(key, value)| format!("{key}/{value}"))
            .collect::<Vec<_>>()
            .join("; ");
        headers.insert(USER_AGENT, HeaderValue::from_str(&user_agent)?);
        if let Some(token) = &self.token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        Ok(headers)
    }

    /// Consumes the builder and builds the final [`Api`]
    pub fn build(self) -> Result<Api, ApiError> {
        let headers = self.build_headers()?;
        let client = Client::builder().default_headers(headers.clone()).build()?;

        // Policy: only follow relative redirects
        // See: https://github.com/huggingface/huggingface_hub/blob/9c6af39cdce45b570f0b7f8fad2b311c96019804/src/huggingface_hub/file_download.py#L411
        let relative_redirect_policy = Policy::custom(|attempt| {
            // Follow redirects up to a maximum of 10.
            if attempt.previous().len() > 10 {
                return attempt.error("too many redirects");
            }

            if let Some(last) = attempt.previous().last() {
                // If the url is not relative
                if last.make_relative(attempt.url()).is_none() {
                    return attempt.stop();
                }
            }

            // Follow redirect
            attempt.follow()
        });

        let relative_redirect_client = Client::builder()
            .redirect(relative_redirect_policy)
            .default_headers(headers)
            .build()?;
        Ok(Api {
            endpoint: self.endpoint,
            cache: self.cache,
            client,
            relative_redirect_client,
            max_files: self.max_files,
            chunk_size: self.chunk_size,
            parallel_failures: self.parallel_failures,
            max_retries: self.max_retries,
            progress: self.progress,
        })
    }
}

/// File metadata.
#[derive(Debug)]
pub struct Metadata {
    commit_hash: String,
    etag: String,
    size: usize,
}

impl Metadata {
    /// Get the commit hash of the file.
    pub fn commit_hash(&self) -> &str {
        &self.commit_hash
    }

    /// Get the etag of the file.
    pub fn etag(&self) -> &str {
        &self.etag
    }

    /// Get the file size.
    pub fn size(&self) -> usize {
        self.size
    }
}

/// The actual Api used to interact with the hub.
/// Use any repo with [`Api::repo`]
#[derive(Clone, Debug)]
pub struct Api {
    endpoint: String,
    cache: Cache,
    client: Client,
    relative_redirect_client: Client,
    max_files: usize,
    chunk_size: Option<usize>,
    parallel_failures: usize,
    max_retries: usize,
    progress: bool,
}

fn make_relative(src: &Path, dst: &Path) -> PathBuf {
    let path = src;
    let base = dst;

    assert_eq!(
        path.is_absolute(),
        base.is_absolute(),
        "This function is made to look at absolute paths only"
    );
    let mut ita = path.components();
    let mut itb = base.components();

    loop {
        match (ita.next(), itb.next()) {
            (Some(a), Some(b)) if a == b => (),
            (some_a, _) => {
                // Ignoring b, because 1 component is the filename
                // for which we don't need to go back up for relative
                // filename to work.
                let mut new_path = PathBuf::new();
                for _ in itb {
                    new_path.push(Component::ParentDir);
                }
                if let Some(a) = some_a {
                    new_path.push(a);
                    for comp in ita {
                        new_path.push(comp);
                    }
                }
                return new_path;
            }
        }
    }
}

fn symlink_or_rename(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    if dst.exists() {
        return Ok(());
    }

    let rel_src = make_relative(src, dst);
    #[cfg(target_os = "windows")]
    {
        if std::os::windows::fs::symlink_file(rel_src, dst).is_err() {
            std::fs::rename(src, dst)?;
        }
    }

    #[cfg(target_family = "unix")]
    std::os::unix::fs::symlink(rel_src, dst)?;

    Ok(())
}

fn jitter() -> usize {
    rand::rng().random_range(0..=500)
}

fn exponential_backoff(base_wait_time: usize, n: usize, max: usize) -> usize {
    (base_wait_time + n.pow(2) + jitter()).min(max)
}

impl Api {
    /// Creates a default Api, for Api options See [`ApiBuilder`]
    pub fn new() -> Result<Self, ApiError> {
        ApiBuilder::new().build()
    }

    /// Get the underlying api client
    /// Allows for lower level access
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get metadata for the file at the given URL.
    pub async fn metadata(&self, url: &str) -> Result<Metadata, ApiError> {
        let response = self
            .relative_redirect_client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .send()
            .await?;
        let response = response.error_for_status()?;
        let headers = response.headers();
        let header_commit = HeaderName::from_static("x-repo-commit");
        let header_linked_etag = HeaderName::from_static("x-linked-etag");
        let header_etag = HeaderName::from_static("etag");

        let etag = match headers.get(&header_linked_etag) {
            Some(etag) => etag,
            None => headers
                .get(&header_etag)
                .ok_or(ApiError::MissingHeader(header_etag))?,
        };
        // Cleaning extra quotes
        let etag = etag.to_str()?.to_string().replace('"', "");
        let commit_hash = headers
            .get(&header_commit)
            .ok_or(ApiError::MissingHeader(header_commit))?
            .to_str()?
            .to_string();

        // The response was redirected to S3 most likely which will
        // know about the size of the file
        let response = if response.status().is_redirection() {
            self.client
                .get(headers.get(LOCATION).unwrap().to_str()?.to_string())
                .header(RANGE, "bytes=0-0")
                .send()
                .await?
        } else {
            response
        };
        let headers = response.headers();
        let content_range = headers
            .get(CONTENT_RANGE)
            .ok_or(ApiError::MissingHeader(CONTENT_RANGE))?
            .to_str()?;

        let size = content_range
            .split('/')
            .next_back()
            .ok_or(ApiError::InvalidHeader(CONTENT_RANGE))?
            .parse()?;
        Ok(Metadata {
            commit_hash,
            etag,
            size,
        })
    }

    /// Creates a new handle [`ApiRepo`] which contains operations
    /// on a particular [`Repo`]
    pub fn repo(&self, repo: Repo) -> ApiRepo {
        ApiRepo::new(self.clone(), repo)
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{api::tokio::Api, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let api = Api::new().unwrap();
    /// let api = api.repo(Repo::new(model_id, RepoType::Model));
    /// ```
    pub fn model(&self, model_id: String) -> ApiRepo {
        self.repo(Repo::new(model_id, RepoType::Model))
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{api::tokio::Api, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let api = Api::new().unwrap();
    /// let api = api.repo(Repo::new(model_id, RepoType::Dataset));
    /// ```
    pub fn dataset(&self, model_id: String) -> ApiRepo {
        self.repo(Repo::new(model_id, RepoType::Dataset))
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{api::tokio::Api, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let api = Api::new().unwrap();
    /// let api = api.repo(Repo::new(model_id, RepoType::Space));
    /// ```
    pub fn space(&self, model_id: String) -> ApiRepo {
        self.repo(Repo::new(model_id, RepoType::Space))
    }
}

/// Shorthand for accessing things within a particular repo
/// You can inspect repos with [`ApiRepo::info`]
/// or download files with [`ApiRepo::download`]
#[derive(Debug)]
pub struct ApiRepo {
    api: Api,
    repo: Repo,
}

impl ApiRepo {
    fn new(api: Api, repo: Repo) -> Self {
        Self { api, repo }
    }
}

impl ApiRepo {
    /// Get the fully qualified URL of the remote filename
    /// ```
    /// # use hf_hub::api::tokio::Api;
    /// let api = Api::new().unwrap();
    /// let url = api.model("gpt2".to_string()).url("model.safetensors");
    /// assert_eq!(url, "https://huggingface.co/gpt2/resolve/main/model.safetensors");
    /// ```
    pub fn url(&self, filename: &str) -> String {
        let endpoint = &self.api.endpoint;
        let revision = &self.repo.url_revision();
        let repo_id = self.repo.url();
        format!("{endpoint}/{repo_id}/resolve/{revision}/{filename}")
    }

    async fn download_tempfile<P: Progress + Clone + Send + Sync + 'static>(
        &self,
        url: &str,
        length: usize,
        filename: PathBuf,
        mut progressbar: P,
    ) -> Result<PathBuf, ApiError> {
        let semaphore = Arc::new(Semaphore::new(self.api.max_files));
        let parallel_failures_semaphore = Arc::new(Semaphore::new(self.api.parallel_failures));

        // Create the file and set everything properly
        const N_BYTES: usize = size_of::<u64>();

        let start = match tokio::fs::OpenOptions::new()
            .read(true)
            .open(&filename)
            .await
        {
            Ok(mut f) => {
                let len = f.metadata().await?.len();
                if len == (length + N_BYTES) as u64 {
                    f.seek(SeekFrom::Start(length as u64)).await?;
                    let mut buf = [0u8; N_BYTES];
                    let n = f.read(buf.as_mut_slice()).await?;
                    if n == N_BYTES {
                        let committed = u64::from_le_bytes(buf);
                        committed as usize
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
            Err(_err) => {
                tokio::fs::File::create(&filename)
                    .await?
                    .set_len((length + N_BYTES) as u64)
                    .await?;
                0
            }
        };
        progressbar.update(start).await;

        let chunk_size = self.api.chunk_size.unwrap_or(length);
        let n_chunks = length / chunk_size;
        let mut handles = Vec::with_capacity(n_chunks);
        for start in (start..length).step_by(chunk_size) {
            let url = url.to_string();
            let filename = filename.clone();
            let client = self.api.client.clone();

            let stop = std::cmp::min(start + chunk_size - 1, length);
            let permit = semaphore.clone();
            let parallel_failures = self.api.parallel_failures;
            let max_retries = self.api.max_retries;
            let parallel_failures_semaphore = parallel_failures_semaphore.clone();
            let progress = progressbar.clone();
            handles.push(tokio::spawn(async move {
                let permit = permit.acquire_owned().await?;
                let mut chunk =
                    Self::download_chunk(&client, &url, &filename, start, stop, progress.clone())
                        .await;
                let mut i = 0;
                if parallel_failures > 0 {
                    while let Err(dlerr) = chunk {
                        let parallel_failure_permit =
                            parallel_failures_semaphore.clone().try_acquire_owned()?;

                        let wait_time = exponential_backoff(300, i, 10_000);
                        tokio::time::sleep(tokio::time::Duration::from_millis(wait_time as u64))
                            .await;

                        chunk = Self::download_chunk(
                            &client,
                            &url,
                            &filename,
                            start,
                            stop,
                            progress.clone(),
                        )
                        .await;
                        i += 1;
                        if i > max_retries {
                            return Err(ApiError::TooManyRetries(dlerr.into()));
                        }
                        drop(parallel_failure_permit);
                    }
                }
                drop(permit);
                chunk
            }));
        }

        let mut futures: FuturesUnordered<_> = handles.into_iter().collect();
        let mut temporaries = BinaryHeap::new();
        let mut committed: u64 = start as u64;
        while let Some(chunk) = futures.next().await {
            let chunk = chunk?;
            let (start, stop) = chunk?;
            temporaries.push(Reverse((start, stop)));

            let mut modified = false;
            while let Some(Reverse((min, max))) = temporaries.pop() {
                if min as u64 == committed {
                    committed = max as u64 + 1;
                    modified = true;
                } else {
                    temporaries.push(Reverse((min, max)));
                    break;
                }
            }
            if modified {
                let mut file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&filename)
                    .await?;
                file.seek(SeekFrom::Start(length as u64)).await?;
                file.write_all(&committed.to_le_bytes()).await?;
                file.flush().await?;
            }
        }
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&filename)
            .await?;
        f.set_len(length as u64).await?;
        // XXX Extremely important and not obvious.
        // Tokio::fs doesn't guarantee data is written at the end of `.await`
        // boundaries. Even though we await the `set_len` it may not have been
        // committed to disk, leading to invalid rename.
        // Forcing a flush forces the data (here the truncation) to be committed to disk
        f.flush().await?;

        progressbar.finish().await;
        Ok(filename)
    }

    async fn download_chunk<P>(
        client: &reqwest::Client,
        url: &str,
        filename: &PathBuf,
        start: usize,
        stop: usize,
        mut progress: P,
    ) -> Result<(usize, usize), ApiError>
    where
        P: Progress,
    {
        // Process each socket concurrently.
        let range = format!("bytes={start}-{stop}");
        let response = client
            .get(url)
            .header(RANGE, range)
            .send()
            .await?
            .error_for_status()?;
        let mut byte_stream = response.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(stop - start);
        while let Some(next) = byte_stream.next().await {
            let next = next?;
            buf.extend(&next);
            progress.update(next.len()).await;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(filename)
            .await?;
        file.seek(SeekFrom::Start(start as u64)).await?;
        file.write_all(&buf).await?;
        file.flush().await?;
        Ok((start, stop))
    }

    /// This will attempt the fetch the file locally first, then [`Api.download`]
    /// if the file is not present.
    /// ```no_run
    /// # use hf_hub::api::tokio::Api;
    /// # tokio_test::block_on(async {
    /// let api = Api::new().unwrap();
    /// let local_filename = api.model("gpt2".to_string()).get("model.safetensors").await.unwrap();
    /// # })
    pub async fn get(&self, filename: &str) -> Result<PathBuf, ApiError> {
        if let Some(path) = self.api.cache.repo(self.repo.clone()).get(filename) {
            Ok(path)
        } else {
            self.download(filename).await
        }
    }

    /// Downloads a remote file into the cache directory
    /// to be used locally.
    /// ```no_run
    /// # use hf_hub::api::tokio::Api;
    /// # tokio_test::block_on(async {
    /// let api = Api::new().unwrap();
    /// let local_filename = api.model("gpt2".to_string()).download("model.safetensors").await.unwrap();
    /// # })
    /// ```
    pub async fn download(&self, filename: &str) -> Result<PathBuf, ApiError> {
        if self.api.progress {
            self.download_with_progress(filename, ProgressBar::new(0))
                .await
        } else {
            self.download_with_progress(filename, ()).await
        }
    }

    /// This function is used to download a file with a custom progress function.
    /// It uses the [`Progress`] trait and can be used in more complex use
    /// cases like downloading a showing progress in a UI.
    /// ```no_run
    /// use hf_hub::api::tokio::{Api, Progress};
    ///
    /// #[derive(Clone)]
    /// struct MyProgress{
    ///     current: usize,
    ///     total: usize
    /// }
    ///
    /// impl Progress for MyProgress{
    ///     async fn init(&mut self, size: usize, _filename: &str){
    ///         self.total = size;
    ///         self.current = 0;
    ///     }
    ///
    ///     async fn update(&mut self, size: usize){
    ///         self.current += size;
    ///         println!("{}/{}", self.current, self.total)
    ///     }
    ///
    ///     async fn finish(&mut self){
    ///         println!("Done !");
    ///     }
    /// }
    /// # tokio_test::block_on(async {
    /// let api = Api::new().unwrap();
    /// let progress = MyProgress{ current: 0, total : 0};
    /// let local_filename = api.model("gpt2".to_string()).download_with_progress("model.safetensors", progress).await.unwrap();
    /// # })
    /// ```
    pub async fn download_with_progress<P: Progress + Clone + Send + Sync + 'static>(
        &self,
        filename: &str,
        mut progress: P,
    ) -> Result<PathBuf, ApiError> {
        let url = self.url(filename);
        let metadata = self.api.metadata(&url).await?;
        let cache = self.api.cache.repo(self.repo.clone());

        let blob_path = cache.blob_path(&metadata.etag);
        std::fs::create_dir_all(blob_path.parent().unwrap())?;

        let lock = lock_file(blob_path.clone()).await?;
        progress.init(metadata.size, filename).await;
        let mut tmp_path = blob_path.clone();
        tmp_path.set_extension(EXTENSION);
        let tmp_filename = self
            .download_tempfile(&url, metadata.size, tmp_path, progress)
            .await?;

        tokio::fs::rename(&tmp_filename, &blob_path).await?;
        drop(lock);

        let mut pointer_path = cache.pointer_path(&metadata.commit_hash);
        pointer_path.push(filename);
        std::fs::create_dir_all(pointer_path.parent().unwrap()).ok();

        symlink_or_rename(&blob_path, &pointer_path)?;
        cache.create_ref(&metadata.commit_hash)?;

        Ok(pointer_path)
    }

    /// Get information about the Repo
    /// ```
    /// # use hf_hub::api::tokio::Api;
    /// # tokio_test::block_on(async {
    /// let api = Api::new().unwrap();
    /// api.model("gpt2".to_string()).info();
    /// # })
    /// ```
    pub async fn info(&self) -> Result<RepoInfo, ApiError> {
        Ok(self
            .info_request()
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Get the raw [`reqwest::RequestBuilder`] with the url and method already set
    /// ```
    /// # use hf_hub::api::tokio::Api;
    /// # tokio_test::block_on(async {
    /// let api = Api::new().unwrap();
    /// api.model("gpt2".to_owned())
    ///     .info_request()
    ///     .query(&[("blobs", "true")])
    ///     .send()
    ///     .await;
    /// # })
    /// ```
    pub fn info_request(&self) -> RequestBuilder {
        let url = format!("{}/api/{}", self.api.endpoint, self.repo.api_url());
        self.api.client.get(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Siblings;
    use crate::assert_no_diff;
    use core::panic;
    use hex_literal::hex;
    use rand::distr::Alphanumeric;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::io::{Seek, Write};
    use std::time::Duration;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new() -> Self {
            let s: String = rand::rng()
                .sample_iter(&Alphanumeric)
                .take(7)
                .map(char::from)
                .collect();
            let mut path = std::env::temp_dir();
            path.push(s);
            std::fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.path).unwrap();
        }
    }

    #[tokio::test]
    async fn simple() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let model_id = "julien-c/dummy-unknown".to_string();
        let repo = Repo::new(model_id.clone(), RepoType::Model);
        let downloaded_path = api.model(model_id).download("config.json").await.unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        // Make sure the file is now seeable without connection
        let cache_path = api.cache.repo(repo.clone()).get("config.json").unwrap();
        assert_eq!(cache_path, downloaded_path);
    }

    #[tokio::test]
    async fn locking() {
        use std::sync::Arc;
        use tokio::sync::Mutex;
        use tokio::task::JoinSet;
        let tmp = Arc::new(Mutex::new(TempDir::new()));

        let mut handles = JoinSet::new();
        for _ in 0..5 {
            let tmp2 = tmp.clone();
            handles.spawn(async move {
                let api = ApiBuilder::new()
                    .with_progress(false)
                    .with_cache_dir(tmp2.lock().await.path.clone())
                    .build()
                    .unwrap();

                // 0..256ms sleep to randomize potential clashes
                let millis: u64 = rand::random::<u8>().into();
                tokio::time::sleep(Duration::from_millis(millis)).await;
                let model_id = "julien-c/dummy-unknown".to_string();
                api.model(model_id.clone())
                    .download("config.json")
                    .await
                    .unwrap()
            });
        }
        while let Some(handle) = handles.join_next().await {
            let downloaded_path = handle.unwrap();
            assert!(downloaded_path.exists());
            let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
            assert_eq!(
                val[..],
                hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
            );
        }
    }

    #[tokio::test]
    async fn resume() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let model_id = "julien-c/dummy-unknown".to_string();
        let downloaded_path = api
            .model(model_id.clone())
            .download("config.json")
            .await
            .unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        // This actually sets the file to a trashed version of the part file, full redownload will
        // ensue
        let blob = std::fs::canonicalize(&downloaded_path).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&blob).unwrap();
        let size = file.metadata().unwrap().len();
        let truncate: f32 = rand::random();
        let new_size = (size as f32 * truncate) as u64;
        file.set_len(new_size).unwrap();
        let mut blob_part = blob.clone();
        blob_part.set_extension("sync.part");
        std::fs::rename(blob, &blob_part).unwrap();
        std::fs::remove_file(&downloaded_path).unwrap();
        let content = std::fs::read(&*blob_part).unwrap();
        assert_eq!(content.len() as u64, new_size);
        let val = Sha256::digest(content);
        // We modified the sha.
        assert!(
            val[..] != hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );
        let new_downloaded_path = api
            .model(model_id.clone())
            .download("config.json")
            .await
            .unwrap();
        let val = Sha256::digest(std::fs::read(&*new_downloaded_path).unwrap());
        assert_eq!(downloaded_path, new_downloaded_path);
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        // Now this is a valid partial download file
        let blob = std::fs::canonicalize(&downloaded_path).unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).open(&blob).unwrap();
        let size = file.metadata().unwrap().len();
        let truncate: f32 = rand::random();
        let new_size = (size as f32 * truncate) as u64;
        // Truncating
        file.set_len(new_size).unwrap();
        let total_size = size + size_of::<u64>() as u64;
        file.set_len(total_size).unwrap();
        file.seek(SeekFrom::Start(size)).unwrap();
        file.write_all(&new_size.to_le_bytes()).unwrap();

        let mut blob_part = blob.clone();
        blob_part.set_extension("sync.part");
        std::fs::rename(blob, &blob_part).unwrap();
        std::fs::remove_file(&downloaded_path).unwrap();
        let content = std::fs::read(&*blob_part).unwrap();
        assert_eq!(content.len() as u64, total_size);
        let val = Sha256::digest(content);
        // We modified the sha.
        assert!(
            val[..] != hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );
        let new_downloaded_path = api
            .model(model_id.clone())
            .download("config.json")
            .await
            .unwrap();
        let val = Sha256::digest(std::fs::read(&*new_downloaded_path).unwrap());
        assert_eq!(downloaded_path, new_downloaded_path);
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        // Here we prove the previous part was correctly resuming by purposefully corrupting the
        // file.
        let blob = std::fs::canonicalize(&downloaded_path).unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).open(&blob).unwrap();
        let size = file.metadata().unwrap().len();
        // Not random for consistent sha corruption
        let truncate: f32 = 0.5;
        let new_size = (size as f32 * truncate) as u64;
        // Truncating
        file.set_len(new_size).unwrap();
        let total_size = size + size_of::<u64>() as u64;
        file.set_len(total_size).unwrap();
        file.seek(SeekFrom::Start(size)).unwrap();
        file.write_all(&new_size.to_le_bytes()).unwrap();

        // Corrupting by changing a single byte.
        file.seek(SeekFrom::Start(new_size - 1)).unwrap();
        file.write_all(&[0]).unwrap();

        let mut blob_part = blob.clone();
        blob_part.set_extension("sync.part");
        std::fs::rename(blob, &blob_part).unwrap();
        std::fs::remove_file(&downloaded_path).unwrap();
        let content = std::fs::read(&*blob_part).unwrap();
        assert_eq!(content.len() as u64, total_size);
        let val = Sha256::digest(content);
        // We modified the sha.
        assert!(
            val[..] != hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );
        let new_downloaded_path = api
            .model(model_id.clone())
            .download("config.json")
            .await
            .unwrap();
        let val = Sha256::digest(std::fs::read(&*new_downloaded_path).unwrap());
        assert_eq!(downloaded_path, new_downloaded_path);
        assert_eq!(
            val[..],
            // Corrupted sha
            hex!("32b83c94ee55a8d43d68b03a859975f6789d647342ddeb2326fcd5e0127035b5")
        );
    }

    #[tokio::test]
    async fn revision() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let model_id = "BAAI/bge-base-en".to_string();
        let repo = Repo::with_revision(model_id.clone(), RepoType::Model, "refs/pr/2".to_string());
        let downloaded_path = api
            .repo(repo.clone())
            .download("tokenizer.json")
            .await
            .unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66")
        );

        // Make sure the file is now seeable without connection
        let cache_path = api.cache.repo(repo).get("tokenizer.json").unwrap();
        assert_eq!(cache_path, downloaded_path);
    }

    #[tokio::test]
    async fn dataset() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let repo = Repo::with_revision(
            "wikitext".to_string(),
            RepoType::Dataset,
            "refs/convert/parquet".to_string(),
        );
        let downloaded_path = api
            .repo(repo)
            .download("wikitext-103-v1/test/0000.parquet")
            .await
            .unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("ABDFC9F83B1103B502924072460D4C92F277C9B49C313CEF3E48CFCF7428E125")
        );
    }

    #[tokio::test]
    async fn models() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let repo = Repo::with_revision(
            "BAAI/bGe-reRanker-Base".to_string(),
            RepoType::Model,
            "refs/pr/5".to_string(),
        );
        let downloaded_path = api.repo(repo).download("tokenizer.json").await.unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("9EB652AC4E40CC093272BBBE0F55D521CF67570060227109B5CDC20945A4489E")
        );
    }

    #[tokio::test]
    async fn info() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let repo = Repo::with_revision(
            "wikitext".to_string(),
            RepoType::Dataset,
            "refs/convert/parquet".to_string(),
        );
        let model_info = api.repo(repo).info().await.unwrap();
        assert_eq!(
            model_info,
            RepoInfo {
                siblings: vec![
                    Siblings {
                        rfilename: ".gitattributes".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-raw-v1/test/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-raw-v1/train/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-raw-v1/train/0001.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-raw-v1/validation/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-v1/test/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-v1/train/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-v1/train/0001.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-103-v1/validation/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-2-raw-v1/test/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-2-raw-v1/train/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-2-raw-v1/validation/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-2-v1/test/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-2-v1/train/0000.parquet".to_string()
                    },
                    Siblings {
                        rfilename: "wikitext-2-v1/validation/0000.parquet".to_string()
                    }
                ],
                sha: "3f68cd45302c7b4b532d933e71d9e6e54b1c7d5e".to_string()
            }
        );
    }

    #[tokio::test]
    async fn info_request() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_token(None)
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();
        let repo = Repo::with_revision(
            "mcpotato/42-eicar-street".to_string(),
            RepoType::Model,
            "8b3861f6931c4026b0cd22b38dbc09e7668983ac".to_string(),
        );
        let blobs_info: Value = api
            .repo(repo)
            .info_request()
            .query(&[("blobs", "true")])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_no_diff!(
            blobs_info,
            json!({
                "_id": "621ffdc136468d709f17ddb4",
                "author": "mcpotato",
                "config": {},
                "createdAt": "2022-03-02T23:29:05.000Z",
                "disabled": false,
                "downloads": 0,
                "gated": false,
                "id": "mcpotato/42-eicar-street",
                "lastModified": "2022-11-30T19:54:16.000Z",
                "likes": 4,
                "modelId": "mcpotato/42-eicar-street",
                "private": false,
                "sha": "8b3861f6931c4026b0cd22b38dbc09e7668983ac",
                "siblings": [
                    {
                        "blobId": "6d34772f5ca361021038b404fb913ec8dc0b1a5a",
                        "rfilename": ".gitattributes",
                        "size": 1175
                    },
                    {
                        "blobId": "be98037f7c542112c15a1d2fc7e2a2427e42cb50",
                        "rfilename": "build_pickles.py",
                        "size": 304
                    },
                    {
                        "blobId": "8acd02161fff53f9df9597e377e22b04bc34feff",
                        "rfilename": "danger.dat",
                        "size": 66
                    },
                    {
                        "blobId": "86b812515e075a1ae216e1239e615a1d9e0b316e",
                        "rfilename": "eicar_test_file",
                        "size": 70
                    },
                    {
                        "blobId": "86b812515e075a1ae216e1239e615a1d9e0b316e",
                        "rfilename": "eicar_test_file_bis",
                        "size":70
                    },
                    {
                        "blobId": "cd1c6d8bde5006076655711a49feae66f07d707e",
                        "lfs": {
                            "pointerSize": 127,
                            "sha256": "f9343d7d7ec5c3d8bcced056c438fc9f1d3819e9ca3d42418a40857050e10e20",
                            "size": 22
                        },
                        "rfilename": "pytorch_model.bin",
                        "size": 22
                    },
                    {
                        "blobId": "8ab39654695136173fee29cba0193f679dfbd652",
                        "rfilename": "supposedly_safe.pkl",
                        "size": 31
                    }
                ],
                "spaces": ["szk2024/est"],
                "tags": ["pytorch", "region:us"],
                "usedStorage": 22
            })
        );
    }

    #[test]
    fn headers_default() {
        let headers = ApiBuilder::new().build_headers().unwrap();
        assert_eq!(
            headers.get(USER_AGENT),
            Some(
                &"unknown/None; hf-hub/0.4.3; rust/unknown"
                    .try_into()
                    .unwrap()
            )
        );
    }

    #[test]
    fn headers_custom() {
        let headers = ApiBuilder::new()
            .with_user_agent("origin", "custom")
            .build_headers()
            .unwrap();
        assert_eq!(
            headers.get(USER_AGENT),
            Some(
                &"unknown/None; hf-hub/0.4.3; rust/unknown; origin/custom"
                    .try_into()
                    .unwrap()
            )
        );
    }

    #[tokio::test]
    async fn repo_info_failed() {
        let api = ApiBuilder::new()
            .with_progress(true)
            // Use an empty token to trigger an authorization error.
            .with_token(Some("".to_string()))
            .build()
            .expect("failed to build API");

        let repo = api.model("danieldk/private-test".to_string());
        let error = repo
            .info()
            .await
            .expect_err("failed to raise authentication error");
        match error {
            ApiError::RequestError(error) if error.is_status() => {}
            _ => panic!("unexpected request error"),
        }
    }

    // #[tokio::test]
    // async fn real() {
    //     let api = Api::new().unwrap();
    //     let repo = api.model("bert-base-uncased".to_string());
    //     let weights = repo.get("model.safetensors").await.unwrap();
    //     let val = Sha256::digest(std::fs::read(&*weights).unwrap());
    //     println!("Digest {val:#x}");
    //     assert_eq!(
    //         val[..],
    //         hex!("68d45e234eb4a928074dfd868cead0219ab85354cc53d20e772753c6bb9169d3")
    //     );
    // }

    #[tokio::test]
    async fn redirect_test() {
        // This test requires a valid HF_TOKEN with gate access to this llama model.
        if let Ok(token) = std::env::var("HF_TOKEN") {
            let api = ApiBuilder::new().with_token(Some(token)).build().unwrap();
            let repo = api.model("meta-llama/Llama-3.1-8B".to_string());
            repo.download("config.json").await.unwrap();

            // with redirect
            let repo = api.model("meta-llama/Meta-Llama-3.1-8B".to_string());
            repo.download("config.json").await.unwrap();
        }
    }
}
