//! icm installer — downloads, verifies, and installs the latest icm release.
//!
//! Usage: ./icm-install [--dir <install-dir>] [--version <tag>]
//!
//! Defaults:
//!   --dir     ~/.local/bin
//!   --version latest

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

const REPO: &str = "rtk-ai/icm";
const BINARY_NAME: &str = "icm";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut install_dir: Option<PathBuf> = None;
    let mut version_tag: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => {
                install_dir = Some(PathBuf::from(args.next().context("--dir requires a path")?));
            }
            "--version" => {
                version_tag = Some(args.next().context("--version requires a tag")?);
            }
            "--help" | "-h" => {
                println!("icm-install — install icm from GitHub releases");
                println!();
                println!("Usage: icm-install [--dir <path>] [--version <tag>]");
                println!();
                println!("Options:");
                println!("  --dir <path>      Install directory (default: ~/.local/bin)");
                println!("  --version <tag>   Release tag (default: latest)");
                return Ok(());
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let install_dir = install_dir.unwrap_or_else(default_install_dir);
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("cannot create {}", install_dir.display()))?;

    let tag = match version_tag {
        Some(t) => t,
        None => {
            info("Fetching latest release...");
            fetch_latest_tag()?
        }
    };

    let version = tag.strip_prefix("icm-v").unwrap_or(&tag);
    info(&format!("Installing icm {version}"));

    let (target, ext) = detect_target()?;
    let archive_name = format!("{BINARY_NAME}-{target}.{ext}");
    let base_url = format!("https://github.com/{REPO}/releases/download/{tag}");

    info(&format!("Downloading {archive_name}..."));
    let archive_bytes = download(&format!("{base_url}/{archive_name}"))?;

    info("Verifying checksum...");
    let checksums = String::from_utf8(download(&format!("{base_url}/checksums.txt"))?)
        .context("checksums.txt is not valid UTF-8")?;
    let expected = parse_sha(&checksums, &archive_name)?;
    let actual = sha256(&archive_bytes);
    if expected != actual {
        bail!("SHA256 mismatch — aborting\n  expected: {expected}\n  got:      {actual}");
    }
    info("SHA256 OK");

    info("Extracting...");
    let binary = extract(&archive_bytes, ext == "zip")?;

    let dest = install_dir.join(if ext == "zip" {
        format!("{BINARY_NAME}.exe")
    } else {
        BINARY_NAME.to_string()
    });

    {
        let mut f = std::fs::File::create(&dest)
            .with_context(|| format!("cannot write to {}", dest.display()))?;
        f.write_all(&binary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
        }
    }

    info(&format!("Installed to {}", dest.display()));
    println!();
    println!("  Next steps:");
    println!("    1. icm init            # configure your AI tools (MCP)");
    println!("    2. icm init --mode hook  # install Claude Code hooks");
    println!("    3. Restart your AI tool to activate");
    println!();

    // Warn if install dir is not in PATH
    let in_path = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|p| install_dir.as_path() == Path::new(p));
    if !in_path {
        eprintln!(
            "warning: {} is not in your PATH. Add it with:",
            install_dir.display()
        );
        eprintln!("  export PATH=\"{}:$PATH\"", install_dir.display());
    }

    Ok(())
}

fn default_install_dir() -> PathBuf {
    dirs_home()
        .map(|h| h.join(".local/bin"))
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn detect_target() -> Result<(&'static str, &'static str)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let suffix = match os {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        _ => bail!("unsupported OS: {os}"),
    };
    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => bail!("unsupported architecture: {arch}"),
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    let target = Box::leak(format!("{arch}-{suffix}").into_boxed_str());
    Ok((target, ext))
}

fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        .set("User-Agent", "icm-install")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("failed to fetch latest release")?;
    let json: serde_json::Value = resp.into_json()?;
    json.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("missing tag_name in GitHub API response"))
}

fn download(url: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let resp = ureq::get(url)
        .set("User-Agent", "icm-install")
        .call()
        .with_context(|| format!("failed to download {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf)?;
    Ok(buf)
}

fn sha256(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

fn parse_sha(checksums: &str, filename: &str) -> Result<String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(hash), Some(name)) = (parts.next(), parts.next()) {
            if name == filename {
                return Ok(hash.to_string());
            }
        }
    }
    bail!("no checksum found for {filename}")
}

fn extract(archive: &[u8], is_zip: bool) -> Result<Vec<u8>> {
    use std::io::Read;
    if is_zip {
        bail!("zip extraction not supported on this platform");
    }
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.file_name().and_then(|n| n.to_str()) == Some(BINARY_NAME) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("binary {BINARY_NAME} not found in archive")
}

fn info(msg: &str) {
    println!("\x1b[32m[INFO]\x1b[0m {msg}");
}
