use rg_ir_model::CrateRef;
use rg_parse::{FileId, Span};

/// One source occurrence of the declaration-like subject selected by a references query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceLocation {
    pub crate_ref: CrateRef,
    pub file_id: FileId,
    pub span: Span,
}
