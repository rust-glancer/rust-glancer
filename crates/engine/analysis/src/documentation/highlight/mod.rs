//! Color item links and fenced Rust examples where they are written in doc comments.
//! Links get the kind of their resolved declaration; examples get colors from their Rust syntax.
//! Both passes end by translating Markdown positions back into the requested source file.

mod code_block;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, Span, TextSpan};
use rg_ir_view::item::declaration::DeclarationView;
use rg_parse::syntax_edition;

use self::code_block::CodeBlock;
use super::{MarkdownLink, SourceDocumentationQuery};
use crate::{Analysis, Highlight, HighlightKind};

pub(crate) struct DocumentationHighlighter<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> DocumentationHighlighter<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Collect highlights in the query's Rust file, clipped to `range` when one is supplied.
    ///
    /// Keep whole Markdown documents while parsing: a link inside the requested range may use
    /// a reference definition outside it. The range limits source spans in the result, rather
    /// than the Markdown available to either parser.
    pub(crate) fn highlight(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<Highlight>> {
        let source = SourceDocumentationQuery::new(self.0);
        let Some(syntax) = source
            .syntax(crate_ref, file)
            .context("read highlight syntax")?
        else {
            return Ok(Vec::new());
        };
        let edition = self
            .0
            .view_db()
            .crate_edition(crate_ref)
            .context("read example edition")?;
        let declarations = DeclarationView::new(self.0.view_db());
        let mut highlights = Vec::new();
        // Assemble each document first, including any docs in the module's other file.
        // Documents with no source text in the requested region need no coloring work.
        for docs in source
            .documents(crate_ref, file, &syntax)
            .context("read highlighted documentation")?
        {
            rg_std::check_cancel!(self.0, "documentation highlighting");
            if docs
                .source_ranges(file, 0..docs.text.len())
                .iter()
                .all(|span| Self::clip(*span, range).is_none())
            {
                continue;
            }
            // A link gets its target's color, such as struct or method. Resolve only links
            // touching the requested region, while letting Markdown see every reference definition.
            for link in MarkdownLink::extract(&docs.text) {
                rg_std::check_cancel!(self.0, "documentation link highlighting");
                let spans = docs.source_ranges(file, link.range.clone());
                if spans.iter().all(|span| Self::clip(*span, range).is_none()) {
                    continue;
                }
                let Some(declaration) = source
                    .resolve(&docs, &link)
                    .context("resolve highlighted link")?
                else {
                    continue;
                };
                let Some(kind) = declarations
                    .kind(declaration)
                    .context("read link highlight kind")?
                else {
                    continue;
                };
                highlights.extend(
                    spans
                        .into_iter()
                        .filter_map(|span| Self::clip(span, range))
                        .map(|span| Highlight {
                            span,
                            kind: HighlightKind::Symbol(kind),
                            documentation: true,
                        }),
                );
            }
            // Examples need only their own syntax, so they can be colored even without a
            // resolved doc owner. Their token ranges arrive in Markdown coordinates; map those
            // back to this Rust file before clipping them to the requested region.
            for block in CodeBlock::extract(&docs.text, syntax_edition(edition)) {
                rg_std::check_cancel!(self.0, "documentation example highlighting");
                for (markdown_range, kind) in block
                    .highlights(self.0)
                    .context("highlight example syntax")?
                {
                    highlights.extend(
                        docs.source_ranges(file, markdown_range)
                            .into_iter()
                            .filter_map(|span| Self::clip(span, range))
                            .map(|span| Highlight {
                                span,
                                kind,
                                documentation: true,
                            }),
                    );
                }
            }
        }
        highlights.sort_by_key(|highlight| (highlight.span.text.start, highlight.span.text.end));
        highlights.dedup();
        Ok(highlights)
    }

    fn clip(span: Span, range: Option<TextSpan>) -> Option<Span> {
        let text = if let Some(range) = range {
            TextSpan {
                start: span.text.start.max(range.start),
                end: span.text.end.min(range.end),
            }
        } else {
            span.text
        };
        (!text.is_empty()).then_some(Span { text })
    }
}
