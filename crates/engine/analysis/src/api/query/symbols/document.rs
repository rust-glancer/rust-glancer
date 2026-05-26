//! Document symbol query for editor outlines.

use anyhow::Result;
use rg_ir_model::TargetRef;
use rg_parse::FileId;

use crate::{
    api::{Analysis, view::symbol::SymbolView},
    model::DocumentSymbol,
};

pub(crate) struct DocumentSymbolCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> DocumentSymbolCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        Ok(SymbolView::new(self.0)
            .document_symbols(target, file_id)?
            .into_iter()
            .map(DocumentSymbol::from)
            .collect())
    }
}
