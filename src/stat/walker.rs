//! Discover JSONL files under the Claude Code projects root.

#![allow(dead_code)] // TODO: remove once aggregate.rs consumes these

use std::path::{Path, PathBuf};

/// Recursively collect every file ending in `.jsonl` under `root`.
/// Returns an empty vec if `root` doesn't exist.
pub fn find_jsonl(_root: &Path) -> Vec<PathBuf> {
    // Implemented in follow-up commit.
    Vec::new()
}
