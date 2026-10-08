//! Self-upgrade command with SHA256 integrity verification.
//!
//! Downloads the latest release binary from GitHub, verifies its SHA256
//! against the release's `checksums.txt`, and replaces the running binary.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
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
fn detect_target() -> Result<(String, &'static str)> {
    target_for(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "musl"),
    )
}

/// The release archive that replaces a binary built for `os` / `arch`.
///
/// A static musl binary must be replaced by the musl archive: it is the one
/// `install.sh` picks on Alpine and on systems whose glibc is older than
/// the gnu archive needs, and the gnu archive does not start there.
fn target_for(os: &str, arch: &str, musl: bool) -> Result<(String, &'static str)> {
    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => bail!("Unsupported architecture: {arch}"),
    };
    let target_suffix = match os {
        "macos" => "apple-darwin",
        "linux" if musl => {
            if arch != "x86_64" {
                bail!("no static musl release exists for {arch}; reinstall with install.sh");
            }
            "unknown-linux-musl"
        }
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        _ => bail!("Unsupported OS: {os}"),
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Ok((format!("{arch}-{target_suffix}"), ext))
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
    icm_core::to_hex(&hasher.finalize())
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
        bail!("{WINDOWS_UPGRADE_HINT}");
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
/// `config.rs::write_secret_file`). Remove any existing entry first via
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

/// What to do instead of `icm upgrade --apply` on Windows, where the release
/// is a zip this command does not unpack.
const WINDOWS_UPGRADE_HINT: &str = "icm upgrade --apply is not available on Windows. \
     Run the installer again instead:\n  \
     irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex";

/// Start the freshly written binary and check it reports the version being
/// installed, before it takes the place of the one that works.
///
/// A download can be intact and still not run here: a binary for another
/// libc, or one that needs a newer glibc than this system has.
fn verify_new_binary(path: &Path, expected_version: &str) -> Result<()> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| {
            format!(
                "the downloaded binary does not start ({}); the installed binary is unchanged",
                path.display()
            )
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let reported = stdout.split_whitespace().nth(1).unwrap_or("");
    if !out.status.success() || reported != expected_version {
        bail!(
            "the downloaded binary did not report version {expected_version} \
             (exit {}, output {:?}); the installed binary is unchanged",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            stdout.trim()
        );
    }
    Ok(())
}

/// Print what the new binary says about semantic search. A release may load
/// its ONNX Runtime on demand where the previous one had it built in; without
/// this line the upgrade would turn semantic search off without a word.
fn report_embeddings_status(binary: &Path) {
    let Ok(out) = std::process::Command::new(binary)
        .args(["embeddings", "status"])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
        eprintln!("Semantic search: {}", line.trim());
    }
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

    // Say so before downloading anything.
    if cfg!(windows) {
        bail!("{WINDOWS_UPGRADE_HINT}");
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

    // Write new binary to .new, and try it before it replaces anything.
    write_new_binary(&new_path, &new_binary)?;
    if let Err(e) = verify_new_binary(&new_path, latest_version) {
        let _ = std::fs::remove_file(&new_path);
        return Err(e);
    }

    swap_binary_into_place(&new_path, &current_exe, &backup_path)?;

    eprintln!("Successfully upgraded to {latest_version}");
    report_embeddings_status(&current_exe);
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
    fn a_musl_binary_is_replaced_by_the_musl_archive() {
        let t = |os, arch, musl| target_for(os, arch, musl).map(|(t, e)| (t, e.to_string()));
        assert_eq!(
            t("linux", "x86_64", true).unwrap(),
            ("x86_64-unknown-linux-musl".into(), "tar.gz".into())
        );
        assert_eq!(
            t("linux", "x86_64", false).unwrap().0,
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            t("linux", "aarch64", false).unwrap().0,
            "aarch64-unknown-linux-gnu"
        );
        // No musl archive is published for aarch64: say so rather than
        // install the gnu one, which would not start.
        assert!(t("linux", "aarch64", true).is_err());
        assert_eq!(
            t("macos", "aarch64", false).unwrap().0,
            "aarch64-apple-darwin"
        );
        assert_eq!(
            t("windows", "x86_64", false).unwrap(),
            ("x86_64-pc-windows-msvc".into(), "zip".into())
        );
        assert!(t("freebsd", "x86_64", false).is_err());
        assert!(t("linux", "riscv64", false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_binary_that_does_not_run_or_reports_another_version_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let p = tmp.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let good = script("good", "echo 'icm 1.2.3'");
        assert!(verify_new_binary(&good, "1.2.3").is_ok());

        let other = script("other", "echo 'icm 1.2.2'");
        let err = verify_new_binary(&other, "1.2.3").unwrap_err().to_string();
        assert!(err.contains("did not report version 1.2.3"), "{err}");
        assert!(err.contains("unchanged"), "{err}");

        let failing = script("failing", "echo 'icm 1.2.3'; exit 3");
        assert!(verify_new_binary(&failing, "1.2.3").is_err());

        // Not an executable for this system at all (the wrong-libc case).
        let garbage = tmp.path().join("garbage");
        std::fs::write(&garbage, b"\x7fELF not really").unwrap();
        std::fs::set_permissions(&garbage, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = verify_new_binary(&garbage, "1.2.3")
            .unwrap_err()
            .to_string();
        // Depending on the system this is a failed exec or a shell's exit 127.
        assert!(err.contains("unchanged"), "{err}");
    }

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
