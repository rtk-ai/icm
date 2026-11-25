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
//! Safety: `ort`'s version check compares the loaded library's minor version
//! against `ORT_API_VERSION` (17 + one per enabled `api-N` feature) and
//! rejects anything *older* under `panic = "abort"` — fastembed enables
//! `api-24`, so this build requires ONNX Runtime >= **1.24**. We pin the
//! latest stable release satisfying that, verify a hard-coded SHA256, and
//! trust *only* our managed copy (or an explicit user `ORT_DYLIB_PATH`) —
//! never an ambient system library.
//!
//! Source: Microsoft's official `Microsoft.ML.OnnxRuntime` NuGet package. It
//! holds the native library of every platform under
//! `runtimes/<rid>/native/`, so it is large (~150 MB); we read only the zip
//! central directory and the one entry we need through HTTP range requests
//! (~6-12 MB). The GitHub release archives used before are gone: the
//! `microsoft/onnxruntime` repository returns 404 since November 2026.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

/// onnxruntime release to fetch. MUST stay ABI-compatible with the `ort`
/// version fastembed pins (currently `ort` 2.0.0-rc.13, `api-24` feature →
/// requires ONNX Runtime >= 1.24; pinned to the latest stable release at
/// the time of the fastembed 4->6 bump).
const ORT_VERSION: &str = "1.29.0";

const ORT_DYLIB_ENV: &str = "ORT_DYLIB_PATH";

/// URL of the official `Microsoft.ML.OnnxRuntime` NuGet package for `version`.
fn nupkg_url(version: &str) -> String {
    format!(
        "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/{version}/\
         microsoft.ml.onnxruntime.{version}.nupkg"
    )
}

/// A supported download target: the NuGet runtime identifier, the pinned
/// SHA256 of the shared library itself (as extracted from the package), the
/// canonical shared-library filename that ort's load-dynamic backend looks
/// for (also its name inside the package), and a human-readable download size
/// for the first-launch prompt.
struct Target {
    rid: &'static str,
    sha256: &'static str,
    lib_name: &'static str,
    approx_size: &'static str,
}

impl Target {
    /// Path of the library inside the NuGet package.
    fn entry_name(&self) -> String {
        format!("runtimes/{}/native/{}", self.rid, self.lib_name)
    }
}

/// Resolve the onnxruntime target for a given `(os, arch)`, or `None` if
/// unsupported. Windows-on-ARM and exotic arches are deferred (issue #345).
/// `macos`/`x86_64` (Intel Mac) is deferred too as of the 1.29.0 pin: upstream
/// onnxruntime stopped publishing `osx-x86_64` prebuilt binaries starting
/// with the first release in our required range (1.24.0) — the 1.29.0 NuGet
/// package has no `osx-x64` runtime either. An Intel-Mac
/// `embeddings-dynamic` build still works via an explicit `ORT_DYLIB_PATH`
/// to a self-built or Homebrew onnxruntime; only the automatic
/// download-on-first-use path is unavailable.
/// Split out from [`detect_target`] so the target table is unit-testable across
/// platforms from any host.
fn target_for(os: &str, arch: &str) -> Option<Target> {
    // (rid, sha256 of the library, approx_size of its compressed entry) — the
    // shared-library name is derived from `os` below.
    let (rid, sha256, approx_size) = match (os, arch) {
        ("macos", "aarch64") => (
            "osx-arm64",
            "c04fe65021445904a3cae047272cad05e648282c75bf1f9eb7b3440120ae13dc",
            "~12 MB",
        ),
        ("linux", "x86_64") => (
            "linux-x64",
            "5715f06d8992ca8eeeddcce43df3a7d38f97d537052126f558e912cb312460ca",
            "~11 MB",
        ),
        ("linux", "aarch64") => (
            "linux-arm64",
            "a27d21126db312aa8f02f3d5eaebe466e991f51f469882e6d0407d5a8b64afda",
            "~10 MB",
        ),
        ("windows", "x86_64") => (
            "win-x64",
            "69d8e6d3879a3b4001cdc74c8ed9ccc7e7f799a5b847059738323404519ec471",
            "~6 MB",
        ),
        _ => return None,
    };
    let lib_name = match os {
        "windows" => "onnxruntime.dll",
        "macos" => "libonnxruntime.dylib",
        _ => "libonnxruntime.so",
    };
    Some(Target {
        rid,
        sha256,
        lib_name,
        approx_size,
    })
}

/// Resolve the onnxruntime asset for the host platform.
fn detect_target() -> Option<Target> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
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
    if let Some(lib) = managed_lib_path()
        && lib.is_file()
    {
        // SAFETY: reached from `main` before the embedder, or any other
        // thread, is started: nothing reads the environment concurrently.
        unsafe { std::env::set_var(ORT_DYLIB_ENV, &lib) };
        return true;
    }
    false
}

const USER_AGENT: &str = "icm-onnxruntime-fetch";

/// Size of one ranged read. The zip reader seeks to the central directory at
/// the end of the package, then streams the one entry we need; reading in
/// 1 MiB blocks keeps that to a handful of requests.
const RANGE_BLOCK: u64 = 1 << 20;

/// GET `bytes=start-end` (inclusive) of `url`. Returns the body and the total
/// size of the resource, parsed from `Content-Range`. Fails if the server
/// ignores the range: downloading the whole ~150 MB package is not an option.
fn http_range(url: &str, start: u64, end: u64) -> Result<(Vec<u8>, u64)> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Range", &format!("bytes={start}-{end}"))
        .call()
        .with_context(|| format!("failed to download {url}"))?;
    if resp.status() != 206 {
        bail!(
            "{url} answered {} instead of 206 Partial Content: byte ranges unsupported",
            resp.status()
        );
    }
    let total = resp
        .header("Content-Range")
        .and_then(|v| v.rsplit('/').next())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .ok_or_else(|| anyhow!("{url}: missing or invalid Content-Range header"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .context("failed to read download body")?;
    Ok((buf, total))
}

/// `Read + Seek` over a remote file of known length, fetched lazily in
/// [`RANGE_BLOCK`]-sized blocks through `fetch(start, end_inclusive)`. Lets
/// the `zip` crate parse a remote archive while downloading only what it
/// actually reads.
struct RangeReader<F> {
    fetch: F,
    len: u64,
    pos: u64,
    blocks: HashMap<u64, Vec<u8>>,
    /// Bytes actually downloaded, for the progress line.
    fetched: u64,
}

impl<F: FnMut(u64, u64) -> Result<Vec<u8>>> RangeReader<F> {
    fn new(len: u64, fetch: F) -> Self {
        Self {
            fetch,
            len,
            pos: 0,
            blocks: HashMap::new(),
            fetched: 0,
        }
    }

    fn block(&mut self, idx: u64) -> std::io::Result<&[u8]> {
        if !self.blocks.contains_key(&idx) {
            let start = idx * RANGE_BLOCK;
            let end = (start + RANGE_BLOCK).min(self.len) - 1;
            let data = (self.fetch)(start, end).map_err(std::io::Error::other)?;
            if data.len() as u64 != end - start + 1 {
                return Err(std::io::Error::other(format!(
                    "short range read: asked {start}-{end}, got {} bytes",
                    data.len()
                )));
            }
            self.fetched += data.len() as u64;
            self.blocks.insert(idx, data);
        }
        Ok(&self.blocks[&idx])
    }
}

impl<F: FnMut(u64, u64) -> Result<Vec<u8>>> Read for RangeReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let idx = self.pos / RANGE_BLOCK;
        let off = (self.pos % RANGE_BLOCK) as usize;
        let block = self.block(idx)?;
        let n = buf.len().min(block.len() - off);
        buf[..n].copy_from_slice(&block[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<F> Seek for RangeReader<F> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = target.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start")
        })?;
        Ok(self.pos)
    }
}

/// Read `entry` out of a zip archive (the NuGet package). The `zip` crate
/// checks the entry's CRC32 once fully read.
fn extract_entry<R: Read + Seek>(archive: R, entry: &str) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(archive).context("reading the onnxruntime package")?;
    let mut file = zip
        .by_name(entry)
        .with_context(|| format!("{entry} not found in the onnxruntime package"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .with_context(|| format!("extracting {entry}"))?;
    if buf.is_empty() {
        bail!("{entry} is empty in the onnxruntime package");
    }
    Ok(buf)
}

/// Fetch only `entry` from the remote zip at `url`. Returns the entry bytes
/// and how many bytes were downloaded.
fn fetch_remote_entry(url: &str, entry: &str) -> Result<(Vec<u8>, u64)> {
    // A one-byte probe gives the package size.
    let (_, len) = http_range(url, 0, 0)?;
    let mut reader = RangeReader::new(len, |start, end| {
        http_range(url, start, end).map(|(data, _)| data)
    });
    let bytes = extract_entry(&mut reader, entry)?;
    Ok((bytes, reader.fetched))
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    icm_core::to_hex(&hasher.finalize())
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

/// Write the verified library bytes to `tmp`, refusing to follow a
/// pre-existing symlink there.
///
/// Audit finding: `tmp` is a fixed, predictable path within the managed
/// dir. Plain `fs::write` is `O_CREAT|O_TRUNC` with no `O_EXCL`, so a
/// pre-planted symlink at `tmp` (another same-user process, shared
/// CI/container) would get followed and its target clobbered with the
/// verified library bytes — the same TOCTOU class already hardened in
/// `config.rs::write_secret_file` and `upgrade.rs::write_new_binary`.
/// Remove any existing entry first via `symlink_metadata` (reports the
/// symlink itself, not its target), then open with `create_new`
/// (`O_EXCL`) so even a fresh symlink racing into the gap fails the open
/// rather than getting followed.
fn write_lib_tmp(tmp: &Path, lib_bytes: &[u8]) -> Result<()> {
    if std::fs::symlink_metadata(tmp).is_ok() {
        std::fs::remove_file(tmp)
            .with_context(|| format!("cannot remove stale {}", tmp.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .with_context(|| format!("cannot create {}", tmp.display()))?;
    f.write_all(lib_bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755))
            .context("setting permissions on onnxruntime library")?;
    }
    Ok(())
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

    let url = nupkg_url(ORT_VERSION);
    let entry = target.entry_name();
    if progress {
        eprintln!(
            "Downloading onnxruntime {ORT_VERSION} for {} ({}, one time)…",
            target.rid, target.approx_size
        );
    }
    let (lib_bytes, fetched) = fetch_remote_entry(&url, &entry)?;
    verify_sha(&lib_bytes, target.sha256, &entry)?;
    if progress {
        eprintln!("Fetched {:.1} MB from api.nuget.org", fetched as f64 / 1e6);
    }

    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Write to a temp sibling then rename, so a concurrent reader never sees a
    // half-written library.
    //
    // Audit finding: this path is fixed and predictable within the managed
    // dir. Plain `fs::write` is `O_CREAT|O_TRUNC` with no `O_EXCL`, so a
    // pre-planted symlink at `tmp` (another same-user process, shared
    // CI/container) would get followed and its target clobbered with the
    // verified library bytes — the same TOCTOU class already hardened in
    // `config.rs::write_secret_file` and `upgrade.rs::write_new_binary`.
    // Remove any existing entry first via `symlink_metadata` (reports the
    // symlink itself, not its target), then open with `create_new`
    // (`O_EXCL`) so even a fresh symlink racing into the gap fails the
    // open rather than getting followed.
    let tmp = lib_path.with_extension("new");
    write_lib_tmp(&tmp, &lib_bytes)?;
    // On Windows `rename` fails if the destination already exists (e.g. a forced
    // re-download), so clear it first. Unix `rename` replaces atomically.
    #[cfg(windows)]
    let _ = std::fs::remove_file(&lib_path);
    std::fs::rename(&tmp, &lib_path)
        .with_context(|| format!("installing {}", lib_path.display()))?;

    // The user has opted in: clear any prior "declined" marker.
    if let Some(marker) = declined_marker() {
        let _ = std::fs::remove_file(marker);
    }
    // SAFETY: reached from `main` before the embedder, or any other
    // thread, is started: nothing reads the environment concurrently.
    unsafe { std::env::set_var(ORT_DYLIB_ENV, &lib_path) };
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
    let Some(target) = detect_target() else {
        return false;
    };
    if !interactive {
        return false;
    }
    // Respect a previous decline.
    if let Some(marker) = declined_marker()
        && marker.exists()
    {
        return false;
    }
    let prompt = format!(
        "Enable semantic (vector) search? This downloads ONNX Runtime {ORT_VERSION} \
         ({}, one time). Download now?",
        target.approx_size
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
                assert!(t.rid.starts_with("osx-"));
            } else if cfg!(target_os = "linux") {
                assert_eq!(t.lib_name, "libonnxruntime.so");
                assert!(t.rid.starts_with("linux-"));
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
    fn target_table_covers_all_supported_platforms() {
        // Every shipped target must have a 64-hex sha, the NuGet runtime
        // identifier, and the canonical lib name ort's load-dynamic backend
        // expects (also its name inside the package).
        let cases = [
            ("macos", "aarch64", "libonnxruntime.dylib", "osx-arm64"),
            ("linux", "x86_64", "libonnxruntime.so", "linux-x64"),
            ("linux", "aarch64", "libonnxruntime.so", "linux-arm64"),
            ("windows", "x86_64", "onnxruntime.dll", "win-x64"),
        ];
        for (os, arch, lib, rid) in cases {
            let t = target_for(os, arch)
                .unwrap_or_else(|| panic!("{os}/{arch} must be a supported target"));
            assert_eq!(t.lib_name, lib, "{os}/{arch} lib name");
            assert_eq!(t.rid, rid, "{os}/{arch} runtime identifier");
            assert_eq!(t.entry_name(), format!("runtimes/{rid}/native/{lib}"));
            assert_eq!(t.sha256.len(), 64, "{os}/{arch} sha must be 64 hex chars");
        }
        // Windows-on-ARM and exotic arches are deferred (issue #345).
        // Intel Mac: upstream onnxruntime stopped shipping osx-x86_64
        // prebuilt archives at 1.24.0, our minimum supported version — see
        // `target_for`'s docs.
        assert!(target_for("windows", "aarch64").is_none());
        assert!(target_for("freebsd", "x86_64").is_none());
        assert!(target_for("macos", "x86_64").is_none());
    }

    /// A tiny zip mirroring the NuGet package layout: a padding entry large
    /// enough to span several range blocks, the providers sibling (must be
    /// ignored) and the real library.
    fn fake_nupkg(lib: &[u8]) -> Vec<u8> {
        use std::io::Cursor;
        use zip::write::SimpleFileOptions;

        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            let deflated = SimpleFileOptions::default();
            w.start_file("runtimes/android/native/onnxruntime.aar", stored)
                .unwrap();
            w.write_all(&vec![7u8; (RANGE_BLOCK * 2 + 123) as usize])
                .unwrap();
            w.start_file(
                "runtimes/linux-x64/native/libonnxruntime_providers_shared.so",
                deflated,
            )
            .unwrap();
            w.write_all(b"PROVIDER").unwrap();
            w.start_file("runtimes/linux-x64/native/libonnxruntime.so", deflated)
                .unwrap();
            w.write_all(lib).unwrap();
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_entry_picks_the_exact_entry() {
        let pkg = fake_nupkg(b"REAL-LIBRARY-BYTES");
        let t = target_for("linux", "x86_64").unwrap();
        let got = extract_entry(std::io::Cursor::new(&pkg), &t.entry_name()).unwrap();
        assert_eq!(got, b"REAL-LIBRARY-BYTES");
        assert!(
            extract_entry(
                std::io::Cursor::new(&pkg),
                "runtimes/osx-arm64/native/libonnxruntime.dylib"
            )
            .is_err()
        );
    }

    #[test]
    fn range_reader_extracts_without_reading_everything() {
        // The library sits after a >2-block padding entry: reading it through
        // the range reader must not download the padding from the start.
        let lib = b"REAL-LIBRARY-BYTES".repeat(1000);
        let pkg = fake_nupkg(&lib);
        let mut calls = Vec::new();
        let mut reader = RangeReader::new(pkg.len() as u64, |start: u64, end: u64| {
            calls.push((start, end));
            Ok(pkg[start as usize..=end as usize].to_vec())
        });
        let got = extract_entry(&mut reader, "runtimes/linux-x64/native/libonnxruntime.so")
            .expect("entry readable through ranges");
        assert_eq!(got, lib);
        assert!(
            reader.fetched < pkg.len() as u64,
            "fetched {} of {} bytes",
            reader.fetched,
            pkg.len()
        );
        drop(reader);
        // Block 0 holds only the start of the padding entry: never fetched.
        // (The zip end-of-archive scan covers the last 64 KiB, which is why
        // the block before the last one may be read.)
        assert!(!calls.contains(&(0, RANGE_BLOCK - 1)), "calls: {calls:?}");
    }

    #[test]
    fn range_reader_rejects_short_reads() {
        let mut reader = RangeReader::new(10, |_, _| Ok(vec![0u8; 3]));
        let mut buf = [0u8; 4];
        assert!(reader.read(&mut buf).is_err());
    }

    #[test]
    fn range_reader_seek_and_eof() {
        let data: Vec<u8> = (0..=255u8).collect();
        let mut reader = RangeReader::new(data.len() as u64, |s: u64, e: u64| {
            Ok(data[s as usize..=e as usize].to_vec())
        });
        assert_eq!(reader.seek(SeekFrom::End(-6)).unwrap(), 250);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![250, 251, 252, 253, 254, 255]);
        assert!(reader.seek(SeekFrom::Current(-1000)).is_err());
    }

    #[test]
    fn nupkg_url_points_at_the_official_package() {
        assert_eq!(
            nupkg_url("1.29.0"),
            "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.29.0/\
             microsoft.ml.onnxruntime.1.29.0.nupkg"
        );
    }

    /// Audit regression: `write_lib_tmp` must not follow a pre-existing
    /// symlink at the fixed `tmp` path — a same-user process (or a
    /// leftover from an interrupted previous download) planting a
    /// symlink there must not have its target clobbered.
    #[cfg(unix)]
    #[test]
    fn write_lib_tmp_does_not_follow_a_preexisting_symlink() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let victim = tmp_dir.path().join("victim.txt");
        std::fs::write(&victim, "untouched").unwrap();

        let target = tmp_dir.path().join("libonnxruntime.1.29.0.dylib.new");
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        write_lib_tmp(&target, b"verified library bytes").unwrap();

        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_file(),
            "target must be a regular file, not still a symlink"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"verified library bytes");
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "untouched",
            "the symlink's old target must be untouched"
        );
    }

    #[test]
    fn write_lib_tmp_creates_a_fresh_file() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let target = tmp_dir.path().join("libonnxruntime.1.29.0.dylib.new");
        write_lib_tmp(&target, b"payload").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
    }
}
