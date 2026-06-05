//! Build-time body-aware routing for shared DefMap and item-store queries.

use rg_def_map::DefMapReadTxn;
use rg_ir_model::{BodyRef, DefMapRef, ModuleRef, TargetRef};
use rg_ir_storage::{DefMap, DefMapSource, ItemStore, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;

use super::state::BodyLocalItems;

/// Provides target semantic facts plus body-local facts collected during this target build.
///
/// The frozen query path can already route arbitrary body origins through `BodyIrReadTxn`. During
/// indexing we need the same shape before the bodies have been written back to storage, so this
/// source reads body-local stores from the target build state instead.
#[derive(Clone, Copy)]
pub(super) struct BodyBuildQuerySource<'a, 'db> {
    def_map: &'a DefMapReadTxn<'db>,
    semantic_ir: &'a SemanticIrReadTxn<'db>,
    target: TargetRef,
    body_local_items: &'a [Option<BodyLocalItems>],
}

impl<'a, 'db> BodyBuildQuerySource<'a, 'db> {
    pub(super) fn new(
        def_map: &'a DefMapReadTxn<'db>,
        semantic_ir: &'a SemanticIrReadTxn<'db>,
        target: TargetRef,
        body_local_items: &'a [Option<BodyLocalItems>],
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            target,
            body_local_items,
        }
    }

    fn body_local_items(self, body_ref: BodyRef) -> Option<&'a BodyLocalItems> {
        if body_ref.target != self.target {
            return None;
        }

        self.body_local_items.get(body_ref.body.0)?.as_ref()
    }
}

impl DefMapSource for BodyBuildQuerySource<'_, '_> {
    type Error = PackageStoreError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Target(target) => self.def_map.def_map(target),
            DefMapRef::Body(body_ref) => Ok((*self)
                .body_local_items(body_ref)
                .map(|items| &items.def_map)),
        }
    }

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.extern_root(target, name)
    }

    fn extern_roots(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        self.def_map.extern_roots(target)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_map.root_module(target)
    }
}

impl<'source, 'db> ItemStoreSource<'source> for &'source BodyBuildQuerySource<'_, 'db> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'source ItemStore>, Self::Error> {
        match origin {
            DefMapRef::Target(target) => self.semantic_ir.items(target),
            DefMapRef::Body(body_ref) => Ok(self
                .body_local_items(body_ref)
                .map(|items| &items.item_store)),
        }
    }

    fn included_stores(&self) -> Result<Vec<&'source ItemStore>, Self::Error> {
        self.semantic_ir.included_stores()
    }
}
