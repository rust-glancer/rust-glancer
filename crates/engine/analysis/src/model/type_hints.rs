use rg_parse::{FileId, Span};

/// One best-effort inferred type annotation suitable for editor inlay hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeHint {
    pub file_id: FileId,
    pub span: Span,
    pub label: String,
}
