//! Retain generated item payloads during collection and publish their source data afterward.
//!
//! Semantic lowering consumes the generated declarations. Later source readers need only their
//! token origins and handles to the expansion tokens, which can be reopened for bodies or rename.

use std::{collections::HashSet, sync::Arc};

use rg_arena::Arena;
use rg_ir_model::{CrateRef, FileId, PackageSlot, SourceId};
use rg_item_tree::{DeclarationSourceStore, ItemNode, ItemTreeId, ItemTreeRef};
use rg_source::QueryContext;
use rg_std::{MemorySize, Shrink};
use rg_tt::Span as TtSpan;
use wincode::{SchemaRead, SchemaWrite};

use super::{
    GeneratedBodySyntax, GeneratedItemExpansion, GeneratedItemRef, GeneratedSourceId,
    GeneratedSourceStoreId, ItemMacroCopySource, ItemSource, expansion::SavedItemExpansion,
};

/// Item-tree-shaped payload produced for one macro expansion or synthetic declaration.
///
/// DefMap uses this while collecting scopes, and Semantic IR copies the declaration facts it
/// needs. The item payload is then dropped. Locations and handles to body tokens move into
/// [`GeneratedSourceStore`].
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub(crate) struct GeneratedSourceData {
    pub(crate) origin_file_id: FileId,
    pub(crate) locations: GeneratedLocations,
    pub(crate) expansion: Option<GeneratedItemExpansion>,
    pub(crate) origin_source: ItemTreeRef,
    pub(crate) top_level: Vec<ItemTreeId>,
    pub(crate) items: Arena<ItemTreeId, ItemNode>,
}

impl GeneratedSourceData {
    pub(crate) fn item(&self, item_id: ItemTreeId) -> Option<&ItemNode> {
        self.items.get(item_id)
    }
}

/// Generated declarations retained until their meaning has been copied into Semantic IR.
///
/// Macro expansion needs item-tree-shaped declarations while it discovers modules, definitions,
/// imports, and associated items. Those declarations are substantially larger than the resulting
/// semantic facts, so they stay in this temporary store. Target collection and each member query
/// have separate stores. A store constructs references with its own identity so callers cannot
/// accidentally label a source with the semantic owner of one of its declarations.
#[derive(Debug, Clone, MemorySize, Shrink)]
pub struct GeneratedItemStore {
    id: GeneratedSourceStoreId,
    sources: Arena<GeneratedSourceId, GeneratedSourceData>,
    #[memsize(skip)]
    #[shrink(skip)]
    unverified_calls: Vec<TtSpan>,
}

impl GeneratedItemStore {
    pub(crate) fn new(id: GeneratedSourceStoreId) -> Self {
        Self {
            id,
            sources: Arena::new(),
            unverified_calls: Vec::new(),
        }
    }

    pub(crate) fn item_ref(&self, source: GeneratedSourceId, item: ItemTreeId) -> GeneratedItemRef {
        GeneratedItemRef {
            source_store: self.id,
            source,
            item,
        }
    }

    /// Publish generated declaration provenance as outputs of the owning query.
    ///
    /// The locations table keeps the slots assigned during lowering. Body and copy readers use
    /// the same [`GeneratedSourceId`] and reopen the same saved output. Keep a copy source even
    /// for an expansion such as `use $path;`, which copies an argument and has no body.
    pub fn publish_sources(self, db: &QueryContext<'_>) -> ajika::Result<GeneratedSourceStore> {
        let mut sources = Vec::with_capacity(self.sources.len());
        let mut bodies = Vec::new();
        let mut expansions = Vec::new();
        for (index, source) in self.sources.into_vec().into_iter().enumerate() {
            if let Some(data) = source.expansion {
                let id = GeneratedSourceId(index);
                let has_bodies = !data.syntax.body_items.is_empty();
                let call_site = data.syntax.call_site;
                let is_root = data.forwarding.is_none();
                let output = db.output(("generated expansion", index), Arc::new(data))?;
                if has_bodies {
                    bodies.push((id, output.clone()));
                }
                expansions.push((
                    id,
                    ItemMacroCopySource {
                        call_site,
                        is_root,
                        expansion: output,
                    },
                ));
            }
            sources.push(source.locations);
        }
        bodies.shrink_to_fit();
        // Rename can find the invocations covering an edit before opening their expansion tokens.
        let copy_sources = if expansions.is_empty() && self.unverified_calls.is_empty() {
            None
        } else {
            Some(db.output(
                "item macro copy sources",
                Arc::new(ItemMacroCopySources {
                    expansions,
                    unverified_calls: self.unverified_calls,
                }),
            )?)
        };
        Ok(GeneratedSourceStore {
            sources,
            bodies,
            copy_sources,
        })
    }

    pub(crate) fn record_unverified_call(&mut self, call: TtSpan) {
        self.unverified_calls.push(call);
    }

    pub(crate) fn alloc_source(&mut self, source: GeneratedSourceData) -> GeneratedSourceId {
        self.sources.alloc(source)
    }

    pub(crate) fn source(&self, source: GeneratedSourceId) -> Option<&GeneratedSourceData> {
        self.sources.get(source)
    }

    pub fn item(&self, item: GeneratedItemRef) -> Option<&ItemNode> {
        // Expansion slots overlap between stores; a valid local index is not enough.
        if item.source_store != self.id {
            return None;
        }
        self.sources
            .get(item.source)
            .and_then(|source| source.item(item.item))
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
    pub origin: TtSpan,
    pub editable: bool,
    pub copy: Option<u32>,
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

/// Invocation sources used to find the expansions an edit can affect. An opaque invocation with
/// a known range stays visible here so rename can refuse an edit it cannot verify.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ItemMacroCopySources {
    expansions: Vec<(GeneratedSourceId, ItemMacroCopySource)>,
    unverified_calls: Vec<TtSpan>,
}

impl ItemMacroCopySources {
    pub fn expansions(&self) -> impl Iterator<Item = (GeneratedSourceId, &ItemMacroCopySource)> {
        self.expansions.iter().map(|(id, source)| (*id, source))
    }

    pub fn unverified_calls(&self) -> &[TtSpan] {
        &self.unverified_calls
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

/// Source mappings and body-token handles published by a target or member query.
///
/// [`GeneratedSourceId`] selects an expansion or inherited source here; [`SourceId`] selects an
/// occurrence inside that source. Keeping this store separate from declaration meaning lets a
/// source reader acquire provenance only when it needs physical positions. Body lowering opens
/// tokens through a separate output. Declaration lookups can read just the locations.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct GeneratedSourceStore {
    sources: Vec<GeneratedLocations>,
    #[memsize(skip)]
    #[shrink(skip)]
    bodies: Vec<(GeneratedSourceId, ajika::Output<GeneratedItemExpansion>)>,
    #[memsize(skip)]
    #[shrink(skip)]
    copy_sources: Option<ajika::Output<ItemMacroCopySources>>,
}

impl GeneratedSourceStore {
    pub fn macro_copy_sources<'db>(
        &self,
        db: &'db QueryContext<'db>,
    ) -> ajika::Result<Option<&'db ItemMacroCopySources>> {
        self.copy_sources
            .as_ref()
            .map(|sources| sources.value_ref(db))
            .transpose()
    }

    pub fn get(&self, source: GeneratedSourceId) -> Option<&GeneratedLocations> {
        self.sources.get(source.0)
    }

    pub fn body_syntax(
        &self,
        db: &QueryContext<'_>,
        krate: CrateRef,
        generated: GeneratedItemRef,
    ) -> ajika::Result<Option<GeneratedBodySyntax>> {
        let Ok(index) = self
            .bodies
            .binary_search_by_key(&generated.source.0, |(id, _)| id.0)
        else {
            return Ok(None);
        };
        GeneratedBodySyntax::open(
            db,
            krate,
            SavedItemExpansion::Macro(self.bodies[index].1.clone()),
            generated.item,
            Some(generated.expansion(krate)),
        )
    }
}
