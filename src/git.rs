//! Git branch detection for statusline rendering.
//!
//! Mirrors `git branch --show-current 2>/dev/null` from the bash predecessor:
//! returns the current branch name on an attached HEAD, `None` on detached
//! HEAD or outside a git repository.
//!
//! Implemented by hand-parsing `.git/HEAD` instead of calling `git` or pulling
//! in libgit2 — keeps the cold-start budget intact (no subprocess fork, no
//! heavy dependency). Supports both regular repos and linked worktrees (where
//! `.git` is a file pointing at `gitdir:`).

use std::path::{Path, PathBuf};

pub fn current_branch(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let content = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    parse_head(&content)
}

fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        let candidate = cur.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            // Worktree case: .git is a file whose first line reads
            // "gitdir: <path-to-real-gitdir>". The path can be absolute or
            // relative to the worktree root.
            let content = std::fs::read_to_string(&candidate).ok()?;
            let rest = content.lines().next()?.strip_prefix("gitdir:")?.trim();
            let path = PathBuf::from(rest);
            return Some(if path.is_absolute() {
                path
            } else {
                cur.join(path)
            });
        }
        cur = cur.parent()?;
    }
}

fn parse_head(content: &str) -> Option<String> {
    content
        .lines()
        .next()?
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

/// Read `.git/config` and return the `origin` remote URL normalized to an
/// HTTPS web URL. Returns None if there is no origin or the URL can't be
/// understood. Used for OSC 8 hyperlinks on the branch segment.
///
/// Worktree-aware: linked worktrees store their HEAD in
/// `<common>/.git/worktrees/<name>/` but `config` only lives in the
/// common gitdir, reached via the `commondir` pointer file.
pub fn origin_web_url(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let common = resolve_common_dir(&git_dir);
    let config = std::fs::read_to_string(common.join("config")).ok()?;
    parse_origin_url(&config).map(|u| normalize_remote_url(&u))
}

/// Return the common gitdir for a given gitdir. For a regular repo it's
/// the gitdir itself; for a linked worktree it's the path recorded in the
/// `commondir` pointer file (resolved relative to the worktree gitdir
/// when it's not absolute).
fn resolve_common_dir(git_dir: &Path) -> PathBuf {
    let commondir_file = git_dir.join("commondir");
    match std::fs::read_to_string(&commondir_file) {
        Ok(content) => resolve_common_dir_from_content(git_dir, &content),
        Err(_) => git_dir.to_path_buf(),
    }
}

fn resolve_common_dir_from_content(git_dir: &Path, content: &str) -> PathBuf {
    match content.lines().next().map(str::trim) {
        Some(first) if !first.is_empty() => {
            let p = PathBuf::from(first);
            if p.is_absolute() {
                p
            } else {
                git_dir.join(p)
            }
        }
        _ => git_dir.to_path_buf(),
    }
}

fn parse_origin_url(config: &str) -> Option<String> {
    // Minimal git-config parser: scan for `[remote "origin"]` section and
    // return the first `url = ...` value within it. Good enough for the
    // 99% case of origin pointing at a single URL.
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == "[remote \"origin\"]";
            continue;
        }
        if in_origin {
            if let Some(rest) = trimmed.strip_prefix("url") {
                let val = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn normalize_remote_url(url: &str) -> String {
    let stripped = url.trim_end_matches(".git");
    // `git@host:path` (SCP-like) -> `https://host/path`.
    if let Some(rest) = stripped.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            return format!("https://{}/{}", host, path);
        }
    }
    // `ssh://[user@]host[:port]/path` -> `https://host/path`. Strips optional
    // user prefix (`git@`, `alice@`, etc.) and optional port so the resulting
    // URL is web-navigable regardless of the underlying git transport config.
    if let Some(rest) = stripped.strip_prefix("ssh://") {
        let host_path = rest.split_once('@').map_or(rest, |(_, r)| r);
        let normalized = match host_path.split_once('/') {
            Some((host_port, path)) => {
                let host = host_port.split(':').next().unwrap_or(host_port);
                format!("{}/{}", host, path)
            }
            None => host_path.split(':').next().unwrap_or(host_path).to_string(),
        };
        return format!("https://{}", normalized);
    }
    // Already HTTPS (possibly with `.git` suffix, already trimmed).
    stripped.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_attached_branches() {
        assert_eq!(parse_head("ref: refs/heads/main\n"), Some("main".into()));
        assert_eq!(
            parse_head("ref: refs/heads/feature/git-branch\n"),
            Some("feature/git-branch".into())
        );
        // No trailing newline (some git versions omit it).
        assert_eq!(parse_head("ref: refs/heads/main"), Some("main".into()));
    }

    #[test]
    fn head_detached_returns_none() {
        // Raw SHA == detached HEAD; bash `--show-current` prints nothing.
        assert_eq!(
            parse_head("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\n"),
            None
        );
    }

    #[test]
    fn head_non_branch_ref_returns_none() {
        // Tag / remote checkout: not a local branch, mirror bash behavior.
        assert_eq!(parse_head("ref: refs/tags/v1.0\n"), None);
        assert_eq!(parse_head("ref: refs/remotes/origin/main\n"), None);
    }

    #[test]
    fn head_empty_returns_none() {
        assert_eq!(parse_head(""), None);
        assert_eq!(parse_head("\n"), None);
    }

    #[test]
    fn remote_url_normalizes_scp_form() {
        assert_eq!(
            normalize_remote_url("git@github.com:shallow/horologium.git"),
            "https://github.com/shallow/horologium"
        );
    }

    #[test]
    fn remote_url_normalizes_ssh_form() {
        assert_eq!(
            normalize_remote_url("ssh://git@github.com/shallow/horologium.git"),
            "https://github.com/shallow/horologium"
        );
    }

    #[test]
    fn remote_url_ssh_non_git_user() {
        // ssh with arbitrary username, not git@
        assert_eq!(
            normalize_remote_url("ssh://alice@gitea.example.com/team/repo.git"),
            "https://gitea.example.com/team/repo"
        );
    }

    #[test]
    fn remote_url_ssh_no_user() {
        assert_eq!(
            normalize_remote_url("ssh://gitea.example.com/team/repo.git"),
            "https://gitea.example.com/team/repo"
        );
    }

    #[test]
    fn remote_url_ssh_with_port_is_dropped() {
        // Web URLs don't use the SSH port; strip the :port segment.
        assert_eq!(
            normalize_remote_url("ssh://git@github.com:22/org/repo.git"),
            "https://github.com/org/repo"
        );
        assert_eq!(
            normalize_remote_url("ssh://host:2222/team/repo.git"),
            "https://host/team/repo"
        );
    }

    #[test]
    fn remote_url_passes_through_https() {
        assert_eq!(
            normalize_remote_url("https://github.com/shallow/horologium.git"),
            "https://github.com/shallow/horologium"
        );
        assert_eq!(
            normalize_remote_url("https://github.com/shallow/horologium"),
            "https://github.com/shallow/horologium"
        );
    }

    #[test]
    fn origin_config_parse_picks_origin() {
        let config = r#"
[core]
	repositoryformatversion = 0
[remote "upstream"]
	url = https://github.com/other/repo.git
[remote "origin"]
	url = git@github.com:shallow/horologium.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
	remote = origin
"#;
        assert_eq!(
            parse_origin_url(config).as_deref(),
            Some("git@github.com:shallow/horologium.git")
        );
    }

    #[test]
    fn origin_config_parse_no_origin() {
        let config = "[core]\n\trepositoryformatversion = 0\n";
        assert_eq!(parse_origin_url(config), None);
    }

    #[test]
    fn commondir_relative_resolves_against_gitdir() {
        let git_dir = Path::new("/tmp/main-repo/.git/worktrees/feat");
        let resolved = resolve_common_dir_from_content(git_dir, "../..\n");
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/main-repo/.git/worktrees/feat/../..")
        );
    }

    #[test]
    fn commondir_absolute_is_used_as_is() {
        let git_dir = Path::new("/anywhere");
        let resolved = resolve_common_dir_from_content(git_dir, "/tmp/main-repo/.git\n");
        assert_eq!(resolved, PathBuf::from("/tmp/main-repo/.git"));
    }

    #[test]
    fn commondir_empty_falls_back_to_gitdir() {
        let git_dir = Path::new("/tmp/x/.git");
        assert_eq!(
            resolve_common_dir_from_content(git_dir, ""),
            PathBuf::from("/tmp/x/.git")
        );
        assert_eq!(
            resolve_common_dir_from_content(git_dir, "\n"),
            PathBuf::from("/tmp/x/.git")
        );
    }

    // --- IO tests (use real tempdirs to exercise find_git_dir + origin_web_url) ---

    use tempfile::TempDir;

    fn write_repo(root: &Path, head_ref: &str, config: Option<&str>) {
        let git = root.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), format!("ref: refs/heads/{}\n", head_ref)).unwrap();
        if let Some(cfg) = config {
            std::fs::write(git.join("config"), cfg).unwrap();
        }
    }

    #[test]
    fn find_git_dir_at_repo_root() {
        let td = TempDir::new().unwrap();
        write_repo(td.path(), "main", None);
        let found = find_git_dir(td.path()).unwrap();
        assert_eq!(found, td.path().join(".git"));
    }

    #[test]
    fn find_git_dir_searches_upward_from_subdirectory() {
        let td = TempDir::new().unwrap();
        write_repo(td.path(), "main", None);
        let nested = td.path().join("a/b/c/d");
        std::fs::create_dir_all(&nested).unwrap();
        let found = find_git_dir(&nested).unwrap();
        assert_eq!(found, td.path().join(".git"));
    }

    #[test]
    fn find_git_dir_handles_worktree_absolute_pointer() {
        let td = TempDir::new().unwrap();
        let real_gitdir = td.path().join("main/.git/worktrees/feat");
        std::fs::create_dir_all(&real_gitdir).unwrap();
        let wt = td.path().join("linked");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", real_gitdir.display()),
        )
        .unwrap();
        let found = find_git_dir(&wt).unwrap();
        assert_eq!(found, real_gitdir);
    }

    #[test]
    fn find_git_dir_handles_worktree_relative_pointer() {
        let td = TempDir::new().unwrap();
        let real_gitdir = td.path().join("main/.git/worktrees/feat");
        std::fs::create_dir_all(&real_gitdir).unwrap();
        let wt = td.path().join("linked");
        std::fs::create_dir_all(&wt).unwrap();
        // Relative gitdir from the worktree dir.
        std::fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/feat\n").unwrap();
        let found = find_git_dir(&wt).unwrap();
        assert_eq!(found, wt.join("../main/.git/worktrees/feat"));
    }

    #[test]
    fn origin_web_url_end_to_end_regular_repo() {
        let td = TempDir::new().unwrap();
        write_repo(
            td.path(),
            "main",
            Some("[remote \"origin\"]\n\turl = git@github.com:foo/bar.git\n"),
        );
        let url = origin_web_url(td.path()).unwrap();
        assert_eq!(url, "https://github.com/foo/bar");
    }

    #[test]
    fn origin_web_url_via_worktree_commondir() {
        let td = TempDir::new().unwrap();
        // Main repo with origin in config.
        let main = td.path().join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/feat")).unwrap();
        std::fs::write(
            main.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/x/y.git\n",
        )
        .unwrap();
        // Per-worktree gitdir: HEAD + commondir pointing back to main .git.
        let wt_gitdir = main.join(".git/worktrees/feat");
        std::fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
        // Worktree directory: .git is a file pointing at the per-worktree gitdir.
        let wt = td.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .unwrap();
        // Verifies the full pipeline: find_git_dir → resolve_common_dir → config.
        let url = origin_web_url(&wt).unwrap();
        assert_eq!(url, "https://github.com/x/y");
    }

    #[test]
    fn find_git_dir_returns_none_when_no_git_ancestor() {
        // Verifies the `cur.parent()?` termination path: when no .git exists
        // anywhere on the upward walk, the loop bails at the filesystem root
        // instead of spinning. Assumes /tmp (and its ancestors) aren't a git
        // repo, which is true on standard Linux / macOS / WSL layouts.
        let td = TempDir::new().unwrap();
        let deep = td.path().join("a/b/c/d/e");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(
            find_git_dir(&deep).is_none(),
            "should return None when no .git exists on the path to root"
        );
    }

    #[test]
    fn current_branch_via_worktree_reads_per_worktree_head() {
        // Worktree HEAD is in the per-worktree gitdir, not the common one.
        let td = TempDir::new().unwrap();
        let main = td.path().join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/feat")).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let wt_gitdir = main.join(".git/worktrees/feat");
        std::fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
        let wt = td.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .unwrap();
        // Should pick up the per-worktree branch, not "main".
        assert_eq!(current_branch(&wt).as_deref(), Some("feat"));
    }
}
