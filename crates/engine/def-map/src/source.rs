//! Source identities for declarations and the provenance retained from macro expansion.
//!
//! An item source tells readers which file, expansion, or body owns a declaration's occurrence
//! IDs. Generated declarations also need the connection back to input tokens: their parsed syntax
//! is temporary, and its offsets are not positions that an editor can navigate to or edit.

use std::collections::{HashMap, HashSet};

use rg_arena::Arena;
use rg_ir_model::{BodyRef, FileId, PackageSlot, SourceId};
use rg_item_tree::{DeclarationSourceStore, ItemNode, ItemTreeId, ItemTreeRef};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Stable identifier of one macro expansion payload produced during crate construction.
///
/// The id remains part of compact source provenance after the payload is discarded, just as an
/// [`ItemTreeRef`] remains useful after the transient ItemTree phase has finished.
/// It selects the same expansion slot in [`GeneratedItemStore`] during construction and in
/// [`GeneratedSourceStore`] after declaration meaning has been lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct GeneratedSourceId(pub usize);

impl rg_arena::ArenaId for GeneratedSourceId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Crate-local source identity for one generated item.
///
/// During Semantic IR lowering this addresses [`GeneratedItemStore`]. Later phases retain it only
/// as provenance and use the semantic declaration data copied during lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct GeneratedItemRef {
    pub source: GeneratedSourceId,
    pub item: ItemTreeId,
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
/// A declaration carries this alongside its [`SourceId`] values. `kind` tells a source reader
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

    /// Returns the ordinary item-tree source when this definition did not come from expansion or
    /// a body-local item arena.
    pub fn as_item_tree(self) -> Option<ItemTreeRef> {
        match self.kind {
            ItemSourceKind::ItemTree(source) => Some(source),
            ItemSourceKind::Generated(_) | ItemSourceKind::Synthetic(_) => None,
            ItemSourceKind::Body(_) => None,
        }
    }

    /// Returns a source identity for an associated item in the same underlying item arena.
    pub fn with_item(self, item: ItemTreeId) -> Self {
        let kind = match self.kind {
            ItemSourceKind::ItemTree(source) => ItemSourceKind::ItemTree(ItemTreeRef {
                file_id: source.file_id,
                item,
            }),
            ItemSourceKind::Generated(source) => ItemSourceKind::Generated(GeneratedItemRef {
                source: source.source,
                item,
            }),
            ItemSourceKind::Synthetic(source) => ItemSourceKind::Synthetic(GeneratedItemRef {
                source: source.source,
                item,
            }),
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

/// Item-tree-shaped payload produced for one macro expansion or synthetic declaration.
///
/// DefMap uses this while collecting scopes, and Semantic IR copies the declaration facts it
/// needs. The item payload is then dropped; `locations` moves into [`GeneratedSourceStore`].
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub(crate) struct GeneratedSourceData {
    pub(crate) origin_file_id: FileId,
    pub(crate) locations: GeneratedLocations,
    pub(crate) origin_source: ItemTreeRef,
    pub(crate) top_level: Vec<ItemTreeId>,
    pub(crate) items: Arena<ItemTreeId, ItemNode>,
}

impl GeneratedSourceData {
    pub(crate) fn item(&self, item_id: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item_id)
    }
}

/// Crate-local generated declarations retained only between DefMap and Semantic IR construction.
///
/// Macro expansion needs item-tree-shaped declarations while it discovers modules, definitions,
/// imports, and associated items. Those declarations are substantially larger than the resulting
/// semantic facts, so this store travels beside the frozen DefMap instead of becoming part of it.
#[derive(Debug, Clone, Default, MemorySize, Shrink)]
pub struct GeneratedItemStore {
    sources: Arena<GeneratedSourceId, GeneratedSourceData>,
    /// Generated associated items keyed by the macro call they replace.
    ///
    /// For `impl User { methods!(); }`, the call source maps to its generated functions, types,
    /// consts, and any retained nested macro calls.
    associated_macro_expansions: HashMap<ItemSource, Vec<ItemSource>>,
}

impl GeneratedItemStore {
    /// Keep provenance after Semantic IR has copied the generated declaration meaning. Preserve
    /// expansion slot order so existing [`GeneratedSourceId`] values still select the same source.
    pub fn into_sources(self) -> GeneratedSourceStore {
        GeneratedSourceStore {
            sources: self
                .sources
                .into_vec()
                .into_iter()
                .map(|source| source.locations)
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.associated_macro_expansions.is_empty()
    }

    pub(crate) fn alloc_source(&mut self, source: GeneratedSourceData) -> GeneratedSourceId {
        self.sources.alloc(source)
    }

    pub(crate) fn source(&self, source: GeneratedSourceId) -> Option<&GeneratedSourceData> {
        self.sources.get(source)
    }

    pub(crate) fn insert_associated_macro_expansion(
        &mut self,
        call: ItemSource,
        generated_items: Vec<ItemSource>,
    ) {
        self.associated_macro_expansions
            .insert(call, generated_items);
    }

    pub fn item(&self, item: GeneratedItemRef) -> Option<&ItemNode> {
        self.sources
            .get(item.source)
            .and_then(|source| source.item(item.item))
    }

    pub fn associated_macro_expansion(&self, call: ItemSource) -> Option<&[ItemSource]> {
        self.associated_macro_expansions
            .get(&call)
            .map(Vec::as_slice)
    }
}

/// The input-token origin of one occurrence recorded while lowering generated syntax.
///
/// This stores a token reference, not a byte range in either the generated text or its input file.
/// A saved source view resolves it through the input file's occurrence table. If that file moves
/// without changing its declaration structure, the token reference still yields the new position.
///
/// A copied macro argument can retain its own origin. Template syntax instead uses the invocation
/// as a navigation fallback and has `editable: false`. For example, expanding `make!(User)` to
/// `struct User; struct Generated;` gives `User` an input occurrence and `Generated` a fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct GeneratedLocation {
    pub origin: rg_tt::Span,
    pub editable: bool,
}

impl GeneratedLocation {
    pub fn input_file(self) -> (PackageSlot, FileId) {
        let anchor = self.origin.anchor;
        (
            PackageSlot(anchor.package as usize),
            FileId::from_raw(anchor.file_id.raw_file_id()),
        )
    }
}

/// An expansion's occurrences and the file tables needed to interpret them.
///
/// Several items can share this table, such as a macro producing a struct and its impl. The input
/// file list is collected once when the table is built. Opening a view for each generated item
/// then loads only those file tables instead of scanning every occurrence in the expansion again.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct GeneratedExpansionLocations {
    locations: DeclarationSourceStore<GeneratedLocation>,
    input_files: Vec<(PackageSlot, FileId)>,
}

impl GeneratedExpansionLocations {
    pub(crate) fn new(locations: DeclarationSourceStore<GeneratedLocation>) -> Self {
        // Many declarations share one expansion. Discover its input files once, so reading each
        // declaration's source does not revisit every occurrence in the expansion.
        let mut input_files = locations
            .entries()
            .map(|location| location.input_file())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        // Keep the persisted representation stable regardless of hash iteration order.
        input_files.sort_unstable();
        Self {
            locations,
            input_files,
        }
    }

    pub fn get(&self, occurrence: SourceId) -> Option<&GeneratedLocation> {
        self.locations.get(occurrence)
    }

    pub fn input_files(&self) -> &[(PackageSlot, FileId)] {
        &self.input_files
    }

    pub fn initializer_preview(&self, occurrence: SourceId) -> Option<&str> {
        self.locations.initializer_preview(occurrence)
    }
}

/// Choose whether generated declarations own occurrence mappings or reuse another item's IDs.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum GeneratedLocations {
    /// Lowered expansion syntax has its own occurrence IDs, each mapped back to input tokens.
    Expansion(GeneratedExpansionLocations),
    /// A builtin derive copies bounds from its input declaration. Those occurrences still
    /// belong to that input's source table; invented paths have no source occurrence.
    /// A reader follows this source without changing the copied IDs.
    Inherited(ItemSource),
}

/// Source mappings retained for a target after generated item payloads have been released.
///
/// [`GeneratedSourceId`] selects an expansion or inherited source here; [`SourceId`] selects an
/// occurrence inside that source. Keeping this store separate from declaration meaning lets a
/// source reader acquire provenance only when it needs physical positions.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct GeneratedSourceStore {
    sources: Vec<GeneratedLocations>,
}

impl GeneratedSourceStore {
    pub fn get(&self, source: GeneratedSourceId) -> Option<&GeneratedLocations> {
        self.sources.get(source.0)
    }
}
