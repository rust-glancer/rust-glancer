//! Semantic item reads over one configured target directory.

use rg_ir_model::{CrateRef, DefMapRef, LocalDefRef, SemanticItemRef};
use rg_source::ReadError;
use rg_std::CancellationToken;

use crate::{
    ItemLookupEnv, LangItemIndex, LookupAnswer, LookupQuestion, SemanticItemSource,
    SemanticItemView, SymbolNameData,
};

/// Acquire item, language-item and lookup payloads independently, or enumerate their targets.
pub trait CrateItemSource: Sync {
    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'_>>, ReadError>;
    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError>;
    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError>;
    fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError>;
    fn lookup_env(&self, krate: CrateRef) -> Result<Option<&ItemLookupEnv>, ReadError>;
    fn lookup_candidates(
        &self,
        use_site: CrateRef,
        question: &LookupQuestion,
    ) -> Result<LookupAnswer, ReadError>;
    fn symbol_names(&self, krate: CrateRef) -> Result<&[SymbolNameData], ReadError>;
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
    pub fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        self.0.item(item)
    }

    pub fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        self.0.item_for_local_def(definition)
    }

    pub fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        self.0.all_items(origin)
    }

    pub(crate) fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError> {
        self.0.lang_items(krate)
    }

    pub fn symbol_names(&self, krate: CrateRef) -> Result<&'a [SymbolNameData], ReadError> {
        self.0.symbol_names(krate)
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
    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        ItemReader::item(self, item)
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        ItemReader::item_for_local_def(self, definition)
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        ItemReader::all_items(self, origin)
    }
}
