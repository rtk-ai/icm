//! RTK Cloud client for ICM — login, credentials, and memory sync.
//!
//! Auth flow mirrors rtk-pro: OAuth browser login to cloud.rtk-ai.app,
//! credentials stored in the platform config directory.
//!
//! Cloud sync pushes project/org-scoped memories to the RTK Cloud API
//! so teams can share context across sessions and users.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use icm_core::{Memory, Scope};

// ── Credentials ─────────────────────────────────────────────────────────────

/// Cloud credentials stored in the platform config directory.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Credentials {
    pub endpoint: String,
    pub token: String,
    #[serde(rename = "orgId")]
    pub org_id: String,
    #[serde(rename = "orgSlug", default)]
    pub org_slug: String,
}

fn credentials_path() -> Result<PathBuf> {
    let proj = directories::ProjectDirs::from("dev", "icm", "icm")
        .context("unable to determine platform config directory")?;
    Ok(proj.config_dir().join("credentials.json"))
}

pub fn load_credentials() -> Option<Credentials> {
    // 1. Try ICM's own credentials
    if let Some(creds) = load_credentials_from_path(credentials_path().ok()?) {
        return Some(creds);
    }

    // 2. Fallback: reuse rtk-pro credentials (same format, avoids re-login)
    //    rtk-pro uses dirs::config_dir() which is:
    //    - macOS: ~/Library/Application Support/rtk/
    //    - Linux: ~/.config/rtk/
    // Resolve home cross-platform — Windows uses %USERPROFILE%, not $HOME.
    let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
    let rtk_paths = [
        // macOS: ~/Library/Application Support/rtk/credentials.json
        home.as_ref()
            .map(|h| h.join("Library/Application Support/rtk/credentials.json")),
        // Linux: ~/.config/rtk/credentials.json
        home.as_ref()
            .map(|h| h.join(".config/rtk/credentials.json")),
    ];
    for path in rtk_paths.into_iter().flatten() {
        if let Some(creds) = load_credentials_from_path(path) {
            return Some(creds);
        }
    }

    None
}

fn load_credentials_from_path(path: PathBuf) -> Option<Credentials> {
    let content = std::fs::read_to_string(&path).ok()?;
    let creds: Credentials = serde_json::from_str(&content).ok()?;
    // Validate token is non-empty
    if creds.token.is_empty() {
        return None;
    }
    Some(creds)
}

pub fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(creds)?;
    write_secret_file(&path, &json)
}

/// Write `content` to `path`, owner-only (0600) from the moment the file is
/// created on Unix. Security audit finding (TOCTOU): a prior `fs::write`
/// then `set_permissions` left a window — created with the process umask
/// (often world-readable) — where a crash between the two calls leaves the
/// bearer token durably readable by other local users.
pub(crate) fn write_secret_file(path: &std::path::Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(content.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, content)?;

    Ok(())
}

pub fn clear_credentials() -> Result<()> {
    let path = credentials_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Refuse a non-HTTPS endpoint unless it's localhost/loopback (audit
/// finding: the endpoint scheme was never validated, so a plain-`http://`
/// custom endpoint — `--endpoint` is a documented flag for self-hosting —
/// would send the login password and every bearer-token request in
/// cleartext to any network observer). `--endpoint` itself stays fully
/// user-configurable; only the scheme is gated, so self-hosted deployments
/// on a real domain are unaffected as long as they run behind TLS.
fn require_secure_endpoint(endpoint: &str) -> Result<()> {
    if endpoint.starts_with("https://") {
        return Ok(());
    }
    let host = endpoint
        .strip_prefix("http://")
        .and_then(|rest| rest.split(['/', ':']).next())
        .unwrap_or("");
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to use non-HTTPS endpoint {endpoint:?}: your password/token would be sent \
         in cleartext. Use an https:// endpoint, or http://localhost for local testing."
    );
}

// ── OAuth state nonce ────────────────────────────────────────────────────────

/// A one-off, hard-to-guess token for the OAuth callback's CSRF check.
/// `RandomState`'s keys are seeded from the OS's secure random source (std
/// uses it to defend `HashMap` against HashDoS) — reusing it here avoids
/// pulling in a new RNG dependency for a single nonce. Good enough for this
/// threat model (a local process racing our callback without access to our
/// process' random seed), not meant as a general-purpose CSPRNG API.
fn random_nonce() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = Vec::with_capacity(32);
    for i in 0..4u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        bytes.extend_from_slice(&h.finish().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── URL decode ──────────────────────────────────────────────────────────────

fn url_decode(s: &str) -> String {
    let mut result = Vec::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().and_then(|c| (c as char).to_digit(16));
            let lo = chars.next().and_then(|c| (c as char).to_digit(16));
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push((h * 16 + l) as u8);
            }
        } else if b == b'+' {
            result.push(b' ');
        } else {
            result.push(b);
        }
    }
    String::from_utf8_lossy(&result).to_string()
}

/// Percent-encode a value for safe use in a URL query string. Unreserved
/// per RFC 3986 (`A-Za-z0-9-._~`) pass through; everything else — notably
/// `+` (decoded as a literal space by `url_decode` above and most
/// form-urlencoded parsers) and `:` (reserved in a query component) — is
/// percent-encoded. Audit finding: `pull_memories`'s `since` RFC3339
/// timestamp (e.g. "2026-07-25T10:00:00+02:00") was pushed into the query
/// string unencoded, silently corrupting the filter on any backend that
/// applies form-urlencoded decoding to query values.
fn url_encode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── Login (browser OAuth) ───────────────────────────────────────────────────

/// Browser-based OAuth login: opens browser, listens for callback on localhost.
/// Same flow as rtk-pro: binds random port, opens cloud.rtk-ai.app/api/auth/oauth/google,
/// receives JWT callback.
pub fn login_browser(endpoint: &str) -> Result<Credentials> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    require_secure_endpoint(endpoint)?;

    let listener = TcpListener::bind("127.0.0.1:0").context("Failed to start local server")?;
    let port = listener.local_addr()?.port();

    // Security audit finding: without a `state` nonce, ANY local process
    // that learns the listening port (visible in the auth URL printed
    // above) can complete the callback with its own token before the real
    // browser response arrives, hijacking the CLI's cloud login onto an
    // attacker-controlled account. Generate a random nonce, pass it to the
    // server, and require it back on the callback.
    let state = random_nonce();

    let auth_url = format!(
        "{}/api/auth/oauth/google?cli_port={}&app=icm&state={}",
        endpoint.trim_end_matches('/'),
        port,
        state
    );

    eprintln!("Opening browser for authentication...");
    eprintln!("If the browser doesn't open, visit:\n  {}", auth_url);

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&auth_url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&auth_url)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", &auth_url])
            .spawn();
    }

    eprintln!("Waiting for authentication...");

    let timeout = std::time::Duration::from_secs(120);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!("Login timed out after 120 seconds");
        }

        listener.set_nonblocking(true)?;
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);

                if let Some(query) = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|path| path.strip_prefix("/callback?"))
                {
                    let params: std::collections::HashMap<String, String> = query
                        .split('&')
                        .filter_map(|pair| {
                            let mut parts = pair.splitn(2, '=');
                            let key = parts.next()?;
                            let value = parts.next().unwrap_or("");
                            Some((key.to_string(), url_decode(value)))
                        })
                        .collect();

                    // Reject a callback that presents a WRONG `state` — a
                    // local process racing the real browser response with
                    // its own token would need to guess this nonce (audit
                    // finding: previously no correlation at all existed
                    // between the callback and the login attempt that
                    // opened it). We only reject on an explicit mismatch,
                    // not on a missing param: the RTK Cloud backend may not
                    // echo `state` back yet, and failing open here would
                    // just mean every real login times out. Once the
                    // backend round-trips `state`, this becomes a full
                    // CSRF defense with no client-side change needed.
                    let presented_state = params.get("state").cloned();
                    if presented_state.as_deref().is_some_and(|s| s != state) {
                        let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Login failed</h2><p>Invalid state.</p></body></html>";
                        let _ = stream.write_all(response.as_bytes());
                        continue;
                    }

                    let token = params.get("token").cloned().unwrap_or_default();
                    let org_id = params.get("org_id").cloned().unwrap_or_default();
                    let email = params.get("email").cloned().unwrap_or_default();

                    if token.is_empty() {
                        let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Login failed</h2><p>No token received.</p></body></html>";
                        let _ = stream.write_all(response.as_bytes());
                        anyhow::bail!("No token received from OAuth callback");
                    }

                    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style=\"font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0f172a;color:white\"><div style=\"text-align:center\"><h1>ICM Cloud authenticated</h1><p>You can close this tab and return to your terminal.</p></div></body></html>";
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();

                    let creds = Credentials {
                        endpoint: endpoint.to_string(),
                        token,
                        org_id,
                        org_slug: String::new(),
                    };
                    save_credentials(&creds)?;
                    eprintln!("Logged in as {}", email);
                    return Ok(creds);
                }

                let response = "HTTP/1.1 404 Not Found\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                anyhow::bail!("Failed to accept connection: {}", e);
            }
        }
    }
}

/// Email/password login for orgs without OAuth (generic email, self-hosted, etc.)
/// POST {endpoint}/api/auth/login
pub fn login_password(endpoint: &str, email: &str, password: &str) -> Result<Credentials> {
    require_secure_endpoint(endpoint)?;

    let url = format!("{}/api/auth/login", endpoint.trim_end_matches('/'));

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send_string(
            &serde_json::json!({
                "email": email,
                "password": password,
            })
            .to_string(),
        )
        .context("Failed to connect to RTK Cloud")?;

    let status = resp.status();
    let body = resp.into_string().context("Failed to read response")?;

    if status != 200 {
        anyhow::bail!("Login failed ({}): {}", status, body);
    }

    #[derive(Deserialize)]
    struct LoginResponse {
        token: String,
        #[serde(rename = "orgId")]
        org_id: Option<String>,
        user: LoginUser,
    }

    #[derive(Deserialize)]
    struct LoginUser {
        #[allow(dead_code)]
        id: String,
        email: String,
        #[allow(dead_code)]
        name: String,
    }

    let data: LoginResponse = serde_json::from_str(&body).context("Invalid server response")?;

    let creds = Credentials {
        endpoint: endpoint.to_string(),
        token: data.token,
        org_id: data.org_id.unwrap_or_default(),
        org_slug: String::new(),
    };

    save_credentials(&creds)?;
    eprintln!("Logged in as {}", data.user.email);
    Ok(creds)
}

pub fn logout() -> Result<()> {
    clear_credentials()?;
    eprintln!("Logged out from ICM Cloud");
    Ok(())
}

pub fn status() -> Result<()> {
    match load_credentials() {
        Some(creds) => {
            eprintln!("ICM Cloud: connected");
            eprintln!("  Endpoint: {}", creds.endpoint);
            eprintln!("  Org ID:   {}", creds.org_id);
        }
        None => {
            eprintln!("ICM Cloud: not connected");
            eprintln!("  Run: icm cloud login");
        }
    }
    Ok(())
}

// ── Cloud Sync API ──────────────────────────────────────────────────────────

/// Push a memory to RTK Cloud.
/// POST {endpoint}/api/icm/memories
pub fn sync_memory(creds: &Credentials, memory: &Memory) -> Result<()> {
    let url = format!("{}/api/icm/memories", creds.endpoint.trim_end_matches('/'));

    let payload = serde_json::json!({
        "id": memory.id,
        "topic": memory.topic,
        "summary": memory.summary,
        "rawExcerpt": memory.raw_excerpt,
        "keywords": memory.keywords,
        "importance": memory.importance.to_string(),
        "scope": memory.scope.to_string(),
        "source": serde_json::to_value(&memory.source).ok(),
        "createdAt": memory.created_at.to_rfc3339(),
        "updatedAt": memory.updated_at.to_rfc3339(),
    });

    let resp = match ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", creds.token))
        .set("Content-Type", "application/json")
        .set("X-Org-Id", &creds.org_id)
        .timeout(std::time::Duration::from_secs(5))
        .send_string(&payload.to_string())
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            anyhow::bail!("Cloud sync failed ({}): {}", code, body);
        }
        Err(e) => anyhow::bail!("Cloud sync connection error: {}", e),
    };

    let status = resp.status();
    if status != 200 && status != 201 {
        let body = resp.into_string().unwrap_or_default();
        anyhow::bail!("Cloud sync failed ({}): {}", status, body);
    }

    Ok(())
}

/// Pull memories from RTK Cloud for a given scope.
/// GET {endpoint}/api/icm/memories?scope={scope}&since={since}
pub fn pull_memories(
    creds: &Credentials,
    scope: Scope,
    since: Option<&str>,
) -> Result<Vec<Memory>> {
    let mut url = format!(
        "{}/api/icm/memories?scope={}",
        creds.endpoint.trim_end_matches('/'),
        scope
    );

    if let Some(ts) = since {
        // Audit finding: `since` is a raw user-supplied --since CLI value
        // (RFC3339), pushed into the query string unencoded. RFC3339
        // timestamps with a timezone offset contain `+` (e.g.
        // "2026-07-25T10:00:00+02:00"), which a form-urlencoded-convention
        // parser on the backend decodes as a literal space — silently
        // corrupting or dropping the filter. `:` is also reserved in a
        // query component per RFC 3986. Percent-encode it properly.
        url.push_str(&format!("&since={}", url_encode_query_value(ts)));
    }

    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", creds.token))
        .set("X-Org-Id", &creds.org_id)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .context("Failed to pull memories from cloud")?;

    let status = resp.status();
    let body = resp.into_string().context("Failed to read response")?;

    if status != 200 {
        anyhow::bail!("Cloud pull failed ({}): {}", status, body);
    }

    /// Intermediate type for deserializing cloud responses.
    /// The cloud API may return different field shapes than local Memory.
    #[derive(Deserialize)]
    struct CloudMemory {
        id: String,
        topic: String,
        summary: String,
        #[serde(default)]
        raw_excerpt: Option<String>,
        #[serde(default)]
        keywords: Vec<String>,
        #[serde(default = "default_importance_str")]
        importance: String,
        #[serde(default = "default_scope_str")]
        scope: String,
        // Audit finding: `#[serde(default)]` on f32 is 0.0, which is below
        // the prune threshold — a memory pulled without a `weight` field
        // (the push side never sends one) would be eligible for immediate
        // deletion by the next `prune`. Default to 1.0, matching
        // `Memory::new()`'s baseline for a fresh memory.
        #[serde(default = "default_weight")]
        weight: f32,
        #[serde(default)]
        access_count: u32,
        #[serde(default)]
        related_ids: Vec<String>,
        #[serde(default)]
        source: Option<serde_json::Value>,
        created_at: Option<String>,
        updated_at: Option<String>,
        last_accessed: Option<String>,
    }

    fn default_importance_str() -> String {
        "medium".to_string()
    }
    fn default_scope_str() -> String {
        "user".to_string()
    }
    fn default_weight() -> f32 {
        1.0
    }

    #[derive(Deserialize)]
    struct PullResponse {
        memories: Vec<CloudMemory>,
    }

    let data: PullResponse = serde_json::from_str(&body).context("Invalid cloud response")?;

    let memories = data
        .memories
        .into_iter()
        .map(|cm| {
            let importance = cm
                .importance
                .parse::<icm_core::Importance>()
                .unwrap_or(icm_core::Importance::Medium);
            let scope = cm.scope.parse::<Scope>().unwrap_or(Scope::User);
            let source = cm
                .source
                .and_then(|v| serde_json::from_value::<icm_core::MemorySource>(v).ok())
                .unwrap_or(icm_core::MemorySource::Manual);
            let now = chrono::Utc::now();

            Memory {
                id: cm.id,
                topic: cm.topic,
                summary: cm.summary,
                raw_excerpt: cm.raw_excerpt,
                keywords: cm.keywords,
                importance,
                scope,
                source,
                weight: cm.weight,
                access_count: cm.access_count,
                related_ids: cm.related_ids,
                embedding: None,
                created_at: cm
                    .created_at
                    .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                    .unwrap_or(now),
                updated_at: cm
                    .updated_at
                    .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                    .unwrap_or(now),
                last_accessed: cm
                    .last_accessed
                    .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                    .unwrap_or(now),
            }
        })
        .collect();

    Ok(memories)
}

/// Check if cloud sync is available (credentials exist and scope requires it).
pub fn requires_cloud(scope: Scope) -> bool {
    scope != Scope::User
}

/// Get credentials or print upsell message.
pub fn require_credentials_for_scope(scope: Scope) -> Option<Credentials> {
    if scope == Scope::User {
        return None; // User scope doesn't need cloud
    }

    match load_credentials() {
        Some(creds) => Some(creds),
        None => {
            eprintln!(
                "Cloud sync required for {} scope. Run: icm cloud login",
                scope
            );
            eprintln!("ICM Cloud enables shared memories across your team.");
            eprintln!("Learn more: https://cloud.rtk-ai.app/features/memories");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit regression: writing a credentials file must never pass through
    /// a world-readable intermediate state (TOCTOU) — verified by checking
    /// the mode immediately after `write_secret_file` returns.
    #[cfg(unix)]
    #[test]
    fn test_write_secret_file_is_0600_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("icm-test-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.json");

        write_secret_file(&path, "{\"token\":\"x\"}").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_require_secure_endpoint() {
        assert!(require_secure_endpoint("https://cloud.rtk-ai.app").is_ok());
        assert!(require_secure_endpoint("https://my-company.example.com").is_ok());
        assert!(require_secure_endpoint("http://localhost:8080").is_ok());
        assert!(require_secure_endpoint("http://127.0.0.1:8080").is_ok());

        assert!(require_secure_endpoint("http://cloud.rtk-ai.app").is_err());
        assert!(require_secure_endpoint("http://evil.example.com").is_err());
    }

    #[test]
    fn test_random_nonce_is_nonempty_and_varies() {
        let a = random_nonce();
        let b = random_nonce();
        assert!(!a.is_empty());
        assert_ne!(a, b, "two nonces should not collide in practice");
    }

    #[test]
    fn test_url_decode() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("user%40example.com"), "user@example.com");
        assert_eq!(url_decode("hello+world"), "hello world");
        assert_eq!(url_decode("no_encoding"), "no_encoding");
    }

    #[cfg(unix)]
    #[test]
    fn test_credentials_file_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;

        // Create a temp file, apply the same permission logic as save_credentials
        let dir = std::env::temp_dir().join(format!("icm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test-credentials.json");

        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials file should be owner-only (0o600)");

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_requires_cloud() {
        assert!(!requires_cloud(Scope::User));
        assert!(requires_cloud(Scope::Project));
        assert!(requires_cloud(Scope::Org));
    }

    /// Audit regression: an RFC3339 `since` timestamp with a `+HH:MM` offset
    /// must survive round-tripping through the query string. `+` is decoded
    /// as a literal space by form-urlencoded convention, so an unencoded
    /// `+` silently corrupts the offset (and thus the filter).
    #[test]
    fn test_url_encode_query_value_escapes_reserved_chars() {
        let encoded = url_encode_query_value("2026-07-25T10:00:00+02:00");
        assert_eq!(encoded, "2026-07-25T10%3A00%3A00%2B02%3A00");
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains(':'));
    }

    #[test]
    fn test_url_encode_query_value_leaves_unreserved_chars_untouched() {
        let unreserved = "AZaz09-._~";
        assert_eq!(url_encode_query_value(unreserved), unreserved);
    }
}
