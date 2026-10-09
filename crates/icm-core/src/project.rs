//! Canonical project-name detection, shared by every surface that tags or
//! filters memories by project (CLI hooks, MCP recall, HTTP API).
//!
//! The CLI hooks used to derive the project from the git remote (worktree-
//! and rename-proof) while the MCP server used the cwd basename. A renamed
//! checkout could then store memories under one project name and recall
//! under another, silently returning nothing. Both sides now share this
//! module.

/// Extract a project name from a git remote URL.
/// Handles HTTPS ("https://github.com/user/repo.git"),
/// slash-SSH ("git@github.com:user/repo.git"), and
/// colon-only SSH ("git@host:repo.git") formats.
pub fn repo_name_from_url(url: &str) -> Option<String> {
    // A trailing slash would otherwise make the final `/`-segment empty and
    // defeat remote detection (e.g. "https://host/user/repo/").
    let trimmed = url.trim_end_matches('/');
    // rsplit('/') always yields ≥1 element; split on ':' afterwards to
    // handle SCP-style SSH URLs that have no slash before the repo name.
    let after_slash = trimmed.rsplit('/').next().unwrap_or(trimmed);
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
    let p = std::path::Path::new(path);

    if path.is_empty() {
        return None;
    }

    if let Ok(repo) = gix::discover(p) {
        // Prefer origin remote URL: the most unique identifier, stable
        // across worktrees and renamed checkouts.
        if let Ok(remote) = repo.find_remote("origin")
            && let Some(url) = remote.url(gix::remote::Direction::Fetch)
        {
            let url_str = url.to_bstring().to_string();
            if let Some(name) = repo_name_from_url(&url_str) {
                return Some(name);
            }
        }

        // Worktree fallback: common_dir() always points to the main repo's
        // .git, so its parent is the main repo root regardless of worktree
        // depth. canonicalize resolves `..` components that gix leaves in
        // the path for linked worktrees (git_dir.join("../..") from the
        // commondir file).
        let common = repo.common_dir();
        let canon;
        let common = match std::fs::canonicalize(common) {
            Ok(c) => {
                canon = c;
                canon.as_path()
            }
            Err(_) => common,
        };
        if let Some(name) = common.parent().and_then(|r| r.file_name()) {
            return Some(name.to_string_lossy().to_string());
        }
    }

    // Last resort: basename of the path itself.
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
        // tempdir basename is a random name, not "myproject", so the remote wins
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
    /// Returns `(base_tempdir, worktree_path)`. Keep `base` alive for the
    /// lifetime of the test, or git cleans up the underlying directory.
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

    #[test]
    fn project_from_path_uses_repo_name_for_remote_less_subdir() {
        let base = tempfile::tempdir().unwrap();
        let repo = base.path().join("norepo");
        std::fs::create_dir(&repo).unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let subdir = repo.join("docs");
        std::fs::create_dir(&subdir).unwrap();
        // No origin remote, so common_dir must resolve to the repo root
        // (not the subdirectory basename).
        assert_eq!(
            project_from_path(subdir.to_str().unwrap()),
            Some("norepo".into())
        );
    }
}
