//! Build-time body-aware routing for shared DefMap and item-store queries.

use std::collections::HashMap;

use rg_arena::Arena;
use rg_def_map::{DefMap, DefMapSource, DefinitionReader};
use rg_ir_model::{BodyId, BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_semantic_ir::{ItemReader, ItemStore, ItemStoreSource};
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

impl<'source, 'db> ItemStoreSource<'source> for &'source BodyBuildQuerySource<'_, 'db> {
    type Error = ReadError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'source ItemStore>, Self::Error> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.semantic_ir.items(crate_ref),
            DefMapRef::Body(body_ref) => Ok(self
                .body_local_items(body_ref)
                .map(BodyLocalItems::item_store)),
        }
    }
}
