use rg_ir_model::TargetRef;
use rg_parse::{FileId, Span};

/// One source occurrence of the declaration-like subject selected by a references query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceLocation {
    pub target: TargetRef,
    pub file_id: FileId,
    pub span: Span,
}
