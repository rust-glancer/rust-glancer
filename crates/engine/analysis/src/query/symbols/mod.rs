mod syntax;
mod workspace;

use anyhow::Result;
use rg_syntax::SourceFile;

use crate::{
    Analysis,
    model::{DocumentSymbol, WorkspaceSymbol},
};

pub(crate) struct SymbolCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Build a file-independent outline from syntax parsed with the document's Rust edition.
    pub(crate) fn document_symbols_from_syntax(syntax: &SourceFile) -> Vec<DocumentSymbol> {
        syntax::SyntaxDocumentSymbolCollector::collect(syntax)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        workspace::WorkspaceSymbolCollector::new(self.0.view_db()).workspace_symbols(query)
    }
}
