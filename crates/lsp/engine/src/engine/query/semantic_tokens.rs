use anyhow::Context as _;
use rg_ir_model::Span;
use rg_lsp_proto::EditorDocumentSnapshot;
use rg_project::DocumentSourceView;

use super::{QueryCancellation, QueryRunError, QueryRunner};
use crate::proto::{position, semantic_tokens::SemanticTokensEncoder};

impl QueryRunner<'_> {
    /// Highlight documentation and format-string placeholders in the editor's captured text.
    ///
    /// Both features need source syntax and saved declarations, but neither needs function-body
    /// analysis. Format highlighting follows macro expansions without resolving expression types.
    pub(crate) fn semantic_tokens(
        &mut self,
        document: EditorDocumentSnapshot,
        range: Option<gen_lsp_types::Range>,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<gen_lsp_types::SemanticTokens, QueryRunError> {
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for highlighting")?;
        let Some(target) = Self::file_context(snapshot, document.source_path())
            .context("resolve highlighted document context")?
        else {
            return Ok(gen_lsp_types::SemanticTokens::default());
        };
        // Use saved source when it matches the editor; otherwise prepare the edited syntax once
        // and share it between documentation and format highlighting.
        let source = snapshot
            .prepare_document_source(
                (target.crate_ref, target.file),
                document.text(),
                &cancellation.token(),
            )
            .context("prepare highlighted document source")?;
        let line_index = source.line_index().clone();
        let range = if let Some(range) = range {
            let Some(start) =
                line_index.offset_from_utf16_position(position::parse_position(range.start))
            else {
                return Ok(gen_lsp_types::SemanticTokens::default());
            };
            let Some(end) =
                line_index.offset_from_utf16_position(position::parse_position(range.end))
            else {
                return Ok(gen_lsp_types::SemanticTokens::default());
            };
            if start >= end {
                return Ok(gen_lsp_types::SemanticTokens::default());
            }
            Some(Span { start, end })
        } else {
            None
        };
        let mut analysis = snapshot
            .analysis_for_crates(&[target.crate_ref], cancellation.token())
            .context("load documentation declaration context")?;
        if let DocumentSourceView::Current(source) = source {
            analysis = analysis.with_current_source(source);
        }
        let mut highlights = analysis
            .documentation_highlights(target.crate_ref, target.file, range)
            .context("highlight source documentation")?;
        highlights.extend(analysis.format_string_highlights(
            target.crate_ref,
            target.file,
            range,
        )?);
        rg_std::check_cancel!(cancellation, "encode documentation highlights");
        Ok(SemanticTokensEncoder::new(document.text(), &line_index).encode(highlights))
    }
}
