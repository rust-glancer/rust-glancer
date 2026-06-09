//! Cursor/source symbol indexing over indexed source occurrences.

use rg_ir_model::TargetRef;
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
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_at(target, file_id, offset)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }

    pub(crate) fn symbols_in_target(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        Ok(SourceOccurrenceView::new(self.db)
            .occurrences_in_target(target, file_id)?
            .into_iter()
            .map(SourceSymbol::from_occurrence)
            .collect())
    }
}
