//! Document symbol query for editor outlines.

use anyhow::Result;
use rg_ir_model::TargetRef;
use rg_parse::FileId;

use crate::{api::view::IndexedViewDb, model::DocumentSymbol};

use super::indexed::IndexedSymbols;

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
        Ok(IndexedSymbols::new(self.0)
            .document_symbols(target, file_id)?
            .into_iter()
            .map(DocumentSymbol::from)
            .collect())
    }
}
