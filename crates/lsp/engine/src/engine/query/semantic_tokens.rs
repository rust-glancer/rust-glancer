use anyhow::Context as _;
use rg_ir_model::TextSpan;
use rg_lsp_proto::EditorDocumentSnapshot;
use rg_project::DocumentSourceView;
use rg_std::UniqueVec;

use super::{QueryCancellation, QueryRunError, QueryRunner};
use crate::proto::{position, semantic_tokens::SemanticTokensEncoder};

impl QueryRunner<'_> {
    /// Documentation needs captured syntax and saved declaration identities. In particular, a
    /// whole-file token request must not select every function body for materialization.
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
        let targets = Self::file_contexts(snapshot, document.source_path())
            .context("resolve highlighted document contexts")?
            .into_iter()
            .flat_map(|context| {
                context
                    .crates
                    .into_iter()
                    .map(move |crate_ref| (crate_ref, context.file))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(gen_lsp_types::SemanticTokens::default());
        }
        let source = snapshot
            .prepare_document_source(&targets, document.text(), &cancellation.token())
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
            Some(TextSpan { start, end })
        } else {
            None
        };
        let crates = targets
            .iter()
            .map(|&(crate_ref, _)| crate_ref)
            .collect::<UniqueVec<_>>();
        let mut analysis = snapshot
            .analysis_for_crates(crates.as_slice(), cancellation.token())
            .context("load documentation declaration context")?;
        if let DocumentSourceView::Current(source) = source {
            analysis = analysis.with_current_source(source);
        }
        let mut highlights = Vec::new();
        for (crate_ref, file) in targets {
            rg_std::check_cancel!(cancellation, "documentation crate highlights");
            highlights.extend(
                analysis
                    .documentation_highlights(crate_ref, file, range)
                    .context("highlight source documentation")?,
            );
        }
        rg_std::check_cancel!(cancellation, "encode documentation highlights");
        Ok(SemanticTokensEncoder::new(document.text(), &line_index).encode(highlights))
    }
}
