use std::cmp::Reverse;

use rg_ir_model::{ExprId, Span};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Provenance of source tokens copied into a body's macro expansions.
///
/// Lowering puts original argument offsets directly in the IR's name and field spans. This map
/// distinguishes those precise locations from generated nodes anchored to the whole invocation.
/// For `duplicate!(value)` expanding to `(value, value)`, both paths keep the span of the single
/// written `value`. This map records that span once; the paths retain their separate IR identities.
/// It also remembers that the input was duplicated, so editing one occurrence cannot accidentally
/// change another use whose meaning was not lowered.
/// Keeping the provenance separate means the expanded syntax trees can be dropped after lowering.
#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceMap {
    macro_arguments: Vec<Span>,
    duplicated_macro_arguments: Vec<Span>,
    format_captures: Vec<FormatCaptureSource>,
}

impl BodySourceMap {
    /// Record one expansion at a time. Forwarding a token through successive expansions is fine;
    /// copying it twice within one expansion means an edit would affect several output locations.
    pub(crate) fn record_expansion_arguments(&mut self, spans: impl Iterator<Item = Span>) {
        let start = self.macro_arguments.len();
        self.macro_arguments.extend(spans);
        let arguments = &mut self.macro_arguments[start..];
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

    pub(crate) fn finish(&mut self) {
        // Nested parses can see `>>` as one token or two closing angle brackets. Keep the
        // enclosing range when copied ranges nest, so retained ends increase with their starts.
        for spans in [
            &mut self.macro_arguments,
            &mut self.duplicated_macro_arguments,
        ] {
            spans.sort_unstable_by_key(|span| (span.start, Reverse(span.end)));
            spans.dedup_by(|later, earlier| earlier.contains_span(*later));
            spans.shrink_to_fit();
        }
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

    pub fn is_macro_argument(&self, span: Span) -> bool {
        Self::contains_token_span(&self.macro_arguments, span)
    }

    /// Whether an expansion copied the containing input token more than once. This is conservative:
    /// a later expansion or cfg filter may discard one copy, but we do not prove that here.
    pub fn is_duplicated_macro_argument(&self, span: Span) -> bool {
        Self::contains_token_span(&self.duplicated_macro_arguments, span)
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
