//! Build and read the members of one trait or impl without opening neighboring owners.
//!
//! Target indexing publishes the headers first. For `impl<T> Bag<T> { methods!(); }`, the header
//! already describes `Bag<T>` and its generics, but the functions emitted by `methods!()` belong
//! to a separate query. That query expands the member list and lowers its declarations when asked.
//! Project preparation asks for owners with written lang items; other owners wait for a reader.
//! Body lowering and inference remain separate work after that.

use std::collections::HashMap;

use rg_def_map::{
    AssociatedItemInput, AssociatedItemOwner, CrateDefMapReader, DefMapQuery, GeneratedSourceStore,
    GeneratedSourceStoreId,
};
use rg_ir_model::{BodyKey, CrateRef, DefMapRef, FileId, ItemOwner, ItemStoreId, ItemStoreRef};
use rg_item_tree::SourceLocations as _;
use rg_source::{QueryContext, ReadError};
use rg_std::Shrink as _;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    AssociatedItemStore, AssociatedItemStoreBuilder, BodyOwnerData, DeclarationSourceResolver,
    ItemSourceView, ItemStore, LangItemIndex, SourceSelection, TargetDeclarationReader,
    target_declarations,
};

const OWNERS_PER_PAGE: usize = 64;
type InputPage = Vec<AssociatedItemInput>;

/// Saved inputs for rebuilding member lists, grouped into pages to avoid a tiny payload per owner.
///
/// A request may decode inputs for neighboring owners in the same page, but it expands and lowers
/// only the selected owner. The resulting member store is cached independently of that page.
/// Slots follow the trait and impl header arenas. Every header has an input, including an explicit
/// empty input for compiler-provided impls. Source identity is never used to pair these records.
#[derive(PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct AssociatedItemInputs {
    traits: Vec<ajika::Output<InputPage>>,
    impls: Vec<ajika::Output<InputPage>>,
}

impl AssociatedItemInputs {
    pub(super) fn publish(
        db: &QueryContext<'_>,
        items: &ItemStore,
        mut inputs: HashMap<AssociatedItemOwner, AssociatedItemInput>,
    ) -> ajika::Result<Self> {
        let traits = items
            .traits()
            .iter()
            .map(|data| {
                inputs
                    .remove(&AssociatedItemOwner::Trait(data.local_def.local_def))
                    .ok_or_else(|| ajika::Error::contract("trait has no collected member input"))
            })
            .collect::<ajika::Result<Vec<_>>>()?;
        let impls = items
            .impls()
            .iter()
            .map(|data| {
                inputs
                    .remove(&AssociatedItemOwner::Impl(data.local_impl.local_impl))
                    .ok_or_else(|| ajika::Error::contract("impl has no collected member input"))
            })
            .collect::<ajika::Result<Vec<_>>>()?;
        if !inputs.is_empty() {
            return Err(ajika::Error::contract(
                "collected member input has no semantic owner",
            ));
        }
        let publish = |kind, inputs: Vec<_>| {
            inputs
                .chunks(OWNERS_PER_PAGE)
                .enumerate()
                .map(|(page, input)| db.output((kind, page as u32), input.to_vec().into()))
                .collect::<ajika::Result<Vec<_>>>()
        };
        Ok(Self {
            traits: publish("trait-input", traits)?,
            impls: publish("impl-input", impls)?,
        })
    }

    fn get<'db>(
        &self,
        db: &'db QueryContext<'db>,
        store: ItemStoreId,
    ) -> ajika::Result<&'db AssociatedItemInput> {
        let (pages, index) = match store {
            ItemStoreId::Trait(id) => (&self.traits, id.0),
            ItemStoreId::Impl(id) => (&self.impls, id.0),
            ItemStoreId::Scope => return Err(ajika::Error::contract("scope has no member input")),
        };
        let page = pages
            .get(index / OWNERS_PER_PAGE)
            .ok_or_else(|| ajika::Error::contract("member input page is missing"))?;
        page.value_ref(db)?
            .get(index % OWNERS_PER_PAGE)
            .ok_or_else(|| ajika::Error::contract("member input is missing"))
    }
}

/// Independently readable results from expanding and lowering one trait or impl's members.
///
/// A semantic reader needs `items`; navigation also needs the generated occurrence maps in
/// `sources`. `body_owners` identifies functions and initializers without lowering their bodies.
/// Keeping these outputs separate avoids loading source maps just to inspect a signature.
#[ajika::output]
pub struct AssociatedDeclarations {
    items: AssociatedItemStore,
    /// Final identities can be read without acquiring signatures or source maps.
    lang_items: LangItemIndex,
    sources: GeneratedSourceStore,
    /// The owner's file and readable include files reached while expanding its members.
    /// These files may be absent from the module file manifest produced at startup.
    files: Vec<FileId>,
    body_owners: Vec<BodyOwnerData>,
}

/// Open one member store using the trait or impl ID from its header.
///
/// The target query supplies replay inputs and frozen macro scopes, never a prebuilt member list.
/// Expansion finishes before lowering allocates any member IDs. A direct member or body reference
/// already identifies this owner, so it does not need to enumerate other traits or impls.
/// The key names the owner, not its source revision. Ajika validates the input reads and compares
/// the separate outputs, allowing unchanged member meaning to survive a source-only edit.
#[ajika::query(returns(ref))]
pub fn associated_declarations(
    db: &QueryContext<'_>,
    krate: CrateRef,
    owner: ItemStoreId,
) -> ajika::Result<Option<AssociatedDeclarations>> {
    let target = target_declarations(db, krate)?;
    let headers = target.items.value_ref(db)?;
    let (item_owner, source_store, module) = match owner {
        ItemStoreId::Trait(id) => {
            let Some(header) = headers.trait_data(db, id)? else {
                return Ok(None);
            };
            (
                ItemOwner::Trait(id),
                GeneratedSourceStoreId::TraitMembers(id),
                header.owner,
            )
        }
        ItemStoreId::Impl(id) => {
            let Some(header) = headers.impl_data(db, id)? else {
                return Ok(None);
            };
            (
                ItemOwner::Impl(id),
                GeneratedSourceStoreId::ImplMembers(id),
                header.owner,
            )
        }
        ItemStoreId::Scope => return Ok(None),
    };
    let input = target.member_inputs.value_ref(db)?.get(db, owner)?;
    let store = ItemStoreRef {
        origin: DefMapRef::Crate(krate),
        id: owner,
    };
    let mut builder = AssociatedItemStoreBuilder::default();
    let mut lang_items = LangItemIndex::default();
    let (mut sources, files) = {
        let reader = TargetDeclarationReader::for_target(db, krate)?;
        let env = DefMapQuery::new(CrateDefMapReader(&reader));
        let expanded = input
            .expand(db, krate, source_store, &target.macro_scopes, &env)
            .map_err(|error| {
                ReadError::query_cause(error.as_ref())
                    .unwrap_or_else(|| ajika::Error::application(error.into_boxed_dyn_error()))
            })?;
        for (source, item) in expanded.items {
            if let Some(id) = builder.lower(source, item_owner, &item)
                && let Some(lang_item) = item.lang_item
            {
                lang_items.insert(lang_item, id.semantic_ref(store));
            }
        }
        (expanded.sources, expanded.files)
    };
    let mut items = builder.build();
    items.shrink_to_fit();
    lang_items.shrink_to_fit();
    sources.shrink_to_fit();
    let bodies: Vec<_> = BodyOwnerData::for_members(module, &items, store).collect();
    AssociatedDeclarations::new(db, items, lang_items, sources, files, bodies).map(Some)
}

/// Select body owners before preparing or inferring them. Header bounds first narrow trait/impl
/// stores; declaration bounds then select individual functions and initializers within them.
pub fn body_owners(
    db: &QueryContext<'_>,
    krate: CrateRef,
    selection: SourceSelection,
) -> ajika::Result<Vec<BodyOwnerData>> {
    let target = target_declarations(db, krate)?;
    let in_selected_file =
        |owner: &BodyOwnerData| selection.file().is_none_or(|file| file == owner.file);
    let mut owners: Vec<_> = target
        .body_owners
        .value_ref(db)?
        .iter()
        .copied()
        .filter(in_selected_file)
        .collect();
    for store in DeclarationSourceResolver::new(db, krate).member_stores(selection)? {
        let Some(members) = associated_declarations(db, krate, store.id)? else {
            continue;
        };
        owners.extend(
            members
                .body_owners
                .value_ref(db)?
                .iter()
                .copied()
                .filter(in_selected_file),
        );
    }
    let Some(bounds) = selection.bounds() else {
        return Ok(owners);
    };
    let mut selected = Vec::new();
    for owner in owners {
        let sources = ItemSourceView::new(db, krate, owner.source)?;
        if sources
            .location(owner.location)
            .is_some_and(|location| location.span.touches_span(bounds))
        {
            selected.push(owner);
        }
    }
    Ok(selected)
}

/// Direct body identity already contains its member store; do not enumerate neighboring owners.
pub fn body_owner<'db>(
    db: &'db QueryContext<'db>,
    key: BodyKey,
) -> ajika::Result<Option<&'db BodyOwnerData>> {
    let owners = match key.owner.store() {
        ItemStoreId::Scope => target_declarations(db, key.krate)?
            .body_owners
            .value_ref(db)?,
        owner => {
            let Some(declarations) = associated_declarations(db, key.krate, owner)? else {
                return Ok(None);
            };
            declarations.body_owners.value_ref(db)?
        }
    };
    Ok(owners.iter().find(|owner| owner.owner == key.owner))
}
