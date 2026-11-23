//! Source identities for declarations and the provenance retained from macro expansion.
//!
//! An item source tells readers which file, expansion, or body owns a declaration's occurrence
//! IDs. Generated declarations also need the connection back to input tokens: their parsed syntax
//! is temporary, and its offsets are not positions that an editor can navigate to or edit.
//!
//! The generated source store keeps those origins after Semantic IR has read the declarations.
//! If a reader later needs to lower a function or initializer, it can reopen that item from the
//! saved expansion tokens.

mod generated;
mod generated_bodies;

use rg_arena::ArenaId;
use rg_ir_model::{BodyRef, CrateRef, FileId, ImplId, SourceLocation, Span, TraitId};
use rg_item_tree::{ItemTreeId, ItemTreeRef};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

pub(crate) use self::generated::GeneratedSourceData;
pub use self::{
    generated::{
        ExpansionTokenCopies, GeneratedExpansionLocations, GeneratedItemStore, GeneratedLocation,
        GeneratedLocations, GeneratedSourceStore,
    },
    generated_bodies::GeneratedBodySyntax,
};
use crate::{MacroCopyRef, MacroExpansionRef};

/// A resolved occurrence and the expansion copy that produced it, when tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceOccurrenceLocation {
    pub location: SourceLocation,
    pub copy: Option<MacroCopyRef>,
}

impl SourceOccurrenceLocation {
    pub fn span(self) -> Span {
        self.location.span
    }
}

/// Stable identifier of one macro expansion within its owning source store.
///
/// The id remains part of compact source provenance after the payload is discarded, just as an
/// [`ItemTreeRef`] remains useful after the transient ItemTree phase has finished.
/// It selects the same expansion slot in [`GeneratedItemStore`] during construction and in
/// [`GeneratedSourceStore`] after declaration meaning has been lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct GeneratedSourceId(pub usize);

impl ArenaId for GeneratedSourceId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Identifies the query output that owns generated sources within one configured target.
///
/// This is independent of semantic ownership. A module macro producing `impl S { fn f() {} }`
/// stores both declarations' source in `Target`, even though `f` belongs to the impl's member store.
/// Only expansions performed while collecting that impl's members use `ImplMembers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum GeneratedSourceStoreId {
    Target,
    TraitMembers(TraitId),
    ImplMembers(ImplId),
}

/// Source identity for one generated item within a target or member expansion store.
///
/// During Semantic IR lowering this addresses [`GeneratedItemStore`]. Later phases use the
/// same identity to open occurrence mappings or the tokens needed to lower a body.
///
/// Two independently expanded impls can both allocate `GeneratedSourceId(0)`. `source_store` selects
/// which owner's expansion table to open before `source` and `item` select entries inside it.
/// The producing store supplies this identity; semantic lowering must preserve it when assigning
/// declarations to their semantic owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct GeneratedItemRef {
    source_store: GeneratedSourceStoreId,
    source: GeneratedSourceId,
    item: ItemTreeId,
}

impl GeneratedItemRef {
    pub fn expansion(self, krate: CrateRef) -> MacroExpansionRef {
        MacroExpansionRef::Item {
            krate,
            store: self.source_store,
            source: self.source,
        }
    }

    pub fn source_store(self) -> GeneratedSourceStoreId {
        self.source_store
    }

    pub fn source(self) -> GeneratedSourceId {
        self.source
    }

    pub fn item(self) -> ItemTreeId {
        self.item
    }
}

/// Body-local reference to one item-tree-shaped source payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyItemSourceRef {
    pub body: BodyRef,
    pub item: ItemTreeId,
}

/// Durable source identity for definitions collected into DefMap and later IR layers.
///
/// A declaration carries this alongside its [`SourceId`](rg_ir_model::SourceId) values. `kind` tells a source reader
/// whether to open a file table, follow a generated expansion, or read a body's table. Looking up
/// an ID using `file_id` alone is not enough: two macro invocations in that file have separate
/// generated tables whose IDs can overlap.
///
/// The item index inside `kind` selects a declaration payload. It is not the owner index in a
/// `SourceId`; source owners are allocated separately while lowering that payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ItemSource {
    pub file_id: FileId,
    pub kind: ItemSourceKind,
}

impl ItemSource {
    pub fn item_tree(source: ItemTreeRef) -> Self {
        Self {
            file_id: source.file_id,
            kind: ItemSourceKind::ItemTree(source),
        }
    }

    pub fn generated(file_id: FileId, source: GeneratedItemRef) -> Self {
        Self {
            file_id,
            kind: ItemSourceKind::Generated(source),
        }
    }

    pub fn synthetic(file_id: FileId, source: GeneratedItemRef) -> Self {
        Self {
            file_id,
            kind: ItemSourceKind::Synthetic(source),
        }
    }

    pub fn body(file_id: FileId, source: BodyItemSourceRef) -> Self {
        Self {
            file_id,
            kind: ItemSourceKind::Body(source),
        }
    }

    /// Returns a source identity for an associated item in the same underlying item arena.
    pub fn with_item(self, item: ItemTreeId) -> Self {
        let kind = match self.kind {
            ItemSourceKind::ItemTree(source) => ItemSourceKind::ItemTree(ItemTreeRef {
                file_id: source.file_id,
                item,
            }),
            // Selecting a child changes only its item slot, never the source's allocation owner.
            ItemSourceKind::Generated(source) => {
                ItemSourceKind::Generated(GeneratedItemRef { item, ..source })
            }
            ItemSourceKind::Synthetic(source) => {
                ItemSourceKind::Synthetic(GeneratedItemRef { item, ..source })
            }
            ItemSourceKind::Body(source) => ItemSourceKind::Body(BodyItemSourceRef {
                body: source.body,
                item,
            }),
        };

        Self {
            file_id: self.file_id,
            kind,
        }
    }
}

impl From<ItemTreeRef> for ItemSource {
    fn from(source: ItemTreeRef) -> Self {
        Self::item_tree(source)
    }
}

/// Where an item payload lives and whether it represents written syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ItemSourceKind {
    ItemTree(ItemTreeRef),
    Generated(GeneratedItemRef),
    /// An analysis-only item stored alongside generated items. Its invented signature paths
    /// have no editable source occurrences, even though the item retains an origin location.
    Synthetic(GeneratedItemRef),
    Body(BodyItemSourceRef),
}
