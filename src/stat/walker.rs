//! Discover JSONL files under the Claude Code projects root.

use std::path::{Path, PathBuf};

/// Recursively collect every file ending in `.jsonl` under `root`.
/// Returns an empty vec if `root` doesn't exist or isn't readable —
/// callers treat "no projects yet" as a valid empty corpus.
///
/// Symlinks are **not** followed: `DirEntry::file_type()` returns the
/// un-dereferenced type, so a symlink to a directory reports as neither
/// dir nor file and is silently skipped (along with sockets, device
/// nodes, and other special entries). Claude Code's projects root doesn't
/// use symlinks in practice; callers who want symlinked session trees
/// can `--root` directly at the target.
pub fn find_jsonl(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // Unreadable subdir (ENOENT, EACCES) — skip without failing the
            // whole scan. Losing one directory is better than refusing to
            // report on the rest of the corpus.
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                result.push(path);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_jsonl_recursively_across_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::write(root.join("top.jsonl"), "").unwrap();
        fs::write(root.join("a/session1.jsonl"), "").unwrap();
        fs::write(root.join("a/b/session2.jsonl"), "").unwrap();
        fs::write(root.join("a/b/c/deep.jsonl"), "").unwrap();
        fs::write(root.join("a/b/c/not_session.txt"), "").unwrap();

        let mut paths = find_jsonl(root);
        paths.sort();
        assert_eq!(paths.len(), 4, "got: {:?}", paths);
        assert!(paths.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    #[test]
    fn nonexistent_root_returns_empty() {
        let missing = std::env::temp_dir().join("horologium-walker-does-not-exist-xyzzy");
        assert!(find_jsonl(&missing).is_empty());
    }

    #[test]
    fn empty_root_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_jsonl(tmp.path()).is_empty());
    }

    #[test]
    fn non_jsonl_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.json"), "").unwrap();
        fs::write(tmp.path().join("a.jsonl.bak"), "").unwrap();
        fs::write(tmp.path().join("jsonl"), "").unwrap(); // no extension
        assert!(find_jsonl(tmp.path()).is_empty());
    }
}
