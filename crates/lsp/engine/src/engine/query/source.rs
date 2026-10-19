//! Selects one saved crate/file context and converts editor positions in that file.

use std::path::Path;

use anyhow::Context as _;
use rg_project::{FileContext, ProjectSnapshot};
use rg_text::RustEdition;

use super::QueryRunner;
use crate::proto::position;

impl QueryRunner<'_> {
    /// Resolve the cursor only in the selected owner. Invalid positions have no query result.
    pub(super) fn file_offset(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
        position: gen_lsp_types::Position,
    ) -> anyhow::Result<Option<(FileContext, u32)>> {
        let Some(context) = Self::file_context(snapshot, path)? else {
            return Ok(None);
        };
        let Some(line_index) = snapshot.file_line_index(context.crate_ref.package, context.file)?
        else {
            return Ok(None);
        };
        Ok(line_index
            .offset_from_utf16_position(position::parse_position(position))
            .map(|offset| (context, offset)))
    }

    pub(super) fn file_context(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
    ) -> anyhow::Result<Option<FileContext>> {
        let context = snapshot
            .file_context_for_source_path(path)
            .context("resolve project file context")?;
        tracing::trace!(path = %path.display(), ?context, "selected file context");
        Ok(context)
    }

    /// Standalone Rust files have no owner and use the newest supported edition for syntax queries.
    pub(super) fn rust_edition_for_path(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
    ) -> anyhow::Result<RustEdition> {
        Ok(Self::file_context(snapshot, path)?
            .and_then(|context| snapshot.package_edition(context.crate_ref.package))
            .unwrap_or(RustEdition::Edition2024))
    }
}
