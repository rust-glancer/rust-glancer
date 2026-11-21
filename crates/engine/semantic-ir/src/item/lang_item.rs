//! Compiler identities at crate-local and use-site visibility layers.
//!
//! Lowering records recognized compiler attributes after allocating each declaration's semantic
//! identity. Preparing a crate's index also lowers the owners of written associated lang items.
//! Their final references are merged into each crate's `ItemLookupEnv`, so consumers can find the
//! declaration without relying on a crate name, module path, or re-export spelling.

use rg_ir_model::{SemanticItemRef, TypeDefId, TypeDefRef};
use rg_item_tree::LangItem;
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Sparse index of compiler language identities, all pointing to allocated semantic items.
///
/// Almost every crate has no language items, while `core` has a small fixed set that semantic
/// queries repeatedly need. Keeping only declared entries avoids adding identity fields to every
/// semantic item. The vector also preserves duplicate declarations so malformed input becomes an
/// ambiguous identity instead of whichever declaration happened to be lowered first.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LangItemIndex {
    entries: Vec<(LangItem, SemanticItemRef)>,
}

impl LangItemIndex {
    /// Record a declaration only when its semantic item family matches the compiler identity.
    pub(crate) fn insert(&mut self, lang_item: LangItem, target: SemanticItemRef) {
        // A language item has a compiler-defined target family. Malformed source may put the
        // attribute elsewhere; retaining that declaration as the requested identity would make
        // every typed consumer reinterpret an invalid target differently.
        let target_matches = matches!(
            (lang_item, target),
            (
                LangItem::Deref
                    | LangItem::DerefMut
                    | LangItem::Iterator
                    | LangItem::Try
                    | LangItem::Future
                    | LangItem::IntoFuture
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
                    LangItem::DerefTarget | LangItem::FnOnceOutput | LangItem::FutureOutput,
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
            self.entries.push((lang_item, target));
        }
    }

    pub(crate) fn extend(&mut self, other: &Self) {
        self.entries.extend_from_slice(&other.entries);
    }

    pub(super) fn target(&self, lang_item: LangItem) -> ExpectedUnique<SemanticItemRef> {
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
