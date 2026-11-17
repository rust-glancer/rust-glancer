//! Visibility and compiler language items selected during semantic indexing.
//!
//! A crate's dependency visibility and selected compiler language items stay fixed until its
//! declarations or dependencies change. These small facts are saved with the crate's lookup index
//! so later queries can borrow them without walking the dependency graph again.

use rg_ir_model::{CrateRef, DefMapRef, SemanticItemRef};
use rg_item_tree::LangItem;
use rg_source::ReadError;
use rg_std::{MemorySize, OperationError, Shrink, UniqueVec};
use wincode::{SchemaRead, SchemaWrite};

use super::lang_item::VisibleLangItems;
use crate::{ItemReader, ItemStore};

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
    /// Select language items in DefMap's lookup order, using the local store before it is published.
    pub fn build(
        crates: UniqueVec<CrateRef>,
        local: &ItemStore,
        dependencies: ItemReader<'_>,
        mut check_cancelled: impl FnMut() -> Result<(), OperationError<ReadError>>,
    ) -> Result<Self, OperationError<ReadError>> {
        let mut visible_crates = UniqueVec::new();
        let mut lang_items = VisibleLangItems::default();

        // The first crate that declares a language item wins. For example, a local
        // #[lang = "deref"] takes precedence over the dependency's Deref trait. If that first
        // crate has duplicate declarations, keep the ambiguity instead of falling through to
        // a dependency and silently selecting a different trait.
        for krate in crates {
            check_cancelled()?;
            // A dependency's signatures can change while its language-item identities stay equal.
            // Reading only this output keeps those edits out of the declaration's dependencies.
            let index = if krate == local.crate_ref() {
                local.lang_items()
            } else {
                let Some(index) = dependencies
                    .lang_items(krate)
                    .map_err(OperationError::Source)?
                else {
                    continue;
                };
                index
            };
            visible_crates.push(krate);
            let origin = DefMapRef::Crate(krate);
            for lang_item in LangItem::ALL {
                lang_items.merge_prefer_existing(lang_item, index.target(lang_item, origin));
            }
        }

        Ok(Self {
            visible_crates,
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
