mod document;
pub(crate) mod shared;
mod workspace;

use anyhow::Result;
use rg_def_map::TargetRef;
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
        document::DocumentSymbolCollector::new(self.0).document_symbols(target, file_id)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        workspace::WorkspaceSymbolCollector::new(self.0).workspace_symbols(query)
    }
}
