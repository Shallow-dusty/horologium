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
}
