use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span};

/// Source span that can be renamed from a selected cursor position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameTarget {
    pub file_id: FileId,
    pub span: Span,
    pub placeholder: String,
}

/// One source edit produced by a semantic rename query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameEdit {
    pub target: TargetRef,
    pub file_id: FileId,
    pub span: Span,
    pub old_text: String,
    pub new_text: String,
}

/// Complete rename result before conversion into editor protocol types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameResult {
    pub target: RenameTarget,
    pub edits: Vec<RenameEdit>,
}
