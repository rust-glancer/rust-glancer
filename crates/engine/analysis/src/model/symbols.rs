use rg_ir_model::CrateRef;
use rg_ir_view::SymbolKind;
use rg_parse::{FileId, Span};

/// Hierarchical source outline for one file under one crate context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentOutline {
    pub file_id: FileId,
    pub symbols: Vec<DocumentSymbol>,
}

/// One node in a document outline.
///
/// Children always belong to the same source file as their parent. The file identity therefore
/// lives once on `DocumentOutline` instead of being repeated throughout this recursive tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<DocumentSymbol>,
}

/// Flat symbol row suitable for workspace-wide search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbol {
    pub crate_ref: CrateRef,
    pub name: String,
    pub kind: SymbolKind,
    pub file_id: FileId,
    pub span: Option<Span>,
    pub container_name: Option<String>,
}
