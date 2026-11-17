//! Workspace-wide symbol search.

use anyhow::Result;
use rg_ir_view::{IndexedViewDb, symbol::SymbolView};

use super::adapter::workspace_symbol;
use crate::model::WorkspaceSymbol;

pub(crate) struct WorkspaceSymbolCollector<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> WorkspaceSymbolCollector<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        let mut symbols = Vec::new();

        for symbol in SymbolView::new(self.0).workspace_symbols(query)? {
            rg_std::check_cancel!(self.0, "workspace symbol filtering");
            symbols.push(workspace_symbol(self.0, symbol)?);
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.name.to_lowercase(),
                symbol.kind,
                symbol.container_name.clone(),
                symbol.crate_ref.package.0,
                symbol.crate_ref.crate_id.0,
                symbol.file_id,
                symbol.span.map(|span| span.start),
            )
        });
        Ok(symbols)
    }
}
