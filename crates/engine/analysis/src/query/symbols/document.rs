//! Document symbol query for editor outlines.

use anyhow::Result;
use rg_ir_model::TargetRef;
use rg_ir_view::{IndexedViewDb, symbol::SymbolView};
use rg_parse::FileId;

use crate::model::DocumentSymbol;

pub(crate) struct DocumentSymbolCollector<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> DocumentSymbolCollector<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub(crate) fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        Ok(SymbolView::new(self.0)
            .source_outline(target, file_id)?
            .into_iter()
            .map(DocumentSymbol::from)
            .collect())
    }
}
