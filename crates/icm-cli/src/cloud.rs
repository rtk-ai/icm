//! RTK Cloud client for ICM — login, credentials, and memory sync.
//!
//! Auth flow mirrors rtk-pro: OAuth browser login to cloud.rtk-ai.app,
//! credentials stored at ~/.config/icm/credentials.json.
//!
//! Cloud sync pushes project/org-scoped memories to the RTK Cloud API
//! so teams can share context across sessions and users.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use icm_core::{Memory, Scope};

// ── Credentials ─────────────────────────────────────────────────────────────

/// Cloud credentials stored at ~/.config/icm/credentials.json
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
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("icm")
        .join("credentials.json"))
}

pub fn load_credentials() -> Option<Credentials> {
    let path = credentials_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(creds)?;
    std::fs::write(&path, json)?;
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

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", creds.token))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(5))
        .send_string(&payload.to_string())
        .context("Failed to sync memory to cloud")?;

    let status = resp.status();
    let body = resp.into_string().unwrap_or_default();
    if status != 200 && status != 201 {
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
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .context("Failed to pull memories from cloud")?;

    let status = resp.status();
    let body = resp.into_string().context("Failed to read response")?;

    if status != 200 {
        anyhow::bail!("Cloud pull failed ({}): {}", status, body);
    }

    #[derive(Deserialize)]
    struct PullResponse {
        memories: Vec<Memory>,
    }

    let data: PullResponse = serde_json::from_str(&body).context("Invalid cloud response")?;

    Ok(data.memories)
}

/// Delete a memory from RTK Cloud.
/// DELETE {endpoint}/api/icm/memories/{id}
pub fn delete_cloud_memory(creds: &Credentials, memory_id: &str) -> Result<()> {
    let url = format!(
        "{}/api/icm/memories/{}",
        creds.endpoint.trim_end_matches('/'),
        memory_id
    );

    let resp = ureq::delete(&url)
        .set("Authorization", &format!("Bearer {}", creds.token))
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .context("Failed to delete cloud memory")?;

    let status = resp.status();
    if status != 200 && status != 204 {
        let body = resp.into_string().unwrap_or_default();
        anyhow::bail!("Cloud delete failed ({}): {}", status, body);
    }

    Ok(())
}

/// Fire-and-forget sync: push memory in a background thread.
/// Used after store/update operations to sync without blocking.
pub fn sync_memory_background(memory: Memory) {
    let creds = match load_credentials() {
        Some(c) => c,
        None => return,
    };

    // Only sync project/org scoped memories
    if memory.scope == Scope::User {
        return;
    }

    std::thread::spawn(move || {
        if let Err(e) = sync_memory(&creds, &memory) {
            tracing::warn!("Cloud sync failed: {}", e);
        }
    });
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

    #[test]
    fn test_requires_cloud() {
        assert!(!requires_cloud(Scope::User));
        assert!(requires_cloud(Scope::Project));
        assert!(requires_cloud(Scope::Org));
    }
}
