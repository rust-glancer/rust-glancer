//! Workspace-wide symbol search.

use anyhow::Result;
use rg_ir_view::{IndexedViewDb, symbol::SymbolView};

use crate::model::WorkspaceSymbol;

pub(crate) struct WorkspaceSymbolCollector<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> WorkspaceSymbolCollector<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        let query = WorkspaceSymbolQuery::new(query);
        let mut symbols = Vec::new();

        for symbol in SymbolView::new(self.0).workspace_symbols()? {
            if !query.matches(symbol.name()) {
                continue;
            }

            symbols.push(WorkspaceSymbol::from(symbol));
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.name.to_lowercase(),
                symbol.kind,
                symbol.container_name.clone(),
                symbol.target.package.0,
                symbol.target.target.0,
                symbol.file_id.0,
                symbol.span.map(|span| span.text.start),
            )
        });
        Ok(symbols)
    }
}

struct WorkspaceSymbolQuery {
    needle: String,
}

impl WorkspaceSymbolQuery {
    fn new(query: &str) -> Self {
        Self {
            needle: query.to_lowercase(),
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.needle.is_empty() || name.to_lowercase().contains(&self.needle)
    }
}
