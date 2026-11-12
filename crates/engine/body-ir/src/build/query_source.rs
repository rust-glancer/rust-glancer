//! Build-time body-aware routing for shared DefMap and item-store queries.

use std::collections::HashMap;

use rg_arena::Arena;
use rg_def_map::{DefMap, DefMapSource, DefinitionReader};
use rg_ir_model::{BodyId, BodyRef, CrateRef, DefMapRef, LocalDefRef, ModuleRef, SemanticItemRef};
use rg_semantic_ir::{ItemReader, SemanticItemSource, SemanticItemView};
use rg_source::ReadError;
use rg_std::UniqueVec;

use crate::BodyLocalItems;

/// Provides crate semantic facts plus body-local facts collected while building that crate.
///
/// The frozen query path can already route arbitrary body origins through `BodyReader`. During
/// indexing we need the same shape before the bodies have been written back to storage, so this
/// source reads body-local stores from the crate build state instead.
#[derive(Clone, Copy)]
pub(super) struct BodyBuildQuerySource<'a, 'db> {
    def_map: &'a DefinitionReader<'db>,
    semantic_ir: &'a ItemReader<'db>,
    crate_ref: CrateRef,
    body_slots: &'a HashMap<BodyRef, BodyId>,
    body_local_items: &'a Arena<BodyId, Option<BodyLocalItems>>,
}

impl<'a, 'db> BodyBuildQuerySource<'a, 'db> {
    pub(super) fn new(
        def_map: &'a DefinitionReader<'db>,
        semantic_ir: &'a ItemReader<'db>,
        crate_ref: CrateRef,
        body_slots: &'a HashMap<BodyRef, BodyId>,
        body_local_items: &'a Arena<BodyId, Option<BodyLocalItems>>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            crate_ref,
            body_slots,
            body_local_items,
        }
    }

    fn body_local_items(self, body_ref: BodyRef) -> Option<&'a BodyLocalItems> {
        if body_ref.crate_ref != self.crate_ref {
            return None;
        }

        let slot = *self.body_slots.get(&body_ref)?;
        self.body_local_items.get(slot)?.as_ref()
    }
}

impl DefMapSource for BodyBuildQuerySource<'_, '_> {
    type Error = ReadError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, ReadError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => Ok((*self)
                .body_local_items(body_ref)
                .map(BodyLocalItems::def_map)),
        }
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, ReadError> {
        self.def_map.crate_is_proc_macro(crate_ref)
    }

    fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, ReadError> {
        self.def_map.target_pointer_width(crate_ref)
    }

    fn extern_root(&self, crate_ref: CrateRef, name: &str) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.extern_root(crate_ref, name)
    }

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, ReadError> {
        self.def_map.extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.prelude_module(crate_ref)
    }

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, ReadError> {
        self.def_map.item_lookup_dependencies(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.def_map.root_module(crate_ref)
    }
}

impl<'source, 'db> SemanticItemSource<'source> for &'source BodyBuildQuerySource<'_, 'db> {
    type Error = ReadError;

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'source>>, ReadError> {
        match item.origin() {
            DefMapRef::Crate(_) => self.semantic_ir.item(item),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .and_then(|items| items.semantic_item_view(item))),
        }
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        match definition.origin {
            DefMapRef::Crate(_) => self.semantic_ir.item_for_local_def(definition),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .and_then(|items| {
                    items
                        .item_for_local_def(definition.local_def)
                        .map(|item| item.semantic_ref(definition.origin))
                })),
        }
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'source>>, ReadError> {
        match origin {
            DefMapRef::Crate(_) => self.semantic_ir.all_items(origin),
            DefMapRef::Body(body) => Ok(self
                .body_local_items(body)
                .map(BodyLocalItems::item_store)
                .map(|items| items.semantic_items().collect())
                .unwrap_or_default()),
        }
    }
}
