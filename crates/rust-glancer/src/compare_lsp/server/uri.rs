//! File URI construction shared by initialization, didOpen, and query requests.
//!
//! LSP file URIs need canonical paths and platform-correct escaping. Keep that conversion on the
//! shared protocol boundary so Windows verbatim paths never leak into editor-facing payloads.

use std::path::Path;

use anyhow::Context as _;

/// Convert a local path into the `file://` URI shape expected by LSP payloads.
pub(crate) fn file_uri(path: &Path) -> anyhow::Result<ls_types::Uri> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Canonicalizing path {} for LSP URI failed", path.display()))?;
    rg_lsp_proto::path_to_file_uri(&path).with_context(|| {
        format!(
            "Converting path {} to an LSP file URI failed",
            path.display()
        )
    })
}
