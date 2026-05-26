//! Chooses the most specific analysis symbol at one source offset.

use rg_def_map::TargetRef;
use rg_parse::FileId;

use crate::{
    api::{Analysis, source_symbol::SourceSymbolIndex},
    model::SymbolAt,
};

pub(crate) struct SymbolFinder<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolFinder<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn symbol_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<SymbolAt>> {
        // Overlapping syntax is common around type paths and expressions. The narrowest span is
        // the best proxy for the thing the user actually placed the cursor on.
        let symbol = SourceSymbolIndex::new(self.0)
            .symbols_at(target, file_id, offset)?
            .into_iter()
            .min_by_key(|candidate| candidate.span().len())
            .map(|candidate| candidate.into_symbol());
        Ok(symbol)
    }
}
