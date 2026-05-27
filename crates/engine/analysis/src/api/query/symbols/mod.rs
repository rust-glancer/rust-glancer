mod document;
mod indexed;
mod workspace;

use anyhow::Result;
use rg_ir_model::TargetRef;
use rg_parse::FileId;

use crate::{
    api::Analysis,
    model::{DocumentSymbol, WorkspaceSymbol},
};

pub(crate) struct SymbolCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        document::DocumentSymbolCollector::new(self.0.view_db()).document_symbols(target, file_id)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        workspace::WorkspaceSymbolCollector::new(self.0.view_db()).workspace_symbols(query)
    }
}
