use rg_ir_model::Span;

use crate::SymbolKind;

/// Source coloring before any editor-specific token legend or position encoding is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    pub span: Span,
    pub kind: HighlightKind,
    pub documentation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Symbol(SymbolKind),
    Keyword,
    String,
    Number,
    Operator,
    Comment,
    Parameter,
    TypeParameter,
}
