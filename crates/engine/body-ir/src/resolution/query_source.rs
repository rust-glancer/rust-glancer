//! Body-aware routing for shared DefMap and item-store queries.

use rg_def_map::{DefMap, DefMapReadTxn, DefMapSource};
use rg_ir_model::{BodyRef, DefMapRef, ModuleRef, TargetRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource, SemanticIrReadTxn};

use crate::ir::body::BodyData;

/// Routes semantic-shaped queries to target storage or to the active body shadow storage.
///
/// Body resolution often needs DefMap lookup and item data together. Keeping both routes in one
/// source makes those algorithms use the same query objects as target-level analysis.
#[derive(Clone, Copy)]
pub(crate) struct BodyQuerySource<'a, 'db> {
    target_def_maps: &'a DefMapReadTxn<'db>,
    semantic_ir: &'a SemanticIrReadTxn<'db>,
    body_ref: BodyRef,
    body: &'a BodyData,
}

impl<'a, 'db> BodyQuerySource<'a, 'db> {
    pub(crate) fn new(
        target_def_maps: &'a DefMapReadTxn<'db>,
        semantic_ir: &'a SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        body: &'a BodyData,
    ) -> Self {
        Self {
            target_def_maps,
            semantic_ir,
            body_ref,
            body,
        }
    }
}

impl DefMapSource for BodyQuerySource<'_, '_> {
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

impl<'a> ItemStoreSource<'a> for BodyQuerySource<'a, '_> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        match origin {
            DefMapRef::Target(target) => self.semantic_ir.items(target),
            DefMapRef::Body(body_ref) if body_ref == self.body_ref => {
                Ok(self.body.body_item_store())
            }
            DefMapRef::Body(_) => Ok(None),
        }
    }

    fn visible_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        self.semantic_ir.included_stores()
    }
}
