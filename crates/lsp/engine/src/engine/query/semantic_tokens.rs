use anyhow::Context as _;
use gen_lsp_types::{Range, SemanticTokens};
use rg_ir_model::Span;
use rg_lsp_proto::DocumentSnapshot;

use super::{QueryCancellation, QueryRunner};
use crate::proto::{position, semantic_tokens::SemanticTokensEncoder};

impl QueryRunner<'_> {
    /// Highlight documentation and format-string placeholders in the editor's captured text.
    ///
    /// Both features need source syntax and declarations, but neither needs function-body
    /// analysis. Format highlighting follows macro expansions without resolving expression types.
    pub(crate) fn semantic_tokens(
        &mut self,
        input: DocumentSnapshot,
        range: Option<Range>,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<SemanticTokens> {
        self.with_editor_snapshot(input.editor(), cancellation, |snapshot| {
            let document = input.document();
            let Some(target) = Self::file_context(snapshot, document.source_path())
                .context("resolve highlighted document context")?
            else {
                return Ok(SemanticTokens::default());
            };
            let line_index = snapshot.sources().line_index(target.file)?;
            let range = if let Some(range) = range {
                let Some(start) =
                    line_index.offset_from_utf16_position(position::parse_position(range.start))
                else {
                    return Ok(SemanticTokens::default());
                };
                let Some(end) =
                    line_index.offset_from_utf16_position(position::parse_position(range.end))
                else {
                    return Ok(SemanticTokens::default());
                };
                if start >= end {
                    return Ok(SemanticTokens::default());
                }
                Some(Span { start, end })
            } else {
                None
            };
            let analysis = snapshot.analysis();
            let mut highlights = analysis
                .documentation_highlights(target.crate_ref, target.file, range)
                .context("highlight source documentation")?;
            highlights.extend(analysis.format_string_highlights(
                target.crate_ref,
                target.file,
                range,
            )?);
            rg_std::check_cancel!(cancellation, "encode documentation highlights");
            Ok(SemanticTokensEncoder::new(document.text(), line_index).encode(highlights))
        })
    }
}
