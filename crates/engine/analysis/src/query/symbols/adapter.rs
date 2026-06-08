//! Editor symbol models built from generic indexed symbol projections.

use rg_ir_view::symbol::{IndexedSymbolEntry, SourceOutlineDeclaration, SourceOutlineNode};

use crate::model::{DocumentSymbol, WorkspaceSymbol};

impl From<SourceOutlineDeclaration> for DocumentSymbol {
    fn from(declaration: SourceOutlineDeclaration) -> Self {
        let (name, kind, file_id, span, selection_span) = declaration.into_parts();
        Self {
            name,
            kind,
            file_id,
            span,
            selection_span,
            children: Vec::new(),
        }
    }
}

impl From<SourceOutlineNode> for DocumentSymbol {
    fn from(node: SourceOutlineNode) -> Self {
        let (declaration, children) = node.into_parts();
        let mut symbol = DocumentSymbol::from(declaration);
        symbol.children = children.into_iter().map(DocumentSymbol::from).collect();
        symbol
    }
}

impl From<IndexedSymbolEntry> for WorkspaceSymbol {
    fn from(entry: IndexedSymbolEntry) -> Self {
        let (declaration, container_name) = entry.into_parts();
        Self {
            target: declaration.target(),
            name: declaration.name().to_string(),
            kind: declaration.kind(),
            file_id: declaration.file_id(),
            span: Some(declaration.selection_span()),
            container_name,
        }
    }
}
