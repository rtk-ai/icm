use super::{RepoInfo, HF_ENDPOINT};
use crate::api::sync::ApiError::InvalidHeader;
use crate::api::Progress;
use crate::{Cache, Repo, RepoType};
use http::{StatusCode, Uri};
use indicatif::ProgressBar;
use rand::Rng;
use std::collections::HashMap;
use std::io::Read;
use std::io::Seek;
use std::num::ParseIntError;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use thiserror::Error;
use ureq::config::ConfigBuilder;
use ureq::config::RedirectAuthHeaders;
use ureq::tls::{TlsConfig, TlsProvider};
use ureq::typestate::{AgentScope, WithoutBody};
use ureq::{Agent, RequestBuilder};

/// Current version (used in user-agent)
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Current name (used in user-agent)
const NAME: &str = env!("CARGO_PKG_NAME");

const RANGE: &str = "Range";
const CONTENT_RANGE: &str = "Content-Range";
const LOCATION: &str = "Location";
const USER_AGENT: &str = "User-Agent";
const AUTHORIZATION: &str = "Authorization";

type HeaderMap = HashMap<&'static str, String>;
type HeaderName = &'static str;

/// Specific name for the sync part of the resumable file
const EXTENSION: &str = "part";

struct Wrapper<'a, P: Progress, R: Read> {
    progress: &'a mut P,
    inner: R,
}

fn wrap_read<P: Progress, R: Read>(inner: R, progress: &mut P) -> Wrapper<'_, P, R> {
    Wrapper { inner, progress }
}

impl<P: Progress, R: Read> Read for Wrapper<'_, P, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.progress.update(read);
        Ok(read)
    }
}

/// Simple wrapper over [`ureq::Agent`] to include default headers
#[derive(Clone, Debug)]
pub struct HeaderAgent {
    agent: Agent,
    headers: HeaderMap,
}

impl HeaderAgent {
    fn new(agent: Agent, headers: HeaderMap) -> Self {
        Self { agent, headers }
    }

    fn get(&self, url: &str) -> RequestBuilder<WithoutBody> {
        let mut request = self.agent.get(url);
        for (header, value) in &self.headers {
            request = request.header(*header, value);
        }
        request
    }
}

struct Handle {
    file: std::fs::File,
}

impl Drop for Handle {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

fn lock_file(mut path: PathBuf) -> Result<Handle, ApiError> {
    path.set_extension("lock");

    let file = std::fs::File::create(path.clone())?;
    let mut res = lock(&file);
    for _ in 0..5 {
        if res == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
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

    pub(crate) fn lock(file: &std::fs::File) -> i32 {
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
    }
    pub(crate) fn unlock(file: &std::fs::File) -> i32 {
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

    pub(crate) fn lock(file: &std::fs::File) -> i32 {
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
    pub(crate) fn unlock(file: &std::fs::File) -> i32 {
        unsafe { UnlockFile(file.as_raw_handle() as HANDLE, 0, 0, !0, !0) }
    }
}
#[cfg(target_family = "windows")]
use windows::{lock, unlock};

#[cfg(not(any(target_family = "unix", target_family = "windows")))]
mod other {
    pub(crate) fn lock(file: &std::fs::File) -> i32 {
        0
    }
    pub(crate) fn unlock(file: &std::fs::File) -> i32 {
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

    // /// The value cannot be used as a header during request header construction
    // #[error("Invalid header value {0}")]
    // InvalidHeaderValue(#[from] InvalidHeaderValue),

    // /// The header value is not valid utf-8
    // #[error("header value is not a string")]
    // ToStr(#[from] ToStrError),
    /// Error in the request
    #[error("request error: {0}")]
    RequestError(#[from] Box<ureq::Error>),

    /// Error parsing some range value
    #[error("Cannot parse int")]
    ParseIntError(#[from] ParseIntError),

    /// I/O Error
    #[error("I/O error {0}")]
    IoError(#[from] std::io::Error),

    /// We tried to download chunk too many times
    #[error("Too many retries: {0}")]
    TooManyRetries(Box<ApiError>),

    /// Native tls error
    #[error("Native tls: {0}")]
    #[cfg(feature = "native-tls")]
    Native(#[from] native_tls::Error),

    /// The part file is corrupted
    #[error("Invalid part file - corrupted file")]
    InvalidResume,

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
    /// use hf_hub::api::sync::ApiBuilder;
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
    /// use hf_hub::api::sync::ApiBuilder;
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

    /// From a given cache
    /// ```
    /// use hf_hub::{api::sync::ApiBuilder, Cache};
    /// let path = std::path::PathBuf::from("/tmp");
    /// let cache = Cache::new(path);
    /// let api = ApiBuilder::from_cache(cache).build().unwrap();
    /// ```
    pub fn from_cache(cache: Cache) -> Self {
        let token = cache.token();

        let max_retries = 0;
        let progress = true;

        let endpoint = "https://huggingface.co".to_string();

        let user_agent = vec![
            ("unknown".to_string(), "None".to_string()),
            (NAME.to_string(), VERSION.to_string()),
            ("rust".to_string(), "unknown".to_string()),
        ];

        Self {
            endpoint,
            cache,
            token,
            max_retries,
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

    /// Sets the number of times the API will retry to download a file
    pub fn with_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Adds custom fields to headers user-agent
    pub fn with_user_agent(mut self, key: &str, value: &str) -> Self {
        self.user_agent.push((key.to_string(), value.to_string()));
        self
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let user_agent = self
            .user_agent
            .iter()
            .map(|(key, value)| format!("{key}/{value}"))
            .collect::<Vec<_>>()
            .join("; ");
        headers.insert(USER_AGENT, user_agent.to_string());
        if let Some(token) = &self.token {
            headers.insert(AUTHORIZATION, format!("Bearer {token}"));
        }
        headers
    }

    /// Consumes the builder and builds the final [`Api`]
    pub fn build(self) -> Result<Api, ApiError> {
        let headers = self.build_headers();

        let builder = builder()?.redirect_auth_headers(RedirectAuthHeaders::SameHost);
        let agent: Agent = builder.build().into();
        let client = HeaderAgent::new(agent, headers.clone());

        let no_redirect_agent: Agent = Agent::config_builder()
            // .try_proxy_from_env(true)
            .max_redirects(0)
            .build()
            .into();
        let no_redirect_client = HeaderAgent::new(no_redirect_agent, headers);

        Ok(Api {
            endpoint: self.endpoint,
            cache: self.cache,
            client,
            no_redirect_client,
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
    client: HeaderAgent,
    no_redirect_client: HeaderAgent,
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
    pub fn client(&self) -> &HeaderAgent {
        &self.client
    }

    /// Get metadata for the file at the given URL.
    pub fn metadata(&self, url: &str) -> Result<Metadata, ApiError> {
        let mut response = self
            .no_redirect_client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .call()
            .map_err(Box::new)?;

        // Closure to check if status code is a redirection
        let should_redirect = |status_code: StatusCode| {
            matches!(
                status_code,
                StatusCode::MOVED_PERMANENTLY
                    | StatusCode::FOUND
                    | StatusCode::SEE_OTHER
                    | StatusCode::TEMPORARY_REDIRECT
                    | StatusCode::PERMANENT_REDIRECT
            )
        };

        // Follow redirects until `host.is_some()` i.e. only follow relative redirects
        // See: https://github.com/huggingface/huggingface_hub/blob/9c6af39cdce45b570f0b7f8fad2b311c96019804/src/huggingface_hub/file_download.py#L411
        let response = loop {
            // Check if redirect
            if should_redirect(response.status()) {
                // Get redirect location
                if let Some(location) = response.headers().get("Location") {
                    // Parse location
                    let uri = Uri::from_str(
                        std::str::from_utf8(location.as_bytes())
                            .map_err(|_| InvalidHeader("location"))?,
                    )
                    .map_err(|_| InvalidHeader("location"))?;

                    // Check if relative i.e. host is none
                    if uri.host().is_none() {
                        // Merge relative path with url
                        let mut parts = Uri::from_str(url).unwrap().into_parts();
                        parts.path_and_query = uri.into_parts().path_and_query;
                        // Final uri
                        let redirect_uri = Uri::from_parts(parts).unwrap();

                        // Follow redirect
                        response = self
                            .no_redirect_client
                            .get(&redirect_uri.to_string())
                            .header(RANGE, "bytes=0-0")
                            .call()
                            .map_err(Box::new)?;
                        continue;
                    }
                };
            }
            break response;
        };

        // let headers = response.headers();
        let header_commit = "x-repo-commit";
        let header_linked_etag = "x-linked-etag";
        let header_etag = "etag";

        let etag = match response.headers().get(header_linked_etag) {
            Some(etag) => etag,
            None => response
                .headers()
                .get(header_etag)
                .ok_or(ApiError::MissingHeader(header_etag))?,
        };
        // Cleaning extra quotes
        let etag = std::str::from_utf8(etag.as_bytes())
            .map_err(|_| ApiError::InvalidHeader("etag"))?
            .replace('"', "");
        let commit_hash = std::str::from_utf8(
            response
                .headers()
                .get(header_commit)
                .ok_or(ApiError::MissingHeader(header_commit))?
                .as_bytes(),
        )
        .map_err(|_| ApiError::InvalidHeader("commit_hash"))?
        .to_string();

        // The response was redirected to S3 most likely which will
        // know about the size of the file
        let status = response.status();
        let is_redirection = status.is_redirection();
        let response = if is_redirection {
            let location = response
                .headers()
                .get(LOCATION)
                .expect("location header in redirect");
            let location = std::str::from_utf8(location.as_bytes())
                .map_err(|_| ApiError::InvalidHeader("etag"))?;
            self.client
                .get(location)
                .header(RANGE, "bytes=0-0")
                .call()
                .map_err(Box::new)?
        } else {
            response
        };
        let content_range = response
            .headers()
            .get(CONTENT_RANGE)
            .ok_or(ApiError::MissingHeader(CONTENT_RANGE))?;
        let content_range = std::str::from_utf8(content_range.as_bytes())
            .map_err(|_| ApiError::InvalidHeader(CONTENT_RANGE))?;

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

    fn download_tempfile<P: Progress>(
        &self,
        url: &str,
        size: usize,
        mut progress: P,
        tmp_path: PathBuf,
        filename: &str,
    ) -> Result<PathBuf, ApiError> {
        progress.init(size, filename);
        let filepath = tmp_path;

        // Create the file and set everything properly

        let mut file = match std::fs::OpenOptions::new().append(true).open(&filepath) {
            Ok(f) => f,
            Err(_) => std::fs::File::create(&filepath)?,
        };

        // In case of resume.
        let start = file.metadata()?.len();
        if start > size as u64 {
            return Err(ApiError::InvalidResume);
        }

        let mut res = self.download_from(url, start, size, &mut file, filename, &mut progress);
        if self.max_retries > 0 {
            let mut i = 0;
            while let Err(dlerr) = res {
                let wait_time = exponential_backoff(300, i, 10_000);
                std::thread::sleep(std::time::Duration::from_millis(wait_time as u64));

                let current = file.stream_position()?;
                res = self.download_from(url, current, size, &mut file, filename, &mut progress);
                i += 1;
                if i > self.max_retries {
                    return Err(ApiError::TooManyRetries(dlerr.into()));
                }
            }
        }
        res?;
        Ok(filepath)
    }

    fn download_from<P>(
        &self,
        url: &str,
        current: u64,
        size: usize,
        file: &mut std::fs::File,
        filename: &str,
        progress: &mut P,
    ) -> Result<(), ApiError>
    where
        P: Progress,
    {
        let range = format!("bytes={current}-");
        let response = self
            .client
            .get(url)
            .header(RANGE, &range)
            .call()
            .map_err(Box::new)?;
        let (_, body) = response.into_parts();
        let reader = body.into_reader();
        progress.init(size, filename);
        progress.update(current as usize);
        let mut reader = Box::new(wrap_read(reader, progress));
        std::io::copy(&mut reader, file)?;
        progress.finish();
        Ok(())
    }

    /// Creates a new handle [`ApiRepo`] which contains operations
    /// on a particular [`Repo`]
    pub fn repo(&self, repo: Repo) -> ApiRepo {
        ApiRepo::new(self.clone(), repo)
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{api::sync::Api, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let api = Api::new().unwrap();
    /// let api = api.repo(Repo::new(model_id, RepoType::Model));
    /// ```
    pub fn model(&self, model_id: String) -> ApiRepo {
        self.repo(Repo::new(model_id, RepoType::Model))
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{api::sync::Api, Repo, RepoType};
    /// # let model_id = "gpt2".to_string();
    /// let api = Api::new().unwrap();
    /// let api = api.repo(Repo::new(model_id, RepoType::Dataset));
    /// ```
    pub fn dataset(&self, model_id: String) -> ApiRepo {
        self.repo(Repo::new(model_id, RepoType::Dataset))
    }

    /// Simple wrapper over
    /// ```
    /// # use hf_hub::{api::sync::Api, Repo, RepoType};
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

#[cfg(feature = "native-tls")]
fn builder() -> Result<ConfigBuilder<AgentScope>, ApiError> {
    Ok(Agent::config_builder().tls_config(
        TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .build(),
    ))
}

#[cfg(not(feature = "native-tls"))]
fn builder() -> Result<ConfigBuilder<AgentScope>, ApiError> {
    Ok(Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::Rustls).build()))
}

impl ApiRepo {
    /// Get the fully qualified URL of the remote filename
    /// ```
    /// # use hf_hub::api::sync::Api;
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

    /// This will attempt the fetch the file locally first, then [`Api.download`]
    /// if the file is not present.
    /// ```no_run
    /// use hf_hub::{api::sync::Api};
    /// let api = Api::new().unwrap();
    /// let local_filename = api.model("gpt2".to_string()).get("model.safetensors").unwrap();
    pub fn get(&self, filename: &str) -> Result<PathBuf, ApiError> {
        if let Some(path) = self.api.cache.repo(self.repo.clone()).get(filename) {
            Ok(path)
        } else {
            self.download(filename)
        }
    }

    /// This function is used to download a file with a custom progress function.
    /// It uses the [`Progress`] trait and can be used in more complex use
    /// cases like downloading a showing progress in a UI.
    /// ```no_run
    /// # use hf_hub::api::{sync::Api, Progress};
    /// struct MyProgress{
    ///     current: usize,
    ///     total: usize
    /// }
    ///
    /// impl Progress for MyProgress{
    ///     fn init(&mut self, size: usize, _filename: &str){
    ///         self.total = size;
    ///         self.current = 0;
    ///     }
    ///
    ///     fn update(&mut self, size: usize){
    ///         self.current += size;
    ///         println!("{}/{}", self.current, self.total)
    ///     }
    ///
    ///     fn finish(&mut self){
    ///         println!("Done !");
    ///     }
    /// }
    /// let api = Api::new().unwrap();
    /// let progress = MyProgress{current: 0, total: 0};
    /// let local_filename = api.model("gpt2".to_string()).download_with_progress("model.safetensors", progress).unwrap();
    /// ```
    pub fn download_with_progress<P: Progress>(
        &self,
        filename: &str,
        progress: P,
    ) -> Result<PathBuf, ApiError> {
        let url = self.url(filename);
        let metadata = self.api.metadata(&url)?;

        let blob_path = self
            .api
            .cache
            .repo(self.repo.clone())
            .blob_path(&metadata.etag);
        std::fs::create_dir_all(blob_path.parent().unwrap())?;

        let lock = lock_file(blob_path.clone())?;
        let mut tmp_path = blob_path.clone();
        tmp_path.set_extension(EXTENSION);
        let tmp_filename =
            self.api
                .download_tempfile(&url, metadata.size, progress, tmp_path, filename)?;

        std::fs::rename(tmp_filename, &blob_path)?;
        drop(lock);

        let mut pointer_path = self
            .api
            .cache
            .repo(self.repo.clone())
            .pointer_path(&metadata.commit_hash);
        pointer_path.push(filename);
        std::fs::create_dir_all(pointer_path.parent().unwrap()).ok();

        symlink_or_rename(&blob_path, &pointer_path)?;
        self.api
            .cache
            .repo(self.repo.clone())
            .create_ref(&metadata.commit_hash)?;

        assert!(pointer_path.exists());

        Ok(pointer_path)
    }

    /// Downloads a remote file into the cache directory
    /// to be used locally.
    /// ```no_run
    /// # use hf_hub::api::sync::Api;
    /// let api = Api::new().unwrap();
    /// let local_filename = api.model("gpt2".to_string()).download("model.safetensors").unwrap();
    /// ```
    pub fn download(&self, filename: &str) -> Result<PathBuf, ApiError> {
        if self.api.progress {
            self.download_with_progress(filename, ProgressBar::new(0))
        } else {
            self.download_with_progress(filename, ())
        }
    }

    /// Get information about the Repo
    /// ```
    /// use hf_hub::{api::sync::Api};
    /// let api = Api::new().unwrap();
    /// api.model("gpt2".to_string()).info();
    /// ```
    pub fn info(&self) -> Result<RepoInfo, ApiError> {
        let mut response = self.info_request().call().map_err(Box::new)?;

        Ok(response.body_mut().read_json().map_err(Box::new)?)
    }

    /// Get the raw [`Request`] with the url and method already set
    /// ```
    /// # use hf_hub::api::sync::Api;
    /// let api = Api::new().unwrap();
    /// api.model("gpt2".to_owned())
    ///     .info_request()
    ///     .query("blobs", "true")
    ///     .call();
    /// ```
    pub fn info_request(&self) -> RequestBuilder<WithoutBody> {
        let url = format!("{}/api/{}", self.api.endpoint, self.repo.api_url());
        self.api.client.get(&url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Siblings;
    use crate::assert_no_diff;
    use hex_literal::hex;
    use rand::{distr::Alphanumeric, Rng};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::io::{Seek, SeekFrom, Write};
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
            std::fs::remove_dir_all(&self.path).unwrap()
        }
    }

    #[test]
    fn simple() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();

        let model_id = "julien-c/dummy-unknown".to_string();
        let downloaded_path = api.model(model_id.clone()).download("config.json").unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        // Make sure the file is now seeable without connection
        let cache_path = api
            .cache
            .repo(Repo::new(model_id, RepoType::Model))
            .get("config.json")
            .unwrap();
        assert_eq!(cache_path, downloaded_path);
    }

    #[test]
    fn resume() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .build()
            .unwrap();

        let model_id = "julien-c/dummy-unknown".to_string();
        let downloaded_path = api.model(model_id.clone()).download("config.json").unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        let blob = std::fs::canonicalize(&downloaded_path).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&blob).unwrap();
        let size = file.metadata().unwrap().len();
        let truncate: f32 = rand::random();
        let new_size = (size as f32 * truncate) as u64;
        file.set_len(new_size).unwrap();
        let mut blob_part = blob.clone();
        blob_part.set_extension("part");
        std::fs::rename(blob, &blob_part).unwrap();
        std::fs::remove_file(&downloaded_path).unwrap();
        let content = std::fs::read(&*blob_part).unwrap();
        assert_eq!(content.len() as u64, new_size);
        let val = Sha256::digest(content);
        // We modified the sha.
        assert!(
            val[..] != hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );
        let new_downloaded_path = api.model(model_id.clone()).download("config.json").unwrap();
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
        // Corrupting by changing a single byte.
        file.seek(SeekFrom::Start(new_size - 1)).unwrap();
        file.write_all(&[0]).unwrap();

        let mut blob_part = blob.clone();
        blob_part.set_extension("part");
        std::fs::rename(blob, &blob_part).unwrap();
        std::fs::remove_file(&downloaded_path).unwrap();
        let content = std::fs::read(&*blob_part).unwrap();
        assert_eq!(content.len() as u64, new_size);
        let val = Sha256::digest(content);
        // We modified the sha.
        assert!(
            val[..] != hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );
        let new_downloaded_path = api.model(model_id.clone()).download("config.json").unwrap();
        let val = Sha256::digest(std::fs::read(&*new_downloaded_path).unwrap());
        assert_eq!(downloaded_path, new_downloaded_path);
        println!("{new_downloaded_path:?}");
        println!("Corrupted {val:#x}");
        assert_eq!(
            val[..],
            // Corrupted sha
            hex!("32b83c94ee55a8d43d68b03a859975f6789d647342ddeb2326fcd5e0127035b5")
        );
    }

    #[test]
    fn locking() {
        use std::sync::{Arc, Mutex};
        let tmp = Arc::new(Mutex::new(TempDir::new()));

        let mut handles = vec![];
        for _ in 0..5 {
            let tmp2 = tmp.clone();
            let f = std::thread::spawn(move || {
                // 0..256ms sleep to randomize potential clashes
                std::thread::sleep(Duration::from_millis(rand::random::<u8>().into()));
                let api = ApiBuilder::new()
                    .with_progress(false)
                    .with_cache_dir(tmp2.lock().unwrap().path.clone())
                    .build()
                    .unwrap();

                let model_id = "julien-c/dummy-unknown".to_string();
                api.model(model_id.clone()).download("config.json").unwrap()
            });
            handles.push(f);
        }
        while let Some(handle) = handles.pop() {
            let downloaded_path = handle.join().unwrap();
            assert!(downloaded_path.exists());
            let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
            assert_eq!(
                val[..],
                hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
            );
        }
    }

    #[test]
    fn simple_with_retries() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_cache_dir(tmp.path.clone())
            .with_retries(3)
            .build()
            .unwrap();

        let model_id = "julien-c/dummy-unknown".to_string();
        let downloaded_path = api.model(model_id.clone()).download("config.json").unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("b908f2b7227d4d31a2105dfa31095e28d304f9bc938bfaaa57ee2cacf1f62d32")
        );

        // Make sure the file is now seeable without connection
        let cache_path = api
            .cache
            .repo(Repo::new(model_id, RepoType::Model))
            .get("config.json")
            .unwrap();
        assert_eq!(cache_path, downloaded_path);
    }

    #[test]
    fn dataset() {
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
            .unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("ABDFC9F83B1103B502924072460D4C92F277C9B49C313CEF3E48CFCF7428E125")
        );
    }

    #[test]
    fn models() {
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
        let downloaded_path = api.repo(repo).download("tokenizer.json").unwrap();
        assert!(downloaded_path.exists());
        let val = Sha256::digest(std::fs::read(&*downloaded_path).unwrap());
        assert_eq!(
            val[..],
            hex!("9EB652AC4E40CC093272BBBE0F55D521CF67570060227109B5CDC20945A4489E")
        );
    }

    #[test]
    fn info() {
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
        let model_info = api.repo(repo).info().unwrap();
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

    #[test]
    fn detailed_info() {
        let tmp = TempDir::new();
        let api = ApiBuilder::new()
            .with_progress(false)
            .with_token(None)
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
            .query("blobs", "true")
            .call()
            .unwrap()
            .body_mut()
            .read_json()
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
    fn endpoint() {
        let api = ApiBuilder::new().build().unwrap();
        assert_eq!(api.endpoint, "https://huggingface.co".to_string());
        let fake_endpoint = "https://fake_endpoint.com".to_string();
        let api = ApiBuilder::new()
            .with_endpoint(fake_endpoint.clone())
            .build()
            .unwrap();
        assert_eq!(api.endpoint, fake_endpoint);
    }

    #[test]
    fn headers_with_token() {
        let api = ApiBuilder::new()
            .with_token(Some("token".to_string()))
            .build()
            .unwrap();
        let headers = api.client.headers;
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer token".to_string())
        );
    }

    #[test]
    fn headers_default() {
        let api = ApiBuilder::new().build().unwrap();
        let headers = api.client.headers;
        assert_eq!(
            headers.get(USER_AGENT),
            Some(&"unknown/None; hf-hub/0.4.3; rust/unknown".to_string())
        );
    }

    #[test]
    fn headers_custom() {
        let api = ApiBuilder::new()
            .with_user_agent("origin", "custom")
            .build()
            .unwrap();
        let headers = api.client.headers;
        assert_eq!(
            headers.get(USER_AGENT),
            Some(&"unknown/None; hf-hub/0.4.3; rust/unknown; origin/custom".to_string())
        );
    }

    // #[test]
    // fn real() {
    //     let api = Api::new().unwrap();
    //     let repo = api.model("bert-base-uncased".to_string());
    //     let weights = repo.get("model.safetensors").unwrap();
    //     let val = Sha256::digest(std::fs::read(&*weights).unwrap());
    //     assert_eq!(
    //         val[..],
    //         hex!("68d45e234eb4a928074dfd868cead0219ab85354cc53d20e772753c6bb9169d3")
    //     );
    // }

    #[test]
    fn redirect_test() {
        // This test requires a valid HF_TOKEN with gate access to this llama model.
        if let Ok(token) = std::env::var("HF_TOKEN") {
            let api = ApiBuilder::new().with_token(Some(token)).build().unwrap();
            let repo = api.model("meta-llama/Llama-3.1-8B".to_string());
            repo.download("config.json").unwrap();

            // with redirect
            let repo = api.model("meta-llama/Meta-Llama-3.1-8B".to_string());
            repo.download("config.json").unwrap();
        }
    }
}
