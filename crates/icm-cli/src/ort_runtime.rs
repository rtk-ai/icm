//! First-launch opt-in onnxruntime runtime for the `embeddings-dynamic` build
//! (issue #345).
//!
//! The load-dynamic build ships without onnxruntime so it compiles in a
//! network-less sandbox (e.g. homebrew-core). This module resolves the runtime
//! at *execution* time instead:
//!
//! - [`activate_if_present`] points `ort` at a previously-downloaded,
//!   icm-managed copy (always, even for `serve`/`hook`).
//! - [`ensure_for_run`] additionally offers an interactive one-time download on
//!   first use (never in non-interactive contexts).
//! - [`cmd_download`] / [`cmd_status`] back the explicit `icm embeddings`
//!   subcommand for scripted setup.
//!
//! Safety: `ort` 2.0.0-rc.9 targets ONNX Runtime **1.20**; a mismatched runtime
//! makes ort's version assertion abort under `panic = "abort"`. We therefore
//! pin 1.20.1, verify a hard-coded SHA256, and trust *only* our managed copy
//! (or an explicit user `ORT_DYLIB_PATH`) — never an ambient system library.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

/// onnxruntime release to fetch. MUST stay ABI-compatible with the `ort`
/// version fastembed pins (currently `ort` 2.0.0-rc.9 → ONNX Runtime 1.20).
const ORT_VERSION: &str = "1.20.1";

const ORT_DYLIB_ENV: &str = "ORT_DYLIB_PATH";

/// A supported download target: the Microsoft asset infix, the pinned SHA256 of
/// `onnxruntime-<asset>-<ver>.tgz`, and the canonical shared-library filename
/// that ort's load-dynamic backend looks for.
struct Target {
    asset: &'static str,
    sha256: &'static str,
    lib_name: &'static str,
}

/// Resolve the onnxruntime asset for this platform, or `None` if unsupported
/// (Windows `.zip` and exotic arches are deferred — issue #345).
fn detect_target() -> Option<Target> {
    let lib_name = if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let (asset, sha256) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => (
            "osx-arm64",
            "b678fc3c2354c771fea4fba420edeccfba205140088334df801e7fc40e83a57a",
        ),
        ("macos", "x86_64") => (
            "osx-x86_64",
            "0f73006813af2a1a5d1723ed7dfb694fc629d15037124081bb61b7bf7d99fc78",
        ),
        ("linux", "x86_64") => (
            "linux-x64",
            "67db4dc1561f1e3fd42e619575c82c601ef89849afc7ea85a003abbac1a1a105",
        ),
        ("linux", "aarch64") => (
            "linux-aarch64",
            "ae4fedbdc8c18d688c01306b4b50c63de3445cdf2dbd720e01a2fa3810b8106a",
        ),
        _ => return None,
    };
    Some(Target {
        asset,
        sha256,
        lib_name,
    })
}

fn onnxruntime_root() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "icm", "icm").map(|d| d.data_dir().join("onnxruntime"))
}

/// Version-scoped directory holding the managed runtime.
fn managed_dir() -> Option<PathBuf> {
    onnxruntime_root().map(|d| d.join(ORT_VERSION))
}

/// Full path to the managed shared library for this platform.
fn managed_lib_path() -> Option<PathBuf> {
    let target = detect_target()?;
    Some(managed_dir()?.join(target.lib_name))
}

/// Marker written when the user declines the first-launch download, so we don't
/// nag on every run. Kept outside the version dir so it survives version bumps.
fn declined_marker() -> Option<PathBuf> {
    onnxruntime_root().map(|d| d.join(".declined"))
}

fn user_set_dylib() -> bool {
    std::env::var_os(ORT_DYLIB_ENV).is_some_and(|v| !v.is_empty())
}

/// Point `ort` at the managed library if it exists and the user hasn't set
/// `ORT_DYLIB_PATH`. Returns whether a runtime is now resolvable. Safe to call
/// unconditionally (including for `serve`/`hook`).
pub fn activate_if_present() -> bool {
    if user_set_dylib() {
        return true;
    }
    if let Some(lib) = managed_lib_path() {
        if lib.is_file() {
            std::env::set_var(ORT_DYLIB_ENV, &lib);
            return true;
        }
    }
    false
}

fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", "icm-onnxruntime-fetch")
        .call()
        .with_context(|| format!("failed to download {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .context("failed to read download body")?;
    Ok(buf)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn verify_sha(bytes: &[u8], expected: &str, what: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    if actual != expected {
        bail!(
            "checksum mismatch for {what}: expected {expected}, got {actual} — refusing to install"
        );
    }
    Ok(())
}

/// Extract the real onnxruntime shared library from the downloaded `.tgz`.
///
/// The archive carries `.../lib/libonnxruntime.<ver>.{dylib,so}` as a regular
/// file plus an unversioned symlink; we want the regular file's bytes and must
/// avoid the `libonnxruntime_providers_*` siblings (they start with an
/// underscore, so `"libonnxruntime."` prefix-matching excludes them).
fn extract_lib(archive: &[u8]) -> Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    for entry in tar.entries().context("reading onnxruntime archive")? {
        let mut entry = entry.context("reading archive entry")?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue; // skip the unversioned symlink
        }
        let name = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let is_lib = name.starts_with("libonnxruntime.")
            && (name.contains(".dylib") || name.contains(".so"));
        if is_lib {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .context("reading library bytes")?;
            if !buf.is_empty() {
                return Ok(buf);
            }
        }
    }
    bail!("libonnxruntime not found in onnxruntime archive")
}

/// Download, verify, and install onnxruntime to the managed path, then point
/// `ort` at it. `progress` prints one-line status to stderr.
pub fn download(progress: bool) -> Result<PathBuf> {
    let target = detect_target().ok_or_else(|| {
        anyhow!(
            "no prebuilt onnxruntime for this platform ({}/{}); set {ORT_DYLIB_ENV} \
             to an onnxruntime {ORT_VERSION} library manually",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let dir = managed_dir().ok_or_else(|| anyhow!("cannot resolve the icm data directory"))?;
    let lib_path = dir.join(target.lib_name);

    let asset = format!("onnxruntime-{}-{ORT_VERSION}.tgz", target.asset);
    let url = format!(
        "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/{asset}"
    );
    if progress {
        eprintln!("Downloading {asset} (~7 MB, one time)…");
    }
    let bytes = download_bytes(&url)?;
    verify_sha(&bytes, target.sha256, &asset)?;
    let lib_bytes = extract_lib(&bytes)?;

    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Write to a temp sibling then rename, so a concurrent reader never sees a
    // half-written library.
    let tmp = lib_path.with_extension("new");
    std::fs::write(&tmp, &lib_bytes).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .context("setting permissions on onnxruntime library")?;
    }
    std::fs::rename(&tmp, &lib_path)
        .with_context(|| format!("installing {}", lib_path.display()))?;

    // The user has opted in: clear any prior "declined" marker.
    if let Some(marker) = declined_marker() {
        let _ = std::fs::remove_file(marker);
    }
    std::env::set_var(ORT_DYLIB_ENV, &lib_path);
    if progress {
        eprintln!(
            "Installed onnxruntime {ORT_VERSION} → {}",
            lib_path.display()
        );
    }
    Ok(lib_path)
}

/// Interactive `[y/N]` confirmation, prompted on **stderr** so it still reaches
/// the user when stdout is piped (e.g. `icm recall … | grep`).
fn confirm(prompt: &str) -> bool {
    eprint!("{prompt} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).is_err() {
        return false;
    }
    matches!(buf.trim().chars().next(), Some('y' | 'Y'))
}

/// Resolve the runtime for a normal CLI run. Returns whether semantic
/// embeddings can be used (else the caller degrades to keyword-only). Prompts
/// for a one-time download only when `interactive` is true.
pub fn ensure_for_run(interactive: bool) -> bool {
    if activate_if_present() {
        return true;
    }
    if !interactive || detect_target().is_none() {
        return false;
    }
    // Respect a previous decline.
    if let Some(marker) = declined_marker() {
        if marker.exists() {
            return false;
        }
    }
    let prompt = format!(
        "Enable semantic (vector) search? This downloads ONNX Runtime {ORT_VERSION} \
         (~7 MB, one time). Download now?"
    );
    if confirm(&prompt) {
        match download(true) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("onnxruntime download failed: {e}");
                eprintln!("Continuing with keyword-only search. Retry: icm embeddings download");
                false
            }
        }
    } else {
        // Persist the decline so we don't ask again every run.
        if let Some(marker) = declined_marker() {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, b"user declined the onnxruntime download\n");
        }
        eprintln!("Continuing with keyword-only search. Enable later: icm embeddings download");
        false
    }
}

/// `icm embeddings download` — force a (re)install, non-interactively.
pub fn cmd_download() -> Result<()> {
    let path = download(true)?;
    println!(
        "Semantic search enabled (onnxruntime {ORT_VERSION} at {}).",
        path.display()
    );
    Ok(())
}

/// `icm embeddings status` — report the runtime state.
pub fn cmd_status() {
    if user_set_dylib() {
        let p = std::env::var(ORT_DYLIB_ENV).unwrap_or_default();
        println!("onnxruntime: using {ORT_DYLIB_ENV} override ({p})");
        return;
    }
    match managed_lib_path() {
        Some(p) if p.is_file() => {
            println!("onnxruntime {ORT_VERSION}: installed ({})", p.display());
        }
        Some(_) => {
            println!(
                "onnxruntime {ORT_VERSION}: not installed — run `icm embeddings download` \
                 to enable semantic search (keyword search works without it)"
            );
        }
        None => {
            println!(
                "onnxruntime: no prebuilt runtime for this platform ({}/{}); set \
                 {ORT_DYLIB_ENV} to an onnxruntime {ORT_VERSION} library manually",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_target_matches_host_lib_extension() {
        // On the supported CI/dev hosts (macOS/Linux, x86_64/aarch64) we expect
        // a target whose canonical lib name matches the OS.
        if let Some(t) = detect_target() {
            if cfg!(target_os = "macos") {
                assert_eq!(t.lib_name, "libonnxruntime.dylib");
                assert!(t.asset.starts_with("osx-"));
            } else if cfg!(target_os = "linux") {
                assert_eq!(t.lib_name, "libonnxruntime.so");
                assert!(t.asset.starts_with("linux-"));
            }
            assert_eq!(t.sha256.len(), 64, "pinned sha256 must be 64 hex chars");
        }
    }

    #[test]
    fn managed_paths_are_version_scoped() {
        if let Some(p) = managed_lib_path() {
            let s = p.to_string_lossy();
            assert!(
                s.contains("onnxruntime"),
                "path should be under onnxruntime/: {s}"
            );
            assert!(
                s.contains(ORT_VERSION),
                "path should be version-scoped: {s}"
            );
        }
    }

    #[test]
    fn sha_mismatch_is_rejected() {
        assert!(verify_sha(b"hello", &sha256_hex(b"hello"), "x").is_ok());
        assert!(verify_sha(b"hello", &sha256_hex(b"world"), "x").is_err());
    }

    #[test]
    fn extract_lib_picks_versioned_regular_file() {
        // Build a tiny .tgz: a providers sibling (must be ignored) + the real
        // versioned library. No symlink entry (tar builder appends regular
        // files), which still exercises the prefix/underscore filtering.
        use flate2::write::GzEncoder;
        use flate2::Compression;

        fn tar_entry(builder: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder.append_data(&mut header, path, data).unwrap();
        }

        let mut builder = tar::Builder::new(Vec::new());
        tar_entry(
            &mut builder,
            "onnxruntime-osx-arm64-1.20.1/lib/libonnxruntime_providers_shared.dylib",
            b"PROVIDER",
        );
        tar_entry(
            &mut builder,
            "onnxruntime-osx-arm64-1.20.1/lib/libonnxruntime.1.20.1.dylib",
            b"REAL-LIBRARY-BYTES",
        );
        let tar_bytes = builder.into_inner().unwrap();

        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&tar_bytes).unwrap();
        let tgz = gz.finish().unwrap();

        let extracted = extract_lib(&tgz).expect("should find the versioned library");
        assert_eq!(extracted, b"REAL-LIBRARY-BYTES");
    }
}
