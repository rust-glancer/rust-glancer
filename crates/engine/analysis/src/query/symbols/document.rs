//! Document symbol query for editor outlines.

use anyhow::Result;
use rg_ir_model::CrateRef;
use rg_ir_view::{IndexedViewDb, symbol::SymbolView};
use rg_parse::FileId;

use crate::model::{DocumentOutline, DocumentSymbol};

pub(crate) struct DocumentSymbolCollector<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> DocumentSymbolCollector<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub(crate) fn document_symbols(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> Result<DocumentOutline> {
        let symbols = SymbolView::new(self.0)
            .source_outline(crate_ref, file_id)?
            .into_iter()
            .map(DocumentSymbol::from)
            .collect();
        Ok(DocumentOutline { file_id, symbols })
    }
}
