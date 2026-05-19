//! Recursive scan of a project tree for free-form ICM references.
//!
//! `icm init` writes the `<!-- icm:start --> ... <!-- icm:end -->` block
//! into instruction files at the cwd it ran in (CLAUDE.md, AGENTS.md,
//! .windsurfrules, etc.). The main location catalog only covers the
//! current cwd; if the user wants to clean up multiple project clones
//! they point `--scan-dir <PATH>` at the root and we walk from there.

use std::path::Path;

use anyhow::Result;
use walkdir::{DirEntry, WalkDir};

use super::discover::{HitDetail, LocationHit};

const MAX_DEPTH: usize = 8;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    ".tox",
    "__pycache__",
    "dist",
    "build",
    ".next",
];

/// Filenames (without dir) that init creates verbatim. Anything in this
/// list is opened regardless of extension.
const INSTRUCTION_FILENAMES: &[&str] = &[
    "CLAUDE.md",
    "AGENTS.md",
    ".windsurfrules",
    ".aider.conventions.md",
    ".cursorrules",
    "GEMINI.md",
];

/// Extensions worth opening to look for the markdown delimiter.
const INSTRUCTION_EXTS: &[&str] = &["md", "mdc", "markdown"];

const START_MARKER: &str = "<!-- icm:start -->";
const END_MARKER: &str = "<!-- icm:end -->";

/// Walk `root` (depth-limited, skipping noisy directories) and return one
/// `LocationHit` per file that contains the ICM markdown block.
pub(crate) fn scan_dir(root: &Path) -> Result<Vec<LocationHit>> {
    let mut hits = Vec::new();
    if !root.exists() {
        return Ok(hits);
    }
    let walker = WalkDir::new(root)
        .follow_links(false)
        .max_depth(MAX_DEPTH)
        .into_iter()
        .filter_entry(|e| !is_skipped(e));

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // unreadable subtree — skip silently
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_candidate(&entry) {
            continue;
        }
        // Only read the file if its size is reasonable. Skip giant
        // blobs (>4 MiB) — they're never instruction files.
        if entry
            .metadata()
            .map(|m| m.len() > 4 * 1024 * 1024)
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(start) = content.find(START_MARKER) else {
            continue;
        };
        let end_off = content
            .find(END_MARKER)
            .map(|o| o + END_MARKER.len())
            .unwrap_or(content.len());
        let before = &content[..start];
        let after = if end_off <= content.len() {
            &content[end_off..]
        } else {
            ""
        };
        let file_will_be_empty = before.trim().is_empty() && after.trim().is_empty();
        let start_line = content[..start].bytes().filter(|&b| b == b'\n').count() + 1;
        let end_line = content[..end_off.min(content.len())]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1;

        hits.push(LocationHit {
            spec_label: "Project tree (--scan-dir)",
            path: entry.path().to_path_buf(),
            detail: HitDetail::MarkdownBlock {
                start_line,
                end_line,
                file_will_be_empty,
            },
        });
    }
    Ok(hits)
}

fn is_skipped(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    entry
        .file_name()
        .to_str()
        .map(|n| SKIP_DIRS.contains(&n))
        .unwrap_or(false)
}

fn is_candidate(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if INSTRUCTION_FILENAMES.contains(&name.as_ref()) {
        return true;
    }
    let ext = entry
        .path()
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    INSTRUCTION_EXTS.contains(&ext.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write(p: &Path, c: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::File::create(p)
            .unwrap()
            .write_all(c.as_bytes())
            .unwrap();
    }

    #[test]
    fn scan_dir_finds_block_in_nested_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("proj-a/CLAUDE.md"),
            "intro\n<!-- icm:start -->\nx\n<!-- icm:end -->\nrest\n",
        );
        write(
            &root.join("proj-b/sub/AGENTS.md"),
            "<!-- icm:start -->\nonly\n<!-- icm:end -->\n",
        );
        // Decoy: contains the marker but inside a skip-dir.
        write(
            &root.join("proj-a/node_modules/pkg/README.md"),
            "<!-- icm:start -->\nignored\n<!-- icm:end -->\n",
        );
        let hits = scan_dir(root).unwrap();
        // Compare via `Path::ends_with` (component-aware) so the test
        // works on both Unix and Windows path separators.
        assert_eq!(hits.len(), 2, "got: {hits:?}");
        assert!(hits
            .iter()
            .any(|h| h.path.ends_with(Path::new("proj-a").join("CLAUDE.md"))));
        assert!(hits.iter().any(|h| {
            h.path
                .ends_with(Path::new("proj-b").join("sub").join("AGENTS.md"))
        }));
        // Decoy must be ignored. Use components, not raw string.
        assert!(!hits
            .iter()
            .any(|h| h.path.components().any(|c| c.as_os_str() == "node_modules")));
    }

    #[test]
    fn scan_dir_respects_max_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = tmp.path().to_path_buf();
        // Bury the file 12 dirs deep — beyond MAX_DEPTH.
        for i in 0..12 {
            p = p.join(format!("d{i}"));
        }
        write(
            &p.join("CLAUDE.md"),
            "<!-- icm:start -->\nx\n<!-- icm:end -->\n",
        );
        let hits = scan_dir(tmp.path()).unwrap();
        assert!(hits.is_empty(), "deep file unexpectedly found: {hits:?}");
    }

    #[test]
    fn scan_dir_returns_empty_for_missing_root() {
        let tmp = tempfile::tempdir().unwrap();
        let hits = scan_dir(&tmp.path().join("does/not/exist")).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn scan_dir_ignores_files_without_markdown_extension_or_known_name() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("script.sh"),
            "# <!-- icm:start --> trap\n<!-- icm:end -->\n",
        );
        write(
            &tmp.path().join("config.txt"),
            "<!-- icm:start -->\nx\n<!-- icm:end -->\n",
        );
        let hits = scan_dir(tmp.path()).unwrap();
        assert!(hits.is_empty(), "non-markdown files should be ignored");
    }
}
