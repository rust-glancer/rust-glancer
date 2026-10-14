//! Visibility and compiler language items selected during semantic indexing.
//!
//! A crate's dependency visibility and selected compiler language items stay fixed until its
//! declarations or dependencies change. These small facts are saved with the crate's lookup index
//! so later queries can borrow them without walking the dependency graph again.

use rg_def_map::DefMapSource;
use rg_ir_model::{CrateRef, DefMapRef, SemanticItemRef};
use rg_item_tree::LangItem;
use rg_std::{CancellationToken, MemorySize, OperationError, Shrink, UniqueVec};
use wincode::{SchemaRead, SchemaWrite};

use super::{lang_item::VisibleLangItems, query::ItemStoreSource};
use crate::CrateItemQuery;

/// Saved lookup choices for one crate, including its visible dependencies.
///
/// For example, when `*value` uses a `Deref` impl, lookup needs the trait marked `#[lang = "deref"]`,
/// regardless of its name or module path. Indexing selects that declaration once, along with the
/// crates whose impls can participate in lookup. Queries then read these facts without loading
/// every visible crate's declaration store.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemLookupFacts {
    // The use-site crate comes first; dependency order also sets language-item precedence.
    visible_crates: UniqueVec<CrateRef>,
    lang_items: VisibleLangItems,
}

impl ItemLookupFacts {
    /// Record DefMap's item-lookup order and select language items from those crates' declarations.
    pub fn build<'item, D, I>(
        crate_items: &CrateItemQuery<'item, D, I>,
        cancellation: &CancellationToken,
    ) -> Result<Self, OperationError<I::Error>>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'item>,
    {
        let stores = crate_items
            .visible_stores()
            .map_err(OperationError::Source)?;
        let mut lang_items = VisibleLangItems::default();

        // The first crate that declares a language item wins. For example, a local
        // #[lang = "deref"] takes precedence over the dependency's Deref trait. If that first
        // crate has duplicate declarations, keep the ambiguity instead of falling through to
        // a dependency and silently selecting a different trait.
        for store in &stores {
            rg_std::check_cancel!(cancellation, "visible language items");
            let origin = DefMapRef::Crate(store.crate_ref());
            for lang_item in LangItem::ALL {
                lang_items
                    .merge_prefer_existing(lang_item, store.lang_items().target(lang_item, origin));
            }
        }

        Ok(Self {
            visible_crates: stores.iter().map(|store| store.crate_ref()).collect(),
            lang_items,
        })
    }

    pub(crate) fn visible_crates(&self) -> &UniqueVec<CrateRef> {
        &self.visible_crates
    }

    pub(crate) fn lang_item(&self, lang_item: LangItem) -> Option<SemanticItemRef> {
        self.lang_items.target(lang_item)
    }
}
