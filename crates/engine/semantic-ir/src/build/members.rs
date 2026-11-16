//! Build and read the members of one saved trait or impl without opening neighboring owners.
//!
//! Target indexing publishes the headers first. For `impl<T> Bag<T> { methods!(); }`, the header
//! already describes `Bag<T>` and its generics, but the functions emitted by `methods!()` belong
//! to a separate query. That query expands the member list and lowers its declarations only when
//! a reader asks for them. Body lowering and inference remain separate work after that.

use std::collections::HashMap;

use rg_def_map::{AssociatedItemInput, AssociatedItemOwner};
use rg_ir_model::ItemStoreId;
use rg_source::QueryContext;
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemStore;

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
