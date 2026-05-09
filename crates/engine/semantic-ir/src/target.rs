use rg_arena::Arena;
use rg_def_map::LocalDefId;

use crate::{ImplId, ItemId, ItemStore};

/// Semantic IR for one target root.
///
/// The target keeps two indexes back into DefMap collection results:
/// local defs map to semantic item ids, and local impls map to semantic impl ids. Those links let
/// later phases move from name resolution into semantic signatures without re-lowering source.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TargetIr {
    pub(crate) local_items: Arena<LocalDefId, Option<ItemId>>,
    pub(crate) local_impls: Vec<ImplId>,
    pub(crate) items: ItemStore,
}

impl TargetIr {
    pub(crate) fn new(local_def_count: usize) -> Self {
        Self {
            local_items: {
                let mut local_items = Arena::new();
                local_items.resize_with(local_def_count, || None);
                local_items
            },
            local_impls: Vec::new(),
            items: ItemStore::default(),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.local_items.shrink_to_fit();
        self.local_impls.shrink_to_fit();
        self.items.shrink_to_fit();
    }

    /// Returns the semantic item lowered from one DefMap local definition.
    pub fn item_for_local_def(&self, local_def: LocalDefId) -> Option<ItemId> {
        self.local_items.get(local_def).copied().flatten()
    }

    /// Returns semantic impl ids in the same order as target-local impl lowering.
    pub fn impls(&self) -> &[ImplId] {
        &self.local_impls
    }

    /// Returns target-local semantic item storage.
    pub fn items(&self) -> &ItemStore {
        &self.items
    }

    pub(crate) fn set_local_item(&mut self, local_def: LocalDefId, item: ItemId) {
        let slot = self
            .local_items
            .get_mut(local_def)
            .expect("local item slot should exist while building semantic IR");
        *slot = Some(item);
    }

    pub(crate) fn push_local_impl(&mut self, impl_id: ImplId) {
        self.local_impls.push(impl_id);
    }

    pub(crate) fn items_mut(&mut self) -> &mut ItemStore {
        &mut self.items
    }
}
