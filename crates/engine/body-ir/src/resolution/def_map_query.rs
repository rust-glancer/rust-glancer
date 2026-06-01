//! Body-aware routing for DefMap storage.

use rg_def_map::{DefMap, DefMapReadTxn, DefMapSource};
use rg_ir_model::{BodyRef, DefMapRef, ModuleRef, TargetRef};
use rg_package_store::PackageStoreError;

use crate::ir::body::BodyData;

#[derive(Clone, Copy)]
pub(super) struct BodyDefMapSource<'a, 'db> {
    target_def_maps: &'a DefMapReadTxn<'db>,
    body_ref: BodyRef,
    body: &'a BodyData,
}

impl<'a, 'db> BodyDefMapSource<'a, 'db> {
    pub(super) fn new(
        target_def_maps: &'a DefMapReadTxn<'db>,
        body_ref: BodyRef,
        body: &'a BodyData,
    ) -> Self {
        Self {
            target_def_maps,
            body_ref,
            body,
        }
    }
}

impl DefMapSource for BodyDefMapSource<'_, '_> {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Target(target) => self.target_def_maps.def_map(target),
            DefMapRef::Body(body_ref) if body_ref == self.body_ref => Ok(self.body.body_def_map()),
            DefMapRef::Body(_) => Ok(None),
        }
    }

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.target_def_maps.extern_root(target, name)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.target_def_maps.prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.target_def_maps.root_module(target)
    }
}
