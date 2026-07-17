//! Compiler language identities at crate-local and use-site visibility layers.
//!
//! Semantic lowering first records `#[lang = "..."]` beside the typed item id in its owning
//! `ItemStore`. `ItemLookupIndex` later merges those sparse declarations across the stores visible
//! from one crate. Consumers therefore ask for the real declaration identity without relying on a
//! crate name, module path, or re-export spelling.

use rg_ir_model::{ItemId, SemanticItemRef, items::LangItem};
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Sparse crate-local index of compiler language identities.
///
/// Almost every crate has no language items, while `core` has a small fixed set that semantic
/// queries repeatedly need. Keeping only declared entries avoids adding identity fields to every
/// semantic item. The vector also preserves duplicate declarations so malformed input becomes an
/// ambiguous identity instead of whichever declaration happened to be lowered first.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(super) struct LangItemIndex {
    entries: Vec<(LangItem, ItemId)>,
}

impl LangItemIndex {
    /// Record a declaration only when its semantic item family matches the compiler identity.
    pub(super) fn insert(&mut self, lang_item: LangItem, target: ItemId) {
        // A language item has a compiler-defined target family. Malformed source may put the
        // attribute elsewhere; retaining that declaration as the requested identity would make
        // every typed consumer reinterpret an invalid target differently.
        let target_matches = matches!(
            (lang_item, target),
            (
                LangItem::Deref | LangItem::Fn | LangItem::FnMut | LangItem::FnOnce,
                ItemId::Trait(_)
            ) | (LangItem::IntoIter, ItemId::Function(_))
                | (
                    LangItem::DerefTarget | LangItem::FnOnceOutput,
                    ItemId::TypeAlias(_)
                )
        );
        if target_matches {
            self.entries.push((lang_item, target));
        }
    }

    pub(super) fn target(
        &self,
        lang_item: LangItem,
        origin: rg_ir_model::DefMapRef,
    ) -> ExpectedUnique<SemanticItemRef> {
        let mut target = ExpectedUnique::new();
        for (_, item) in self
            .entries
            .iter()
            .filter(|(candidate, _)| *candidate == lang_item)
        {
            target.push(item.semantic_ref(origin));
        }
        target
    }
}

/// Language identities merged for one use-site crate and its visible dependencies.
///
/// Visible stores are scanned from the use-site crate outward. The first store that declares an
/// identity wins. An ambiguous declaration in that store is retained as a closed result rather
/// than falling through to a dependency, because doing so would silently reinterpret malformed
/// source as a different crate's language item.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(super) struct VisibleLangItems {
    entries: Vec<(LangItem, ExpectedUnique<SemanticItemRef>)>,
}

impl VisibleLangItems {
    /// Keep the first non-empty visible result, including a malformed ambiguous result.
    pub(super) fn merge_prefer_existing(
        &mut self,
        lang_item: LangItem,
        target: ExpectedUnique<SemanticItemRef>,
    ) {
        if target.is_empty()
            || self
                .entries
                .iter()
                .any(|(existing, _)| *existing == lang_item)
        {
            return;
        }
        self.entries.push((lang_item, target));
    }

    /// Return a declaration only when visibility produced exactly one identity.
    pub(super) fn target(&self, lang_item: LangItem) -> Option<SemanticItemRef> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == lang_item)
            .and_then(|(_, target)| target.as_option().copied())
    }
}
