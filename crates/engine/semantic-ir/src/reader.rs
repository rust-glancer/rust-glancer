//! Semantic item reads over one configured target directory.

use rg_ir_model::{CrateRef, DefMapRef};
use rg_source::ReadError;
use rg_std::{CancellationToken, OperationError};

use crate::{ItemLookupIndex, ItemStore, ItemStoreSource, LangItemIndex};

/// Acquire item, language-item and lookup payloads independently, or enumerate their targets.
pub trait CrateItemSource: Sync {
    fn items(&self, krate: CrateRef) -> Result<Option<&ItemStore>, ReadError>;
    fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError>;
    fn lookup_index(&self, krate: CrateRef) -> Result<Option<&ItemLookupIndex>, ReadError>;
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
    pub fn items(&self, krate: CrateRef) -> Result<Option<&ItemStore>, ReadError> {
        self.0.items(krate)
    }

    pub(crate) fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError> {
        self.0.lang_items(krate)
    }

    pub fn included_crates(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<CrateRef>, OperationError<ReadError>> {
        rg_std::check_cancel!(cancellation, "enumerate item stores");
        Ok(self.0.crates())
    }

    pub(crate) fn lookup_index(
        &self,
        krate: CrateRef,
    ) -> Result<Option<&'a ItemLookupIndex>, ReadError> {
        self.0.lookup_index(krate)
    }
}

impl<'a> ItemStoreSource<'a> for &'a ItemReader<'_> {
    type Error = ReadError;

    fn item_store_for_origin(&self, origin: DefMapRef) -> Result<Option<&'a ItemStore>, ReadError> {
        let Some(krate) = origin.as_crate_ref() else {
            return Ok(None);
        };
        (*self).items(krate)
    }
}
