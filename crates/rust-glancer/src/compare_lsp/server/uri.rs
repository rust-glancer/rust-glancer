//! File URI construction shared by initialization, didOpen, and query requests.
//!
//! LSP file URIs need canonical paths and platform-correct escaping. Use `ls_types` here instead
//! of hand-building strings so the rest of the harness can pass typed URIs around.

use std::path::Path;

use anyhow::Context as _;

/// Convert a local path into the `file://` URI shape expected by LSP payloads.
pub(crate) fn file_uri(path: &Path) -> anyhow::Result<ls_types::Uri> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Canonicalizing path {} for LSP URI failed", path.display()))?;
    ls_types::Uri::from_file_path(&path).with_context(|| {
        format!(
            "Converting path {} to an LSP file URI failed",
            path.display()
        )
    })
}
