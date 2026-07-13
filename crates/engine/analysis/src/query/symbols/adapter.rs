//! Editor symbol models built from generic indexed symbol projections.

use rg_ir_view::{
    IndexedViewDb,
    item::declaration::DeclarationView,
    symbol::{IndexedSymbolEntry, SourceOutlineDeclaration, SourceOutlineNode},
};

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

pub(super) fn workspace_symbol(
    db: &IndexedViewDb<'_>,
    entry: IndexedSymbolEntry,
) -> anyhow::Result<WorkspaceSymbol> {
    let (declaration, container_name) = entry.into_parts();
    let name = DeclarationView::new(db)
        .declaration_site_name(&declaration)?
        .to_string();
    Ok(WorkspaceSymbol {
        crate_ref: declaration.crate_ref(),
        name,
        kind: declaration.kind(),
        file_id: declaration.file_id(),
        span: Some(declaration.selection_span()),
        container_name,
    })
}
