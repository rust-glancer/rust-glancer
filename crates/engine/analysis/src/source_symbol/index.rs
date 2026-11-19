//! Cursor/source symbol indexing over indexed source occurrences.

use rg_ir_model::{BodyKey, CrateRef, FileId};
use rg_ir_view::{
    IndexedViewDb,
    source::{ReferenceSourceSelection, SourceOccurrenceView},
};

use super::SourceSymbol;

pub(crate) struct SourceSymbolIndex<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceSymbolIndex<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn symbols_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_at(crate_ref, file_id, offset)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    pub(crate) fn symbols_in_crate(
        &self,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
        selection: &ReferenceSourceSelection,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_in_crate(crate_ref, file_id, Some(selection))?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    pub(crate) fn symbols_in_body(&self, owner: BodyKey) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_in_body(owner)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }
}
