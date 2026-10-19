//! Highlight captures and formatting syntax in macro arguments.
//!
//! Macro expansion identifies which original literals are format templates, including those
//! forwarded through wrappers. Split each template into names and formatting punctuation;
//! ordinary string text and escapes keep their syntax highlighting.

use rg_ir_model::{CrateRef, FileId, Span};
use rg_ir_view::source::SourceOccurrenceView;
use rg_syntax::{
    AstNode as _, AstToken as _,
    ast::{self, FormatStringPartKind},
};

use crate::{Analysis, Highlight, HighlightKind, SymbolKind};

pub(crate) struct FormatStringHighlighter<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> FormatStringHighlighter<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Color placeholder parts in the request's source, which may include unsaved edits.
    pub(crate) fn highlight(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        range: Option<Span>,
    ) -> anyhow::Result<Vec<Highlight>> {
        let Some(syntax) = self.0.source_syntax(crate_ref, file)? else {
            return Ok(Vec::new());
        };
        let Some(package) = self.0.saved_source.parse.package(crate_ref.package.0) else {
            return Ok(Vec::new());
        };
        let mut highlights = Vec::new();
        for span in SourceOccurrenceView::new(self.0.view_db())
            .format_string_spans(package, crate_ref, file, &syntax, range)?
        {
            rg_std::check_cancel!(self.0, "format string highlighting");
            if span.intersection(range.unwrap_or(span)).is_none() {
                continue;
            }
            // Expansion returned a span in the original file. Read that token from the syntax
            // we already have, so `"\n{user}"` keeps the written escape's two-byte spelling.
            let Some(token) = syntax
                .syntax()
                .token_at_offset(span.start.into())
                .right_biased()
            else {
                continue;
            };
            if Span::from_text_range(token.text_range()) != span {
                continue;
            }
            let Some(string) = ast::String::cast(token) else {
                continue;
            };
            // For `"{user:>width$}"`, color `user` and `width` as variables and the gaps as
            // formatting syntax. Knowing their names and positions is enough; no types are needed.
            string.format_parts(|part| {
                let kind = match part.kind {
                    FormatStringPartKind::Name(_) => HighlightKind::Symbol(SymbolKind::Variable),
                    FormatStringPartKind::Specifier => HighlightKind::FormatSpecifier,
                };
                let part_span = Span {
                    start: span.start + u32::from(part.range.start()),
                    end: span.start + u32::from(part.range.end()),
                };
                if let Some(span) = part_span.intersection(range.unwrap_or(part_span)) {
                    highlights.push(Highlight {
                        span,
                        kind,
                        documentation: false,
                    });
                }
            });
        }
        Ok(highlights)
    }
}
