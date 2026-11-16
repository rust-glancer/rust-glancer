//! Semantic item reads over configured targets.

use std::sync::Arc;

use rg_ir_model::{CrateRef, DefMapRef, ItemStoreRef, LocalDefRef, SemanticItemRef};
use rg_source::ReadError;
use rg_std::CancellationToken;

use crate::{
    AssociatedItemStore, ItemLookupEnv, LangItemIndex, LookupAnswer, LookupQuestion,
    SemanticItemSource, SemanticItemView, SourceSelection, SymbolNameData, SymbolNameIndex,
};

/// Acquire item, language-item and lookup payloads independently, or enumerate their targets.
pub trait CrateItemSource: Sync {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&AssociatedItemStore>, ReadError>;

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'_>>, ReadError>;
    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError>;
    /// Read module-owned declarations and owner headers without acquiring associated stores.
    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError>;
    /// Enumerate every declaration, opening all associated stores in a saved target. Use this
    /// for whole-target operations; a file or cursor scan should select through `source_items`.
    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError>;
    /// Select written containers or every interpretation of a file, as requested explicitly by
    /// `selection`. Consumers still check exact ranges after acquisition.
    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'_>>, ReadError>;
    fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError>;
    fn lookup_env(&self, krate: CrateRef) -> Result<Option<&ItemLookupEnv>, ReadError>;
    fn lookup_candidates(
        &self,
        use_site: CrateRef,
        question: &LookupQuestion,
    ) -> Result<LookupAnswer, ReadError>;
    /// Discover a batch of target name tables. The batch lets saved readers compute independent
    /// targets concurrently; shared tables keep the result compact until filtering is finished.
    /// A target may return several tables, separating written summaries from discovered macro
    /// names. Requesting these tables can expand member macros, but does not open macro-free
    /// member stores. The search consumer opens those only when their written names match.
    fn symbol_names(&self, crates: &[CrateRef]) -> Result<Vec<Arc<SymbolNameIndex>>, ReadError>;
    fn crates(&self) -> Vec<CrateRef>;
}

#[derive(Clone, Copy)]
pub struct ItemReader<'a>(pub &'a dyn CrateItemSource);

impl std::fmt::Debug for ItemReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ItemReader")
    }
}

impl<'a> ItemReader<'a> {
    pub fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError> {
        self.0.associated_items(store)
    }

    pub fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        self.0.item(item)
    }

    pub fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        self.0.item_for_local_def(definition)
    }

    pub fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        self.0.scope_items(origin)
    }

    pub fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        self.0.all_items(origin)
    }

    pub fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        self.0.source_items(origin, selection)
    }

    pub(crate) fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError> {
        self.0.lang_items(krate)
    }

    /// Search names before acquiring signatures. Macro owners contribute discovered names;
    /// an ordinary owner is opened only when its written-name summary matches the predicate.
    pub fn search_symbols(
        &self,
        crates: &[CrateRef],
        query: &str,
        cancellation: &CancellationToken,
    ) -> Result<Vec<SymbolNameData>, ReadError> {
        let needle = query.to_lowercase();
        let matches = |name: &str| name.to_lowercase().contains(&needle);
        let mut selected = Vec::new();
        for table in self.0.symbol_names(crates)? {
            for candidate in &table.declarations {
                rg_std::check_cancel!(cancellation, "workspace symbol discovery");
                if matches(candidate.name.as_str()) {
                    selected.push(candidate.clone());
                }
            }
            for summary in &table.members {
                rg_std::check_cancel!(cancellation, "workspace member discovery");
                if !summary.names.iter().any(|name| matches(name.as_str())) {
                    continue;
                }
                // Syntax order cannot predict IDs: preceding expansions can allocate members.
                if let Some(members) = self.associated_items(summary.owner)? {
                    selected.extend(
                        SymbolNameData::for_members(summary.owner, members)
                            .filter(|candidate| matches(candidate.name.as_str())),
                    );
                }
            }
        }
        Ok(selected)
    }

    pub fn included_crates(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<CrateRef>, ReadError> {
        rg_std::check_cancel!(cancellation, "enumerate semantic targets");
        Ok(self.0.crates())
    }
}

impl<'a> SemanticItemSource<'a> for &ItemReader<'a> {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError> {
        ItemReader::associated_items(self, store)
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        ItemReader::item(self, item)
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        ItemReader::item_for_local_def(self, definition)
    }

    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        ItemReader::scope_items(self, origin)
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        ItemReader::all_items(self, origin)
    }
    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        ItemReader::source_items(self, origin, selection)
    }
}
