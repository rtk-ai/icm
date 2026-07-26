//! Self-upgrade command with SHA256 integrity verification.
//!
//! Downloads the latest release binary from GitHub, verifies its SHA256
//! against the release's `checksums.txt`, and replaces the running binary.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

const REPO: &str = "rtk-ai/icm";
const BINARY_NAME: &str = "icm";

/// Parse a `major.minor.patch` prefix, ignoring any `-prerelease`/`+build`
/// suffix. Returns `None` if the string doesn't start with three
/// dot-separated numeric components.
fn parse_semver_core(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Is `latest` actually newer than `current`?
///
/// Audit finding: the caller used to check `latest_version ==
/// current_version` only — any *difference* (not just a newer version)
/// triggered the upgrade flow. A source build with an unreleased version
/// bump (e.g. `0.11.0-dev` built ahead of the last published tag
/// `0.10.59`) would silently downgrade to the older published release.
/// Falls back to the old equality-only check when either string doesn't
/// parse as `major.minor.patch` — never silently treats an unparseable
/// version as newer.
fn is_newer_version(current: &str, latest: &str) -> bool {
    match (parse_semver_core(current), parse_semver_core(latest)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => latest != current,
    }
}

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

/// Write the downloaded (already SHA256-verified) binary to `path`,
/// refusing to follow a pre-existing symlink there.
///
/// Audit finding: `File::create` is `O_CREAT|O_TRUNC` with no `O_EXCL` - it
/// follows an existing symlink at `path`. If an attacker with write access
/// to this directory pre-places a symlink pointing elsewhere, the verified
/// download gets written through it, clobbering an unrelated file (not
/// RCE - the payload is the legitimate SHA256-verified binary - but a real
/// file-clobber/DoS gap, the same TOCTOU class already hardened in
/// `cloud.rs::write_secret_file`). Remove any existing entry first via
/// `symlink_metadata` (which reports the symlink itself, not its target) so
/// a stale symlink or a leftover from an interrupted previous upgrade
/// doesn't get followed, then open with `create_new` (`O_EXCL`) so even an
/// attacker racing a fresh symlink into the gap fails the open rather than
/// getting followed.
fn write_new_binary(path: &Path, content: &[u8]) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path)
            .with_context(|| format!("cannot remove stale {}", path.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    f.write_all(content)
        .with_context(|| format!("cannot write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
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

    if !is_newer_version(current_version, latest_version) {
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

    // Detect package-managed installations — refuse to avoid breaking metadata
    let current_exe =
        std::env::current_exe().context("cannot determine current executable path")?;
    let path_str = current_exe.to_string_lossy();
    if path_str.contains("/Cellar/") || path_str.contains("/homebrew/") {
        bail!(
            "Detected Homebrew installation ({}).\nUse 'brew upgrade icm' instead to keep metadata consistent.",
            current_exe.display()
        );
    }
    if path_str.starts_with("/usr/bin/")
        || path_str.starts_with("/opt/") && !path_str.contains("/homebrew/")
    {
        eprintln!(
            "Warning: {} may be managed by a package manager (apt/dnf/rpm).",
            current_exe.display()
        );
        eprintln!("Consider using your package manager to upgrade instead.");
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
    let backup_path: PathBuf = current_exe.with_extension("old");
    let new_path: PathBuf = current_exe.with_extension("new");

    eprintln!("Installing to {}...", current_exe.display());

    // Write new binary to .new.
    write_new_binary(&new_path, &new_binary)?;

    swap_binary_into_place(&new_path, &current_exe, &backup_path)?;

    eprintln!("Successfully upgraded to {latest_version}");
    Ok(())
}

/// Swap `new_path` into `current_exe`'s place, keeping `current_exe`'s
/// prior content at `backup_path` until the swap succeeds. On failure,
/// attempts to roll back and reports accurately whether the rollback
/// itself succeeded.
///
/// Audit finding: the rollback's own result used to be discarded via
/// `.ok()`, yet the error message unconditionally claimed "(rolled
/// back)" — if the rollback rename itself failed (permissions changed
/// mid-flight, disk full), the user would be told the binary was
/// restored when `current_exe` was in fact still missing.
fn swap_binary_into_place(new_path: &Path, current_exe: &Path, backup_path: &Path) -> Result<()> {
    if backup_path.exists() {
        std::fs::remove_file(backup_path).ok();
    }
    std::fs::rename(current_exe, backup_path)
        .with_context(|| format!("cannot backup {}", current_exe.display()))?;
    if let Err(e) = std::fs::rename(new_path, current_exe) {
        let rollback_result = std::fs::rename(backup_path, current_exe);
        return Err(swap_failure_error(
            e,
            rollback_result,
            current_exe,
            backup_path,
        ));
    }

    // Clean up backup
    std::fs::remove_file(backup_path).ok();
    Ok(())
}

/// Build the error for a failed swap, reporting accurately whether the
/// rollback attempt itself succeeded — split out from
/// `swap_binary_into_place` so this reporting logic is directly testable
/// without needing to simulate a real filesystem-level rollback failure.
fn swap_failure_error(
    swap_err: std::io::Error,
    rollback_result: std::io::Result<()>,
    current_exe: &Path,
    backup_path: &Path,
) -> anyhow::Error {
    match rollback_result {
        Ok(()) => {
            anyhow::Error::new(swap_err).context("failed to install new binary (rolled back)")
        }
        Err(rollback_err) => anyhow::Error::new(swap_err).context(format!(
            "failed to install new binary AND rollback failed ({rollback_err}) — \
             {} may be missing; restore it manually from {}",
            current_exe.display(),
            backup_path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit regression: `File::create` follows a symlink pre-placed at
    /// the target path, letting an attacker with write access to the
    /// directory redirect the verified-download write elsewhere. The fix
    /// must remove any existing entry (symlink or stale leftover) first
    /// and never write through a symlink.
    #[test]
    #[cfg(unix)]
    fn write_new_binary_does_not_follow_a_preexisting_symlink() {
        let tmp = tempfile::TempDir::new().unwrap();
        let victim = tmp.path().join("victim.txt");
        std::fs::write(&victim, "untouched").unwrap();

        let target = tmp.path().join("icm.new");
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        write_new_binary(&target, b"verified binary content").unwrap();

        // The symlink must be gone, replaced by a real file...
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_file(),
            "target must be a regular file, not still a symlink"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"verified binary content");
        // ...and the symlink's old target must be untouched.
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    }

    /// Sanity check for the normal case: no pre-existing entry at all.
    #[test]
    fn write_new_binary_creates_a_fresh_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("icm.new");
        write_new_binary(&target, b"payload").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
    }

    /// Audit regression: a bare `==` check treated ANY difference as "an
    /// update is available" rather than checking direction — a source
    /// build ahead of the last published tag would silently downgrade.
    #[test]
    fn is_newer_version_requires_a_real_increase() {
        assert!(is_newer_version("0.10.59", "0.10.60"));
        assert!(
            is_newer_version("0.9.0", "0.10.0"),
            "0.10.0 > 0.9.0 numerically, not lexically"
        );
        assert!(
            !is_newer_version("0.11.0-dev", "0.10.59"),
            "an unreleased dev build ahead of the last tag must not be downgraded"
        );
        assert!(
            !is_newer_version("0.10.59", "0.10.59"),
            "equal versions are not newer"
        );
        assert!(
            !is_newer_version("0.10.60", "0.10.59"),
            "an older tag is not newer"
        );
    }

    #[test]
    fn is_newer_version_falls_back_to_equality_for_unparseable_versions() {
        // Neither side parses as major.minor.patch — preserve the old
        // equality-only behavior rather than guessing a direction.
        assert!(is_newer_version("garbage", "also-garbage"));
        assert!(!is_newer_version("garbage", "garbage"));
    }

    /// Audit regression: the rollback's own result must determine the
    /// error message — a failed rollback must never be reported as
    /// "rolled back".
    #[test]
    fn swap_failure_error_reports_rollback_outcome_accurately() {
        let swap_err = std::io::Error::other("swap failed");
        let current_exe = Path::new("/fake/icm");
        let backup_path = Path::new("/fake/icm.old");

        let ok_msg = format!(
            "{:#}",
            swap_failure_error(
                std::io::Error::other("swap failed"),
                Ok(()),
                current_exe,
                backup_path,
            )
        );
        assert!(ok_msg.contains("rolled back"));
        assert!(!ok_msg.contains("rollback failed"));

        let rollback_err_msg = format!(
            "{:#}",
            swap_failure_error(
                swap_err,
                Err(std::io::Error::other("permission denied")),
                current_exe,
                backup_path,
            )
        );
        assert!(
            rollback_err_msg.contains("rollback failed"),
            "must surface the rollback failure, not claim success: {rollback_err_msg}"
        );
        assert!(
            rollback_err_msg.contains("restore it manually"),
            "must tell the user how to recover: {rollback_err_msg}"
        );
    }

    /// End-to-end swap test on real files: the common failure mode (the
    /// new binary is missing) must roll back cleanly and leave the
    /// original binary content intact.
    #[test]
    fn swap_binary_into_place_rolls_back_when_new_binary_is_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let current_exe = tmp.path().join("icm");
        let new_path = tmp.path().join("icm.new");
        let backup_path = tmp.path().join("icm.old");
        std::fs::write(&current_exe, b"original binary").unwrap();
        // new_path deliberately does not exist.

        let result = swap_binary_into_place(&new_path, &current_exe, &backup_path);
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&current_exe).unwrap(),
            b"original binary",
            "original binary must survive a failed swap"
        );
        assert!(
            !backup_path.exists(),
            "backup should be consumed by the rollback"
        );
    }
}
