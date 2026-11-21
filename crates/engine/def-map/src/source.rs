//! Source identities for declarations and the provenance retained from macro expansion.
//!
//! An item source tells readers which file, expansion, or body owns a declaration's occurrence
//! IDs. Generated declarations also need the connection back to input tokens: their parsed syntax
//! is temporary, and its offsets are not positions that an editor can navigate to or edit.

use std::{collections::HashSet, sync::Arc};

use rg_arena::Arena;
use rg_ir_model::{BodyRef, BodySource, CrateRef, FileId, ImplId, PackageSlot, SourceId, TraitId};
use rg_item_tree::{
    DeclarationSourceStore, ExpandedItemBodies, ItemNode, ItemTreeId, ItemTreeRef,
    file_declarations,
};
use rg_macro_runtime::{ExpansionParseKind, ExpansionSyntax};
use rg_source::QueryContext;
use rg_std::{MemorySize, Shrink};
use rg_syntax::{AstNode as _, SyntaxNode, TextRange, ast};
use wincode::{SchemaRead, SchemaWrite};

use crate::{ExpandedBodyMacro, body_macros::BodyMacroInputSources};

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

impl rg_arena::ArenaId for GeneratedSourceId {
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

/// Item-tree-shaped payload produced for one macro expansion or synthetic declaration.
///
/// DefMap uses this while collecting scopes, and Semantic IR copies the declaration facts it
/// needs. The item payload is then dropped. Locations and handles to body tokens move into
/// [`GeneratedSourceStore`].
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink)]
pub(crate) struct GeneratedSourceData {
    pub(crate) origin_file_id: FileId,
    pub(crate) locations: GeneratedLocations,
    pub(crate) bodies: Option<ExpandedItemBodies>,
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
}

impl GeneratedItemStore {
    pub(crate) fn new(id: GeneratedSourceStoreId) -> Self {
        Self {
            id,
            sources: Arena::new(),
        }
    }

    pub(crate) fn item_ref(&self, source: GeneratedSourceId, item: ItemTreeId) -> GeneratedItemRef {
        GeneratedItemRef {
            source_store: self.id,
            source,
            item,
        }
    }

    /// Keep provenance after Semantic IR has copied the generated declaration meaning. Preserve
    /// expansion slot order so existing [`GeneratedSourceId`] values still select the same source.
    pub fn into_sources(self, db: &QueryContext<'_>) -> ajika::Result<GeneratedSourceStore> {
        let mut sources = Vec::with_capacity(self.sources.len());
        let mut bodies = Vec::new();
        for (index, source) in self.sources.into_vec().into_iter().enumerate() {
            if let Some(data) = source.bodies {
                // A source lookup can open the locations without loading the expanded tokens.
                let output = db.output(("generated bodies", index), Arc::new(data))?;
                bodies.push((GeneratedSourceId(index), output));
            }
            sources.push(source.locations);
        }
        bodies.shrink_to_fit();
        Ok(GeneratedSourceStore { sources, bodies })
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
    bodies: Vec<(GeneratedSourceId, ajika::Output<ExpandedItemBodies>)>,
}

impl GeneratedSourceStore {
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
        GeneratedBodySyntax::open(db, krate, self.bodies[index].1.clone(), generated.item)
    }
}

/// The selected item and the expansion context needed to lower it.
///
/// `syntax` selects one function or initializer. `expansion` maps its tokens back to the input.
/// `source` gives the invocation range to use when a generated node has no exact written range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedBodySyntax {
    pub syntax: SyntaxNode,
    pub expansion: ExpandedBodyMacro<SyntaxNode>,
    pub source: BodySource,
}

impl GeneratedBodySyntax {
    /// A `cfg_select!` arm keeps its declarations in the file's item tree. The ordinary file
    /// parse sees the arm as tokens, so parse the saved arm tokens when preparing one of its bodies.
    pub fn for_file_item(
        db: &QueryContext<'_>,
        krate: CrateRef,
        item: ItemTreeRef,
    ) -> ajika::Result<Option<Self>> {
        let declarations = file_declarations(db, krate.package, item.file_id)?;
        let bodies = declarations.cfg_select_bodies.value_ref(db)?;
        let Some(output) = bodies.get(item.item) else {
            return Ok(None);
        };
        Self::open(db, krate, output.clone(), item.item)
    }

    fn open(
        db: &QueryContext<'_>,
        krate: CrateRef,
        output: ajika::Output<ExpandedItemBodies>,
        item: ItemTreeId,
    ) -> ajika::Result<Option<Self>> {
        let parsed = parsed_generated_bodies(db, krate, output)?;
        let Some((_, range)) = parsed.data.items.iter().find(|(id, _)| *id == item) else {
            return Ok(None);
        };
        // An invocation can generate several owners at the same physical range. These offsets
        // select the item in the reproduced expansion, where each owner has a distinct range.
        let range = TextRange::new(range.start.into(), range.end.into());
        let syntax = parsed
            .root
            .covering_element(range)
            .ancestors()
            .find(|node| node.text_range() == range && ast::Item::can_cast(node.kind()))
            .ok_or_else(|| ajika::Error::contract("generated body is absent from its expansion"))?;
        Ok(Some(Self {
            syntax,
            expansion: parsed.expansion.clone(),
            source: parsed.source,
        }))
    }
}

struct ParsedGeneratedBodies {
    root: SyntaxNode,
    data: Arc<ExpandedItemBodies>,
    source: BodySource,
    expansion: ExpandedBodyMacro<SyntaxNode>,
}

/// Reparse the packed tokens once per operation so sibling bodies can borrow the same tree.
/// The saved token IDs resolve through this snapshot's declaration source table. Moving the
/// invocation can shift every written range while leaving the packed tokens unchanged.
#[ajika::query(transient, returns(ref))]
fn parsed_generated_bodies(
    db: &QueryContext<'_>,
    krate: CrateRef,
    output: ajika::Output<ExpandedItemBodies>,
) -> ajika::Result<ParsedGeneratedBodies> {
    let data = output.value(db)?;
    let (parse, span_map) =
        ExpansionSyntax::parse_token_tree(&data.tokens, ExpansionParseKind::Items);
    let package = PackageSlot(data.call_site.anchor.package as usize);
    let file = FileId::from_raw(data.call_site.anchor.file_id.raw_file_id());
    let inputs = file_declarations(db, package, file)?.sources.value(db)?;
    let span = inputs
        .token_span(data.call_site)
        .ok_or_else(|| ajika::Error::contract("generated body invocation range is absent"))?;
    let root = parse.syntax_node();
    let expansion = ExpandedBodyMacro::new(
        root.clone(),
        span_map,
        BodyMacroInputSources::Declaration(inputs),
        data.call_site,
        package,
        data.dollar_crate.unwrap_or(krate),
    );
    Ok(ParsedGeneratedBodies {
        root,
        data,
        source: BodySource::macro_generated(file, span),
        expansion,
    })
}
