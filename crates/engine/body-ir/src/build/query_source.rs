//! Build-time body-aware routing for shared DefMap and item-store queries.

use rg_arena::Arena;
use rg_def_map::DefMapReadTxn;
use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyId, BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_semantic_ir::{ItemStore, ItemStoreSource};

use crate::BodyLocalItems;

/// Provides crate semantic facts plus body-local facts collected while building that crate.
///
/// The frozen query path can already route arbitrary body origins through `BodyIrReadTxn`. During
/// indexing we need the same shape before the bodies have been written back to storage, so this
/// source reads body-local stores from the crate build state instead.
#[derive(Clone, Copy)]
pub(super) struct BodyBuildQuerySource<'a, 'db> {
    def_map: &'a DefMapReadTxn<'db>,
    semantic_ir: &'a SemanticIrReadTxn<'db>,
    crate_ref: CrateRef,
    body_local_items: &'a Arena<BodyId, Option<BodyLocalItems>>,
}

impl<'a, 'db> BodyBuildQuerySource<'a, 'db> {
    pub(super) fn new(
        def_map: &'a DefMapReadTxn<'db>,
        semantic_ir: &'a SemanticIrReadTxn<'db>,
        crate_ref: CrateRef,
        body_local_items: &'a Arena<BodyId, Option<BodyLocalItems>>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            crate_ref,
            body_local_items,
        }
    }

    fn body_local_items(self, body_ref: BodyRef) -> Option<&'a BodyLocalItems> {
        if body_ref.crate_ref != self.crate_ref {
            return None;
        }

        self.body_local_items.get(body_ref.body)?.as_ref()
    }
}

impl DefMapSource for BodyBuildQuerySource<'_, '_> {
    type Error = PackageStoreError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.def_map.def_map(crate_ref),
            DefMapRef::Body(body_ref) => Ok((*self)
                .body_local_items(body_ref)
                .map(BodyLocalItems::def_map)),
        }
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, PackageStoreError> {
        self.def_map.crate_is_proc_macro(crate_ref)
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.extern_root(crate_ref, name)
    }

    fn extern_roots(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        self.def_map.extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.prelude_module(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.root_module(crate_ref)
    }
}

impl<'source, 'db> ItemStoreSource<'source> for &'source BodyBuildQuerySource<'_, 'db> {
    type Error = PackageStoreError;

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

    fn included_stores(&self) -> Result<Vec<&'source ItemStore>, Self::Error> {
        self.semantic_ir.included_stores()
    }
}
