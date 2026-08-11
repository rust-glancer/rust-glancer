//! Completion assembly for source positions.
//!
//! Completion has two inputs. Indexed source views supply semantic scope and identity for shapes
//! such as `user.na$0`, `Widget::ne$0`, `let value = inp$0`, and `User { na$0 }`.
//! A speculative parse of the request buffer recovers source that is intentionally incomplete,
//! such as `fn re$0` in a trait impl, `mod pars$0`, `#[derive(Cl$0)]`, or
//! `format!("{na$0}")`.
//!
//! `CompletionResolver` chooses exactly one primary syntax/semantic family, then adds the small
//! overlays valid at that site. The `resolvers` module owns those result-producing flows;
//! sibling modules provide site detection, syntax classification, candidate lookup, rendering,
//! and source-edit policy. Examples use `$0` to mark the cursor.

mod candidates;
mod import_edit;
mod pattern;
mod render;
mod resolvers;
mod site;
mod syntax;

use std::fmt;

use rg_ir_model::CrateRef;
use rg_parse::FileId;

pub(crate) use resolvers::CompletionResolver;

/// One parsed and classified editor buffer shared across semantic crate interpretations.
///
/// A source path can belong to several crates. Those interpretations need different semantic
/// lookup, but the incomplete text at the cursor is identical. Preparing this value once lets
/// every `CompletionQuery` reuse the same speculative tree and normalized syntax domain.
pub struct CompletionSource<'source> {
    offset: u32,
    syntax: syntax::CompletionSyntaxContext<'source>,
}

impl fmt::Debug for CompletionSource<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletionSource")
            .field("offset", &self.offset)
            .field("source_len", &self.source_text().len())
            .finish_non_exhaustive()
    }
}

impl<'source> CompletionSource<'source> {
    /// Parses `source_text` once after replacing the partial cursor prefix with a stable marker.
    pub fn new(source_text: &'source str, offset: u32) -> Option<Self> {
        Some(Self {
            offset,
            syntax: syntax::CompletionSyntaxContext::at(Some(source_text), offset)?,
        })
    }

    /// Returns the UTF-8 cursor offset this syntax analysis describes.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Returns the exact editor buffer used to build the speculative tree.
    pub fn source_text(&self) -> &'source str {
        self.syntax.source()
    }
}

/// Editor capabilities that affect how completion items should be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompletionClientCapabilities {
    /// Whether insertion text may contain LSP snippet placeholders.
    pub snippet_support: bool,
}

impl CompletionClientCapabilities {
    pub fn with_snippet_support(mut self, snippet_support: bool) -> Self {
        self.snippet_support = snippet_support;
        self
    }
}

/// One source-position completion query plus the editor state needed for request-local syntax.
///
/// Semantic completion can still use indexed sites without `source_text`. Supplying the exact
/// request buffer additionally enables incomplete syntax classification, accurate replacement
/// spans, call/punctuation detection, and source edits such as auto-imports.
#[derive(Debug, Clone, Copy)]
pub struct CompletionQuery<'a> {
    /// Crate context used to interpret paths and visibility from `file_id`.
    pub crate_ref: CrateRef,
    /// Source file containing the completion position.
    pub file_id: FileId,
    /// UTF-8 byte offset of the cursor in `source_text` and the matching analysis snapshot.
    pub offset: u32,
    /// Exact editor buffer for speculative parsing and source-aware insertion policy.
    pub source_text: Option<&'a str>,
    completion_source: Option<&'a CompletionSource<'a>>,
    /// Client features that affect insertion text but not semantic eligibility.
    pub client_capabilities: CompletionClientCapabilities,
}

impl<'a> CompletionQuery<'a> {
    pub fn new(crate_ref: CrateRef, file_id: FileId, offset: u32) -> Self {
        Self {
            crate_ref,
            file_id,
            offset,
            source_text: None,
            completion_source: None,
            client_capabilities: CompletionClientCapabilities::default(),
        }
    }

    pub fn with_source_text(mut self, source_text: &'a str) -> Self {
        self.source_text = Some(source_text);
        self.completion_source = None;
        self
    }

    /// Reuses syntax prepared once for all semantic interpretations of this source position.
    pub fn with_completion_source(mut self, completion_source: &'a CompletionSource<'a>) -> Self {
        self.source_text = Some(completion_source.source_text());
        self.completion_source = Some(completion_source);
        self
    }

    pub fn with_client_capabilities(
        mut self,
        client_capabilities: CompletionClientCapabilities,
    ) -> Self {
        self.client_capabilities = client_capabilities;
        self
    }

    pub(crate) fn completion_source(self) -> Option<&'a CompletionSource<'a>> {
        self.completion_source
            .filter(|source| source.offset() == self.offset)
    }
}
