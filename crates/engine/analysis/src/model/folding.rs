use rg_parse::Span;

/// One source range that an editor may collapse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    pub span: Span,
    pub kind: FoldKind,
}

/// Semantic category used by editor commands such as "Fold All Block Comments".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Code,
    Comment,
    Imports,
}
