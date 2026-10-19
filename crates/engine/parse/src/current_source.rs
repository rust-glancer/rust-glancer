//! Exact request text together with the derived source data shared by editor queries.
//!
//! The selected crate supplies one Rust edition. Body analysis, hover, inlay hints, and completion
//! read the same request-owned text, line index, and ordinary parse.

use std::sync::Arc;

use rg_ir_model::Span;
use rg_source::SourceRevision;
use rg_syntax::{Parse as SyntaxParse, SourceFile};
use rg_text::RustEdition;

use crate::{LineIndex, parse_source_file};

/// Source data derived from one immutable editor document snapshot.
///
/// This value has no project identity and is never retained in the parse database. A caller must
/// still pair it with a saved `(package, file)` interpretation before borrowing global semantics.
#[derive(Debug, Clone)]
pub struct CurrentSource {
    text: Arc<str>,
    revision: SourceRevision,
    line_index: LineIndex,
    syntax: SyntaxParse<SourceFile>,
}

impl CurrentSource {
    /// Parse the source using the selected crate's edition.
    pub fn new(text: impl Into<Arc<str>>, edition: RustEdition) -> Self {
        let text = text.into();
        let revision = SourceRevision::from_bytes(text.as_bytes());
        let line_index = LineIndex::new(&text);
        let syntax = parse_source_file(&text, edition);

        Self {
            text,
            revision,
            line_index,
            syntax,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn revision(&self) -> SourceRevision {
        self.revision
    }

    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    pub fn parse(&self) -> &SyntaxParse<SourceFile> {
        &self.syntax
    }

    /// Read a span known to belong to this exact source.
    pub fn text_for_span(&self, span: Span) -> Option<&str> {
        let start = usize::try_from(span.start).ok()?;
        let end = usize::try_from(span.end).ok()?;
        self.text.get(start..end)
    }

    /// Convert an offset known to belong to this exact source into its line number.
    pub fn line_for_offset(&self, offset: u32) -> Option<u32> {
        (usize::try_from(offset).ok()? <= self.text.len())
            .then(|| self.line_index.position(offset).line)
    }
}
