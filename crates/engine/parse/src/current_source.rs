//! Exact request text together with the derived source data shared by editor queries.
//!
//! A document can be interpreted in several crates, sometimes with different Rust editions. The
//! text itself does not change between those interpretations. `CurrentSource` builds its line
//! index once and keeps one ordinary parse for each required edition, so body analysis, hover,
//! inlay hints, and completion all read the same request-owned source.

use std::{collections::HashMap, sync::Arc};

use rg_source::SourceRevision;
use rg_syntax::{Parse as SyntaxParse, SourceFile};
use rg_text::RustEdition;

use crate::{LineIndex, Span, parse_source_file};

/// Source data derived from one immutable editor document snapshot.
///
/// This value has no project identity and is never retained in the parse database. A caller must
/// still pair it with a saved `(package, file)` interpretation before borrowing global semantics.
#[derive(Debug, Clone)]
pub struct CurrentSource {
    text: Arc<str>,
    revision: SourceRevision,
    line_index: LineIndex,
    syntax_by_edition: HashMap<RustEdition, SyntaxParse<SourceFile>>,
}

impl CurrentSource {
    /// Parse the source once for every edition needed by this request.
    pub fn new(text: impl Into<Arc<str>>, editions: impl IntoIterator<Item = RustEdition>) -> Self {
        let text = text.into();
        let revision = SourceRevision::from_bytes(text.as_bytes());
        let line_index = LineIndex::new(&text);
        let mut syntax_by_edition = HashMap::new();
        for edition in editions {
            syntax_by_edition
                .entry(edition)
                .or_insert_with(|| parse_source_file(&text, edition));
        }

        Self {
            text,
            revision,
            line_index,
            syntax_by_edition,
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

    /// Return the ordinary parse for one semantic interpretation of this source.
    pub fn parse(&self, edition: RustEdition) -> Option<&SyntaxParse<SourceFile>> {
        self.syntax_by_edition.get(&edition)
    }

    /// Read a span known to belong to this exact source.
    pub fn text_for_span(&self, span: Span) -> Option<&str> {
        let start = usize::try_from(span.text.start).ok()?;
        let end = usize::try_from(span.text.end).ok()?;
        self.text.get(start..end)
    }

    /// Convert an offset known to belong to this exact source into its line number.
    pub fn line_for_offset(&self, offset: u32) -> Option<u32> {
        (usize::try_from(offset).ok()? <= self.text.len())
            .then(|| self.line_index.position(offset).line)
    }
}
