use rg_ir_model::{ExprId, SourceId, Span};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Extra source facts collected while lowering a body.
///
/// When a lowered name came from a tracked copy, record the ID of that output token. Its
/// total also includes uses that produced no name occurrence, such as input to an opaque
/// macro. Rename can check coverage after the expansion syntax has been dropped.
#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceMap {
    argument_uses: Vec<MacroArgumentUse>,
    format_captures: Vec<FormatCaptureSource>,
}

impl BodySourceMap {
    pub(crate) fn record_argument_use(&mut self, source: SourceId, copy: u32) {
        self.argument_uses.push(MacroArgumentUse {
            source: source.local,
            copy,
            total_copies: 0,
        });
    }

    pub(crate) fn record_format_capture(&mut self, expr: ExprId, span: Span) {
        self.format_captures
            .push(FormatCaptureSource { expr, span });
    }

    pub(crate) fn finish(&mut self, total_copies: impl Fn(u32) -> Option<u32>) {
        // An input token forwarded to another expansion can have been replaced during
        // lowering. Keep only the uses that still count after the whole worklist is lowered.
        self.argument_uses.retain_mut(|usage| {
            let Some(total) = total_copies(usage.copy) else {
                return false;
            };
            usage.total_copies = total;
            true
        });
        self.argument_uses.shrink_to_fit();
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

    /// Returns the output token ID and the input's total across the enclosing body worklist.
    pub fn macro_argument_use(&self, source: SourceId) -> Option<(u32, u32)> {
        // Entries were appended in source ID order, which is independent of written offsets.
        let index = self
            .argument_uses
            .binary_search_by_key(&source.local, |usage| usage.source)
            .ok()?;
        let usage = &self.argument_uses[index];
        Some((usage.copy, usage.total_copies))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct MacroArgumentUse {
    source: u32,
    copy: u32,
    total_copies: u32,
}

/// An implicit capture expression with its proven range inside a written literal.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct FormatCaptureSource {
    expr: ExprId,
    span: Span,
}
