//! Timestamped backup of every file uninstall mutates.
//!
//! Layout (Windows-safe ISO timestamp with `-` instead of `:`):
//!
//! ```text
//! ~/.icm-uninstall-backups/2026-05-20T14-32-05/
//!   files/
//!     home/patrick/.claude/settings.json     (mirrors the original tree)
//!     home/patrick/.codex/config.toml
//!     ...
//!   manifest.json                            (sha256 per staged file)
//! ```
//!
//! Restore is `cp -a <backup>/files/. /` — the relative tree under
//! `files/` mirrors the original absolute paths with the leading `/`
//! stripped, so a recursive copy lands every config back where it was.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One backup session, scoped to a single `icm uninstall` run. The
/// session is created lazily — if the run turns out to have zero hits we
/// never touch disk.
pub(crate) struct BackupSession {
    /// Root of this session: `~/.icm-uninstall-backups/<ts>/`.
    root: PathBuf,
    /// Subdirectory under the root that mirrors the original tree.
    files_root: PathBuf,
    /// Recorded entries — keyed by the original absolute path.
    entries: BTreeMap<PathBuf, BackupEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackupEntry {
    original_path: PathBuf,
    backup_path: PathBuf,
    sha256: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    /// ISO timestamp string this session was created at.
    timestamp: String,
    /// Total number of files staged.
    files: usize,
    /// One entry per staged file.
    entries: Vec<BackupEntry>,
}

impl BackupSession {
    /// Build a session rooted at `<base>/<ISO-ts>/`. When `override_root`
    /// is `None`, `<base>` defaults to `~/.icm-uninstall-backups/`.
    pub(crate) fn new(override_root: Option<&Path>, home: &Path) -> Result<Self> {
        let base = match override_root {
            Some(p) => p.to_path_buf(),
            None => home.join(".icm-uninstall-backups"),
        };
        let ts = iso_timestamp_no_colons();
        let root = base.join(&ts);
        let files_root = root.join("files");
        std::fs::create_dir_all(&files_root)
            .with_context(|| format!("cannot create backup root at {}", root.display()))?;
        Ok(Self {
            root,
            files_root,
            entries: BTreeMap::new(),
        })
    }

    /// The absolute path to the session root (used in the final report).
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Stage one file. Idempotent: a second call for the same path is a
    /// no-op (the first stage already captured the pre-mutation state).
    /// Silently skips paths that don't exist on disk (they had nothing to
    /// back up — the mutator catches the discrepancy via the hit list).
    pub(crate) fn stage(&mut self, original: &Path) -> Result<()> {
        let canonical = original.to_path_buf();
        if self.entries.contains_key(&canonical) {
            return Ok(());
        }
        if !canonical.exists() {
            return Ok(());
        }
        // Avoid following symlinks during the stat — preserves the
        // user's intent if `~/.claude` is symlinked into a dotfiles repo.
        let meta = std::fs::symlink_metadata(&canonical)
            .with_context(|| format!("cannot stat {}", canonical.display()))?;
        if meta.file_type().is_symlink() {
            // Don't dereference; record but don't copy contents. The
            // mutator will leave such paths alone.
            return Ok(());
        }
        if !meta.is_file() {
            return Ok(()); // dirs handled by stage_dir
        }
        let backup_path = self.backup_path_for(&canonical);
        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create backup parent {}", parent.display()))?;
        }
        std::fs::copy(&canonical, &backup_path).with_context(|| {
            format!(
                "cannot copy {} -> {}",
                canonical.display(),
                backup_path.display()
            )
        })?;
        let bytes = meta.len();
        let sha = sha256_of(&canonical)?;
        self.entries.insert(
            canonical.clone(),
            BackupEntry {
                original_path: canonical,
                backup_path,
                sha256: sha,
                bytes,
            },
        );
        Ok(())
    }

    /// Stage every regular file under `dir` recursively. Used for data
    /// directories before `--purge-data` deletion.
    pub(crate) fn stage_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }
        if meta.is_file() {
            return self.stage(dir);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            let m = std::fs::symlink_metadata(&p)?;
            if m.file_type().is_symlink() {
                continue;
            }
            if m.is_file() {
                self.stage(&p)?;
            } else if m.is_dir() {
                self.stage_dir(&p)?;
            }
        }
        Ok(())
    }

    /// Write the manifest. Should be called once at the end of a run,
    /// after every stage() / mutation pair completed. Cheap even with
    /// zero staged files — produces a valid empty manifest.
    pub(crate) fn commit_manifest(&self) -> Result<()> {
        let manifest = Manifest {
            timestamp: self
                .root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            files: self.entries.len(),
            entries: self.entries.values().cloned().collect(),
        };
        let path = self.root.join("manifest.json");
        let json = serde_json::to_string_pretty(&manifest)?;
        std::fs::write(&path, json)
            .with_context(|| format!("cannot write manifest {}", path.display()))?;
        Ok(())
    }

    /// Map an absolute path to its location under `<root>/files/`. On
    /// Windows the drive letter prefix is preserved as a path segment so
    /// `C:\Users\x\.claude.json` lands under `files/C/Users/x/...`.
    fn backup_path_for(&self, original: &Path) -> PathBuf {
        let mut rel = PathBuf::new();
        for c in original.components() {
            match c {
                std::path::Component::Prefix(p) => {
                    let s = p.as_os_str().to_string_lossy();
                    // Strip the trailing `:` from a Windows drive prefix.
                    let cleaned: String = s.chars().filter(|c| *c != ':').collect();
                    if !cleaned.is_empty() {
                        rel.push(cleaned);
                    }
                }
                std::path::Component::RootDir => {
                    // Skip — `files/` is already the root anchor.
                }
                std::path::Component::CurDir | std::path::Component::ParentDir => {
                    // Should not appear in a canonical path; ignore.
                }
                std::path::Component::Normal(s) => {
                    rel.push(s);
                }
            }
        }
        self.files_root.join(rel)
    }
}

/// `YYYY-MM-DDTHH-MM-SS` in UTC — colons replaced with dashes for
/// Windows-safe directory names. Date is computed manually so we avoid
/// pulling `chrono` into a hot path that already costs nothing.
fn iso_timestamp_no_colons() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymdhms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}-{mi:02}-{s:02}")
}

/// Civil-from-days, adapted from Howard Hinnant's algorithm. Produces a
/// UTC timestamp with no DST/locale dependency.
fn epoch_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let sec_of_day = secs % 86_400;
    let h = (sec_of_day / 3600) as u32;
    let mi = ((sec_of_day % 3600) / 60) as u32;
    let s = (sec_of_day % 60) as u32;

    let z = days + 719_468; // shift epoch to 0000-03-01
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146097]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = if mo <= 2 { y + 1 } else { y };
    (y as i32, mo, d, h, mi, s)
}

fn sha256_of(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_preserves_directory_layout_under_files_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let original = home.join(".claude/settings.json");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, "{\"hooks\": {}}").unwrap();

        let backup_base = tmp.path().join(".icm-uninstall-backups");
        let mut session = BackupSession::new(Some(&backup_base), &home).unwrap();
        session.stage(&original).unwrap();
        session.commit_manifest().unwrap();

        // The staged file's relative path under files/ should mirror the
        // original absolute path (leading slash stripped).
        let canonical_rel = original
            .strip_prefix("/")
            .unwrap_or_else(|_| original.as_path());
        let expected = session.files_root.join(canonical_rel);
        assert!(
            expected.exists(),
            "expected staged copy at {}",
            expected.display()
        );

        // Manifest must list this entry.
        let mf_path = session.root().join("manifest.json");
        assert!(mf_path.exists(), "manifest.json was not written");
        let mf: Manifest =
            serde_json::from_str(&std::fs::read_to_string(mf_path).unwrap()).unwrap();
        assert_eq!(mf.files, 1);
        assert_eq!(mf.entries[0].original_path, original);
        assert!(!mf.entries[0].sha256.is_empty());
    }

    #[test]
    fn backup_stage_is_idempotent_for_same_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let original = home.join("a.json");
        std::fs::write(&original, "v1").unwrap();
        let mut session = BackupSession::new(Some(&home.join("bk")), &home).unwrap();
        session.stage(&original).unwrap();
        // Mutate the source so a naïve second copy would overwrite v1.
        std::fs::write(&original, "v2").unwrap();
        session.stage(&original).unwrap(); // no-op
        let staged = session.backup_path_for(&original);
        assert_eq!(std::fs::read_to_string(staged).unwrap(), "v1");
    }

    #[test]
    fn backup_skips_nonexistent_paths_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut session = BackupSession::new(Some(&home.join("bk")), &home).unwrap();
        session.stage(&home.join("does/not/exist.json")).unwrap();
        assert_eq!(session.entries.len(), 0);
    }

    #[test]
    fn iso_timestamp_is_windows_safe() {
        let ts = iso_timestamp_no_colons();
        assert!(!ts.contains(':'), "timestamp must not contain colons: {ts}");
        assert_eq!(ts.len(), "YYYY-MM-DDTHH-MM-SS".len());
    }

    #[test]
    fn epoch_to_ymdhms_known_reference_point() {
        // 1700000000 = 2023-11-14T22:13:20 UTC
        let (y, mo, d, h, mi, s) = epoch_to_ymdhms(1_700_000_000);
        assert_eq!((y, mo, d, h, mi, s), (2023, 11, 14, 22, 13, 20));
    }
}
