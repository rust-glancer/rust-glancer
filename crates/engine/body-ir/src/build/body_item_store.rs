//! Lowers finalized body-local DefMaps into semantic-shaped item stores.
//!
//! This is intentionally an adapter around the generic semantic item-store lowerer: body source
//! items look like item-tree entries once the local DefMap has been finalized.

use anyhow::Context as _;
use rg_ir_model::{
    DefMapRef,
    hir::source::{ItemSource, ItemSourceKind},
};
use rg_ir_storage::{DefMap, ItemStore};
use rg_item_tree::ItemNode;
use rg_semantic_ir::{ItemStoreLowerer, ItemStoreSourceReader};

use crate::BodyData;

pub(crate) struct BodyItemStoreCollector<'body> {
    body: &'body BodyData,
    def_map: &'body DefMap,
}

impl<'body> BodyItemStoreCollector<'body> {
    pub fn new(body: &'body BodyData, def_map: &'body DefMap) -> Self {
        Self { body, def_map }
    }

    /// Lowers body-local DefMap entries into semantic item-shaped shadow storage.
    pub fn collect(self) -> ItemStore {
        let reader = BodyItemStoreSourceReader {
            body: self.body,
            def_map: self.def_map,
        };
        ItemStoreLowerer::new(self.def_map, reader)
            .lower()
            .expect("body item store should lower from collected body source items")
    }
}

// Adapts body-local source item storage to the generic semantic item-store lowerer.
struct BodyItemStoreSourceReader<'body> {
    body: &'body BodyData,
    def_map: &'body DefMap,
}

impl<'body> ItemStoreSourceReader<'body> for BodyItemStoreSourceReader<'body> {
    fn item(&self, source: ItemSource) -> anyhow::Result<&'body ItemNode> {
        let (DefMapRef::Body(body_ref), ItemSourceKind::Body(source)) =
            (self.def_map.own_ref(), source.kind)
        else {
            anyhow::bail!("body item store source should point to body source item");
        };

        if source.body != body_ref {
            anyhow::bail!("body item store source should belong to this body");
        }

        self.body.source_item(source.item).with_context(|| {
            format!(
                "while attempting to fetch body source item {:?}",
                source.item
            )
        })
    }
}
