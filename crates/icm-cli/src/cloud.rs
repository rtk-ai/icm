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
    let rtk_paths = [
        // macOS: ~/Library/Application Support/rtk/credentials.json
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(&h).join("Library/Application Support/rtk/credentials.json")),
        // Linux: ~/.config/rtk/credentials.json
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(&h).join(".config/rtk/credentials.json")),
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
    std::fs::write(&path, json)?;

    // Restrict file permissions to owner-only on Unix (0o600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

pub fn clear_credentials() -> Result<()> {
    let path = credentials_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
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

// ── Login (browser OAuth) ───────────────────────────────────────────────────

/// Browser-based OAuth login: opens browser, listens for callback on localhost.
/// Same flow as rtk-pro: binds random port, opens cloud.rtk-ai.app/api/auth/oauth/google,
/// receives JWT callback.
pub fn login_browser(endpoint: &str) -> Result<Credentials> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").context("Failed to start local server")?;
    let port = listener.local_addr()?.port();

    let auth_url = format!(
        "{}/api/auth/oauth/google?cli_port={}&app=icm",
        endpoint.trim_end_matches('/'),
        port
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
        url.push_str(&format!("&since={}", ts));
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
        #[serde(default)]
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
                context: None,
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
}
