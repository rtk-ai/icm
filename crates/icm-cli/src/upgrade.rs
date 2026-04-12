//! Self-upgrade command with SHA256 integrity verification.
//!
//! Downloads the latest release binary from GitHub, verifies its SHA256
//! against the release's `checksums.txt`, and replaces the running binary.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

const REPO: &str = "rtk-ai/icm";
const BINARY_NAME: &str = "icm";

/// Detect the target triple for this platform.
fn detect_target() -> Result<(&'static str, &'static str)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let target_suffix = match os {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        _ => bail!("Unsupported OS: {os}"),
    };

    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => bail!("Unsupported architecture: {arch}"),
    };

    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    let target = Box::leak(format!("{arch}-{target_suffix}").into_boxed_str());
    Ok((target, ext))
}

/// Fetch the latest release tag from the GitHub API.
fn fetch_latest_version() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        .set("User-Agent", "icm-upgrader")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("failed to fetch latest release")?;

    let json: serde_json::Value = resp.into_json().context("invalid API response")?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing tag_name in API response"))?;
    Ok(tag.to_string())
}

/// Download a URL to a byte vector with size tracking.
fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", "icm-upgrader")
        .call()
        .with_context(|| format!("failed to download {url}"))?;

    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .context("failed to read response body")?;
    Ok(buf)
}

/// Compute SHA256 of bytes as lowercase hex.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Parse the expected SHA256 for a file from a `sha256sum` output.
/// Format per line: `<64-hex>  <filename>`.
fn parse_expected_sha(checksums: &str, filename: &str) -> Result<String> {
    for line in checksums.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() == 2 && parts[1] == filename {
            return Ok(parts[0].to_string());
        }
    }
    bail!("no checksum found for {filename} in checksums.txt")
}

/// Extract a binary from a tar.gz or zip archive. Returns the binary bytes.
fn extract_binary(archive: &[u8], is_zip: bool) -> Result<Vec<u8>> {
    if is_zip {
        // Windows: zip containing icm.exe
        bail!("zip extraction not supported — use the standalone installer on Windows");
    }

    // Unix: tar.gz containing icm
    use flate2::read::GzDecoder;
    let gz = GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);

    for entry in tar.entries().context("reading tar")? {
        let mut entry = entry.context("tar entry")?;
        let path = entry.path().context("entry path")?;
        if path.file_name().and_then(|n| n.to_str()) == Some(BINARY_NAME) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).context("reading binary")?;
            return Ok(buf);
        }
    }
    bail!("binary {BINARY_NAME} not found in archive")
}

/// Run the upgrade flow: fetch latest, verify checksum, replace binary.
pub fn cmd_upgrade(apply: bool, check_only: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("Current version: {current_version}");

    // 1. Fetch latest release
    eprintln!("Checking for updates...");
    let latest_tag = fetch_latest_version()?;
    let latest_version = latest_tag.strip_prefix("icm-v").unwrap_or(&latest_tag);
    eprintln!("Latest version:  {latest_version}");

    if latest_version == current_version {
        eprintln!("Already up to date.");
        return Ok(());
    }

    if check_only {
        eprintln!("Update available: {current_version} → {latest_version}");
        eprintln!("Run 'icm upgrade --apply' to install.");
        return Ok(());
    }

    if !apply {
        eprintln!("Update available: {current_version} → {latest_version}");
        eprintln!("Run 'icm upgrade --apply' to install.");
        return Ok(());
    }

    // 2. Detect target
    let (target, ext) = detect_target()?;
    let archive_name = format!("{BINARY_NAME}-{target}.{ext}");
    let archive_url =
        format!("https://github.com/{REPO}/releases/download/{latest_tag}/{archive_name}");
    let checksums_url =
        format!("https://github.com/{REPO}/releases/download/{latest_tag}/checksums.txt");

    // 3. Download archive
    eprintln!("Downloading {archive_name}...");
    let archive_bytes = download_bytes(&archive_url)?;
    eprintln!("  {} bytes", archive_bytes.len());

    // 4. Download and verify checksum (MANDATORY)
    eprintln!("Verifying integrity...");
    let checksums = String::from_utf8(download_bytes(&checksums_url)?)
        .context("checksums.txt is not valid UTF-8")?;
    let expected_sha = parse_expected_sha(&checksums, &archive_name)?;
    let actual_sha = sha256_hex(&archive_bytes);

    if expected_sha != actual_sha {
        bail!(
            "SHA256 mismatch!\n  expected: {expected_sha}\n  got:      {actual_sha}\nAborting upgrade — binary may be tampered."
        );
    }
    eprintln!("  SHA256 OK: {actual_sha}");

    // 5. Extract binary
    eprintln!("Extracting...");
    let is_zip = ext == "zip";
    let new_binary = extract_binary(&archive_bytes, is_zip)?;

    // 6. Replace running binary atomically
    let current_exe =
        std::env::current_exe().context("cannot determine current executable path")?;
    let backup_path: PathBuf = current_exe.with_extension("old");
    let new_path: PathBuf = current_exe.with_extension("new");

    eprintln!("Installing to {}...", current_exe.display());

    // Write new binary to .new
    {
        let mut f = std::fs::File::create(&new_path)
            .with_context(|| format!("cannot create {}", new_path.display()))?;
        f.write_all(&new_binary)
            .with_context(|| format!("cannot write {}", new_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))?;
        }
    }

    // Atomic swap: rename old → .old, new → current
    if backup_path.exists() {
        std::fs::remove_file(&backup_path).ok();
    }
    std::fs::rename(&current_exe, &backup_path)
        .with_context(|| format!("cannot backup {}", current_exe.display()))?;
    if let Err(e) = std::fs::rename(&new_path, &current_exe) {
        // Rollback on error
        std::fs::rename(&backup_path, &current_exe).ok();
        return Err(e).context("failed to install new binary (rolled back)");
    }

    // Clean up backup
    std::fs::remove_file(&backup_path).ok();

    eprintln!("Successfully upgraded to {latest_version}");
    Ok(())
}
