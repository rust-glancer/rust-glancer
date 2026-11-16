//! Adapts finalized lexical DefMaps and source items to semantic item-store lowering.
//!
//! This is intentionally an adapter around the generic semantic item-store lowerer: body source
//! items look like item-tree entries once the local DefMap has been finalized.

use std::collections::HashMap;

use anyhow::Context as _;
use rg_def_map::{DefMap, ItemSource, ItemSourceKind};
use rg_ir_model::{DefMapRef, ItemOwner};
use rg_item_tree::{ItemKind, ItemNode};
use rg_semantic_ir::{AssociatedItemStoreBuilder, ItemStoreLowerer, ItemStoreSourceReader};
use rg_std::CancellationToken;

use crate::{BodyLocalItems, BodySourceItems};

pub(crate) struct LocalItemStoreCollector<'source> {
    items: &'source BodySourceItems,
    def_map: DefMap,
}

impl<'source> LocalItemStoreCollector<'source> {
    pub fn new(items: &'source BodySourceItems, def_map: DefMap) -> Self {
        Self { items, def_map }
    }

    /// Keep the definitions, scope declarations, and member stores together as one body result.
    pub fn collect(self, cancellation: &CancellationToken) -> anyhow::Result<BodyLocalItems> {
        let reader = LocalItemStoreSourceReader {
            items: self.items,
            def_map: &self.def_map,
            cancellation,
        };
        let items = ItemStoreLowerer::new(&self.def_map, reader)
            .lower()
            .context("lower collected body source items")?;
        // Body lowering has already expanded these lists. Allocate each owner's members in a
        // separate store; the scope store must mean the same thing as it does for saved targets.
        let mut members = HashMap::new();
        let owners = items
            .traits_with_refs()
            .map(|(owner, data)| (ItemOwner::Trait(owner.id), data.source))
            .chain(
                items
                    .impls_with_refs()
                    .map(|(owner, data)| (ItemOwner::Impl(owner.id), data.source)),
            );
        for (owner, source) in owners {
            let ids = match &reader.item(source)?.kind {
                ItemKind::Trait(item) => &item.items,
                ItemKind::Impl(item) => &item.items,
                _ => unreachable!("member store came from a trait or impl"),
            };
            let mut builder = AssociatedItemStoreBuilder::default();
            for id in ids {
                let source = source.with_item(*id);
                builder.lower(source, owner, reader.item(source)?);
            }
            members.insert(owner.item_store(self.def_map.own_ref()).id, builder.build());
        }
        Ok(BodyLocalItems::new(self.def_map, items, members))
    }
}

// Adapts body-local source item storage to the generic semantic item-store lowerer.
#[derive(Clone, Copy)]
struct LocalItemStoreSourceReader<'source, 'operation> {
    cancellation: &'operation CancellationToken,
    items: &'source BodySourceItems,
    def_map: &'source DefMap,
}

impl<'source> ItemStoreSourceReader<'source> for LocalItemStoreSourceReader<'source, '_> {
    #[rg_std::cancelable("local signature item", token = self.cancellation)]
    fn item(&self, source: ItemSource) -> anyhow::Result<&'source ItemNode> {
        let (DefMapRef::Body(body_ref), ItemSourceKind::Body(source)) =
            (self.def_map.own_ref(), source.kind)
        else {
            anyhow::bail!("body item store source should point to body source item");
        };

        if source.body != body_ref {
            anyhow::bail!("body item store source should belong to this body");
        }

        self.items.item(source.item).with_context(|| {
            format!(
                "while attempting to fetch body source item {:?}",
                source.item
            )
        })
    }
}
