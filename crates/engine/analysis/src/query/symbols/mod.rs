mod adapter;
mod document;
mod workspace;

use anyhow::Result;
use rg_ir_model::CrateRef;
use rg_parse::FileId;

use crate::{
    Analysis,
    model::{DocumentSymbol, WorkspaceSymbol},
};

pub(crate) struct SymbolCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn document_symbols(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        document::DocumentSymbolCollector::new(self.0.view_db())
            .document_symbols(crate_ref, file_id)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        workspace::WorkspaceSymbolCollector::new(self.0.view_db()).workspace_symbols(query)
    }
}
