//! Compiler language items selected for one use site.
//!
//! Declaration indexing supplies the visible crates in lookup order. This environment selects
//! their language identities after member IDs have been allocated. Candidate lookup reads the
//! visibility list separately, so it does not need this preparation just to select candidate shards.

use rg_ir_model::SemanticItemRef;
use rg_item_tree::LangItem;
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::lang_item::LangItemIndex;

/// Saved lookup choices for one crate, including its visible dependencies.
///
/// For example, when `*value` uses a `Deref` impl, lookup needs the trait marked `#[lang = "deref"]`,
/// regardless of its name or module path. Preparation selects that declaration once. Queries then
/// read this environment without loading member stores or searching source identities.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemLookupEnv {
    lang_items: Vec<(LangItem, ExpectedUnique<SemanticItemRef>)>,
}

impl ItemLookupEnv {
    /// Select completed language-item indexes in DefMap's lookup order, with the use site first.
    pub(crate) fn build<'a>(indexes: impl IntoIterator<Item = &'a LangItemIndex>) -> Self {
        let mut lang_items = Vec::new();

        // The first crate that declares a language item wins. For example, a local
        // #[lang = "deref"] takes precedence over the dependency's Deref trait. If that first
        // crate has duplicate declarations, keep the ambiguity instead of falling through to
        // a dependency and silently selecting a different trait.
        for index in indexes {
            for lang_item in LangItem::ALL {
                let target = index.target(lang_item);
                if !target.is_empty()
                    && !lang_items
                        .iter()
                        .any(|(existing, _)| *existing == lang_item)
                {
                    lang_items.push((lang_item, target));
                }
            }
        }

        Self { lang_items }
    }

    pub(crate) fn lang_item(&self, lang_item: LangItem) -> Option<SemanticItemRef> {
        self.lang_items
            .iter()
            .find(|(candidate, _)| *candidate == lang_item)
            .and_then(|(_, target)| target.as_option().copied())
    }
}
