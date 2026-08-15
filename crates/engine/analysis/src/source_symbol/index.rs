//! Cursor/source symbol indexing over indexed source occurrences.

use rg_ir_model::CrateRef;
use rg_ir_view::{IndexedViewDb, source::SourceOccurrenceView};
use rg_parse::FileId;

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

    /// Read only Body IR symbols when the offset belongs to rebuilt current source.
    pub(crate) fn body_symbols_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .body_occurrences_at(crate_ref, file_id, offset)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    /// Read declarations from saved DefMap and signature coordinates, excluding Body IR.
    pub(crate) fn saved_declaration_symbols_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .saved_declaration_occurrences_at(crate_ref, file_id, offset)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    pub(crate) fn symbols_in_crate(
        &self,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_in_crate(crate_ref, file_id)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    /// Read only Body IR occurrences from a current-source file or crate surface.
    pub(crate) fn body_symbols_in_crate(
        &self,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .body_occurrences_in_crate(crate_ref, file_id)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }
}
