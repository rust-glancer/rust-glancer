//! Small filesystem helpers for source paths discovered from Rust syntax.

use std::path::{Path, PathBuf};

/// Resolves a string literal path against `base_dir` and keeps only existing files.
pub(crate) fn resolve_path_literal(base_dir: &Path, path_literal: &str) -> Option<PathBuf> {
    let path = Path::new(path_literal);
    if path.as_os_str().is_empty() {
        return None;
    }

    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };
    path.exists().then_some(path)
}

/// Resolves a relative-only string literal path against `base_dir`.
pub(crate) fn resolve_relative_path_literal(
    base_dir: &Path,
    path_literal: &str,
) -> Option<PathBuf> {
    let path = Path::new(path_literal);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return None;
    }

    let path = base_dir.join(path);
    path.exists().then_some(path)
}
