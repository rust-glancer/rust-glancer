//! Compiler identities at crate-local and use-site visibility layers.
//!
//! Header indexing records recognized compiler attributes. A module item already has its typed
//! identity; an associated item keeps its source identity until its owner is lowered. These
//! targets are merged into each crate's `ItemLookupEnv`, so consumers can find the declaration
//! without relying on a crate name, module path, or re-export spelling.

use rg_def_map::ItemSource;
use rg_ir_model::{ItemStoreRef, SemanticItemRef, TypeDefId, TypeDefRef};
use rg_item_tree::{ItemKind, LangItem};
use rg_source::ReadError;
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::AssociatedItemStore;

/// Associated IDs do not exist until their owner is lowered. Retain the written declaration's
/// source identity so earlier macro output cannot shift a language item to a different member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) enum LangItemTarget {
    Item(SemanticItemRef),
    Member {
        store: ItemStoreRef,
        source: ItemSource,
    },
}

impl LangItemTarget {
    pub(crate) fn resolve<'a>(
        self,
        members: impl FnOnce(ItemStoreRef) -> Result<Option<&'a AssociatedItemStore>, ReadError>,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        Ok(match self {
            Self::Item(item) => Some(item),
            Self::Member { store, source } => members(store)?.and_then(|members| {
                members
                    .semantic_items(store)
                    .find(|item| item.source() == source)
                    .map(|item| item.item())
            }),
        })
    }
}

/// Sparse crate-local index of compiler language identities.
///
/// Almost every crate has no language items, while `core` has a small fixed set that semantic
/// queries repeatedly need. Keeping only declared entries avoids adding identity fields to every
/// semantic item. The vector also preserves duplicate declarations so malformed input becomes an
/// ambiguous identity instead of whichever declaration happened to be lowered first.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LangItemIndex {
    entries: Vec<(LangItem, LangItemTarget)>,
}

impl LangItemIndex {
    /// Record a declaration only when its semantic item family matches the compiler identity.
    pub(super) fn insert(&mut self, lang_item: LangItem, target: SemanticItemRef) {
        // A language item has a compiler-defined target family. Malformed source may put the
        // attribute elsewhere; retaining that declaration as the requested identity would make
        // every typed consumer reinterpret an invalid target differently.
        let target_matches = matches!(
            (lang_item, target),
            (
                LangItem::Deref
                    | LangItem::Iterator
                    | LangItem::Try
                    | LangItem::Index
                    | LangItem::Fn
                    | LangItem::FnMut
                    | LangItem::FnOnce
                    | LangItem::PointeeSized
                    | LangItem::Sized
                    | LangItem::MetaSized
                    | LangItem::Tuple
                    | LangItem::Destruct,
                SemanticItemRef::Trait(_)
            ) | (LangItem::IntoIter, SemanticItemRef::Function(_))
                | (
                    LangItem::DerefTarget | LangItem::FnOnceOutput,
                    SemanticItemRef::TypeAlias(_)
                )
                | (
                    LangItem::FormatArguments
                        | LangItem::OwnedBox
                        | LangItem::Arc
                        | LangItem::Rc
                        | LangItem::Range
                        | LangItem::RangeFrom
                        | LangItem::RangeTo
                        | LangItem::RangeInclusive
                        | LangItem::RangeToInclusive
                        | LangItem::RangeFull,
                    SemanticItemRef::TypeDef(TypeDefRef {
                        id: TypeDefId::Struct(_),
                        ..
                    })
                )
                | (
                    LangItem::Option,
                    SemanticItemRef::TypeDef(TypeDefRef {
                        id: TypeDefId::Enum(_),
                        ..
                    })
                )
        );
        if target_matches {
            self.entries.push((lang_item, LangItemTarget::Item(target)));
        }
    }

    pub(crate) fn insert_member(
        &mut self,
        lang_item: LangItem,
        store: ItemStoreRef,
        source: ItemSource,
        kind: &ItemKind,
    ) {
        if matches!(
            (lang_item, kind),
            (LangItem::IntoIter, ItemKind::Function(_))
                | (
                    LangItem::DerefTarget | LangItem::FnOnceOutput,
                    ItemKind::TypeAlias(_)
                )
        ) {
            self.entries
                .push((lang_item, LangItemTarget::Member { store, source }));
        }
        // TODO: Language attributes produced by member macros need discovery without eagerly
        // expanding every owner during module indexing.
    }

    pub(super) fn target(&self, lang_item: LangItem) -> ExpectedUnique<LangItemTarget> {
        let mut target = ExpectedUnique::new();
        for (_, item) in self
            .entries
            .iter()
            .filter(|(candidate, _)| *candidate == lang_item)
        {
            target.push(*item);
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
    entries: Vec<(LangItem, ExpectedUnique<LangItemTarget>)>,
}

impl VisibleLangItems {
    /// Keep the first non-empty visible result, including a malformed ambiguous result.
    pub(super) fn merge_prefer_existing(
        &mut self,
        lang_item: LangItem,
        target: ExpectedUnique<LangItemTarget>,
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
    pub(super) fn target(&self, lang_item: LangItem) -> Option<LangItemTarget> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == lang_item)
            .and_then(|(_, target)| target.as_option().copied())
    }
}
