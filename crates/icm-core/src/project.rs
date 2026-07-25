//! Canonical project-name detection, shared by every surface that tags or
//! filters memories by project (CLI hooks, MCP recall, HTTP API).
//!
//! Audit finding: the CLI hooks derived the project from the git remote
//! (worktree- and rename-proof) while the MCP server used the cwd basename —
//! so a renamed checkout stored memories under one project name and recalled
//! under another, silently returning nothing. Both sides now share this
//! module.

/// Extract a project name from a git remote URL.
/// Handles HTTPS ("https://github.com/user/repo.git"),
/// slash-SSH ("git@github.com:user/repo.git"), and
/// colon-only SSH ("git@host:repo.git") formats.
pub fn repo_name_from_url(url: &str) -> Option<String> {
    // rsplit('/') always yields ≥1 element; split on ':' afterwards to
    // handle SCP-style SSH URLs that have no slash before the repo name.
    let after_slash = url.rsplit('/').next().unwrap_or(url);
    let name = after_slash
        .rsplit(':')
        .next()
        .unwrap_or(after_slash)
        .trim_end_matches(".git");
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Extract a project name from a filesystem path (basename), treating empty
/// or root paths as "no project".
pub fn project_from_path(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let p = std::path::Path::new(path);

    // Try git remote get-url origin (most unique identifier)
    if let Ok(out) = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(p)
        .output()
    {
        if out.status.success() {
            let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Some(name) = repo_name_from_url(&url) {
                return Some(name);
            }
        }
    }

    // For worktrees without a remote: git-common-dir returns the main repo's
    // .git as an absolute path, so its parent basename is the real project name.
    if let Ok(out) = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(p)
        .output()
    {
        if out.status.success() {
            let raw = out.stdout;
            let common = std::str::from_utf8(&raw).unwrap_or("").trim();
            let common_path = std::path::Path::new(common);
            if common_path.is_absolute() {
                if let Some(name) = common_path.parent().and_then(|r| r.file_name()) {
                    return Some(name.to_string_lossy().to_string());
                }
            }
        }
    }

    // Fallback: basename of the path itself
    p.file_name().map(|n| n.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_from_path_extracts_basename() {
        assert_eq!(
            project_from_path("/Users/patrick/dev/rtk-ai/icm"),
            Some("icm".into())
        );
        assert_eq!(
            project_from_path("/tmp/my-project"),
            Some("my-project".into())
        );
        assert_eq!(project_from_path(""), None);
    }

    #[test]
    fn project_from_path_uses_git_remote_over_basename() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/user/myproject.git",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // tempdir basename is a random name, not "myproject" — remote must win
        assert_eq!(
            project_from_path(dir.path().to_str().unwrap()),
            Some("myproject".into())
        );
    }

    #[test]
    fn project_from_path_handles_ssh_remote() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:user/sshproject.git",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            project_from_path(dir.path().to_str().unwrap()),
            Some("sshproject".into())
        );
    }

    #[test]
    fn repo_name_from_url_handles_scp_ssh_without_slash() {
        // git@host:repo.git — no slash between host and repo name
        assert_eq!(repo_name_from_url("git@host:repo.git"), Some("repo".into()));
        assert_eq!(
            repo_name_from_url("git@github.com:user/repo.git"),
            Some("repo".into())
        );
        assert_eq!(
            repo_name_from_url("https://github.com/user/repo.git"),
            Some("repo".into())
        );
        assert_eq!(repo_name_from_url(""), None);
    }

    /// Creates a git repo named "mainproject" with a worktree at "w1".
    /// Returns `(base_tempdir, worktree_path)` — keep `base` alive for the
    /// lifetime of the test or git will clean up the underlying directory.
    fn make_worktree() -> (tempfile::TempDir, std::path::PathBuf) {
        let base = tempfile::tempdir().unwrap();
        let main_repo = base.path().join("mainproject");
        std::fs::create_dir(&main_repo).unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
            vec!["commit", "--allow-empty", "-m", "init"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&main_repo)
                .output()
                .unwrap();
        }
        let worktree = base.path().join("w1");
        std::process::Command::new("git")
            .args(["worktree", "add", "--detach", worktree.to_str().unwrap()])
            .current_dir(&main_repo)
            .output()
            .unwrap();
        (base, worktree)
    }

    #[test]
    fn project_from_path_uses_main_repo_name_for_worktree() {
        let (_base, worktree) = make_worktree();
        // w1 basename would give "w1"; must resolve to "mainproject" instead
        assert_eq!(
            project_from_path(worktree.to_str().unwrap()),
            Some("mainproject".into())
        );
    }
}
