use std::cmp::Reverse;

use rg_ir_model::{ExprId, Span};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Extra source facts collected while lowering a body.
///
/// The occurrence table records where each name came from. This map records which input tokens
/// were used more than once, even if a copy produced no lowered occurrence. It also records
/// format captures, which follow their own spelling rules during rename. Queries can read both
/// facts after the expansion syntax has been dropped.
#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceMap {
    duplicated_macro_arguments: Vec<Span>,
    format_captures: Vec<FormatCaptureSource>,
    generated_owner: bool,
}

impl BodySourceMap {
    /// Record one expansion at a time. In `outer!(value)` expanding to `inner!(value)`, the
    /// same token passes through both expansions. Two copies within one expansion mean that
    /// editing the input would affect several output locations.
    pub(crate) fn record_expansion_arguments(&mut self, spans: impl Iterator<Item = Span>) {
        let mut arguments: Vec<_> = spans.collect();
        arguments.sort_unstable_by_key(|span| (span.start, span.end));
        self.duplicated_macro_arguments.extend(
            arguments
                .windows(2)
                .filter(|pair| pair[0] == pair[1])
                .map(|pair| pair[0]),
        );
    }

    pub(crate) fn record_format_capture(&mut self, expr: ExprId, span: Span) {
        self.format_captures
            .push(FormatCaptureSource { expr, span });
    }

    pub(crate) fn finish(&mut self, generated_owner: bool) {
        self.generated_owner = generated_owner;
        // Nested parses can see `>>` as one token or two closing angle brackets. Keep the
        // enclosing range when copied ranges nest, so retained ends increase with their starts.
        self.duplicated_macro_arguments
            .sort_unstable_by_key(|span| (span.start, Reverse(span.end)));
        self.duplicated_macro_arguments
            .dedup_by(|later, earlier| earlier.contains_span(*later));
        self.duplicated_macro_arguments.shrink_to_fit();
        // Expression scans check individual captures. Sort once so each lookup does not walk
        // every placeholder in the body.
        self.format_captures
            .sort_unstable_by_key(|capture| capture.expr.0);
        self.format_captures.shrink_to_fit();
    }

    /// Whether lowering proved this capture's exact range in a written template.
    pub fn is_format_capture(&self, expr: ExprId, span: Span) -> bool {
        self.format_captures
            .binary_search_by_key(&expr.0, |capture| capture.expr.0)
            .ok()
            .is_some_and(|index| self.format_captures[index].span == span)
    }

    /// Whether the containing input token was copied more than once.
    ///
    /// If `outer!(name)` forwards `name` to two item macros, each generated function may
    /// see just one copy. Return `None` when the generated body cannot establish whether
    /// there were other copies.
    pub fn macro_argument_duplication(&self, span: Span) -> Option<bool> {
        if Self::contains_token_span(&self.duplicated_macro_arguments, span) {
            Some(true)
        } else {
            (!self.generated_owner).then_some(false)
        }
    }

    fn contains_token_span(tokens: &[Span], span: Span) -> bool {
        // A name inside a string literal shares the token's provenance. Because retained ends
        // increase, the last range starting before this name reaches as far as any earlier range.
        let end = tokens.partition_point(|token| token.start <= span.start);
        tokens[..end]
            .last()
            .is_some_and(|token| token.contains_span(span))
    }
}

/// An implicit capture expression with its proven range inside a written literal.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct FormatCaptureSource {
    expr: ExprId,
    span: Span,
}
