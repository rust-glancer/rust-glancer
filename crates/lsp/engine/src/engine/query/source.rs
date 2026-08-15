//! Maps LSP paths and UTF-16 positions onto project file/crate contexts.
//!
//! One physical Rust file is not necessarily one semantic query location. A shared module may be
//! reachable from several crate roots, and a workspace path can have package-local file identities.
//! These helpers expand that fan-out first, then convert the editor's UTF-16 position with the line
//! index belonging to each file context.
//!
//! For example, `src/shared.rs` included by both a library and a binary produces two crate/offset
//! entries for one editor cursor. Analysis may answer differently under each crate's cfg and scope.

use std::path::Path;

use anyhow::Context as _;
use rg_ir_model::CrateRef;
use rg_project::{FileContext, ProjectSnapshot};

use super::QueryRunner;
use crate::proto::position;

impl QueryRunner<'_> {
    /// Expand one LSP cursor into every `(file context, crate, byte offset)` interpretation.
    ///
    /// Invalid UTF-16 positions are skipped per context instead of failing the whole request. This
    /// lets another valid owner of the same path still answer the query.
    pub(super) fn crate_offsets(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<(FileContext, CrateRef, u32)>> {
        let mut crates = Vec::new();

        let contexts =
            Self::file_contexts(snapshot, path).context("resolve query file contexts")?;
        for context in contexts {
            let Some(offset) = Self::offset_for_context(snapshot, &context, position)
                .context("convert query position")?
            else {
                tracing::trace!(
                    path = %path.display(),
                    line = position.line,
                    character = position.character,
                    package = ?context.package,
                    file = ?context.file,
                    "could not convert LSP position to file offset"
                );
                continue;
            };

            for crate_ref in &context.crates {
                crates.push((context.clone(), *crate_ref, offset));
            }
        }

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            crate_offset_count = crates.len(),
            "resolved request crate offsets"
        );

        Ok(crates)
    }

    /// Find every package-local project identity for an already-selected source path.
    pub(super) fn file_contexts(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
    ) -> anyhow::Result<Vec<FileContext>> {
        let contexts = snapshot
            .file_contexts_for_source_path(path)
            .context("resolve project file contexts")?;
        let target_count = contexts
            .iter()
            .map(|context| context.crates.len())
            .sum::<usize>();
        tracing::trace!(
            path = %path.display(),
            context_count = contexts.len(),
            target_count,
            "resolved file contexts"
        );

        Ok(contexts)
    }

    /// Convert one editor position with the line index owned by this file context.
    fn offset_for_context(
        snapshot: ProjectSnapshot<'_>,
        context: &FileContext,
        position: ls_types::Position,
    ) -> anyhow::Result<Option<u32>> {
        let Some(line_index) = snapshot
            .file_line_index(context.package, context.file)
            .context("load position line index")?
        else {
            return Ok(None);
        };
        let offset = line_index.offset_from_utf16_position(position::parse_position(position));
        tracing::trace!(
            package = ?context.package,
            file = ?context.file,
            line = position.line,
            character = position.character,
            offset = ?offset,
            "converted LSP position to file offset"
        );
        Ok(offset)
    }
}
