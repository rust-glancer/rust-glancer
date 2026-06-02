//! Body-aware routing for shared DefMap and item-store queries.

use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyRef, DefMapRef, ModuleRef, TargetRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource};

use crate::ir::body::BodyData;

/// Routes semantic-shaped queries to target storage or to the active body storage.
///
/// Body resolution often needs DefMap lookup and item data together. Keeping both routes in one
/// source makes those algorithms use the same query objects as target-level analysis.
#[derive(Clone, Copy)]
pub(crate) struct BodyQuerySource<'a, D, I> {
    def_maps: D,
    item_stores: I,
    body_ref: BodyRef,
    body: &'a BodyData,
}

impl<'a, D, I> BodyQuerySource<'a, D, I> {
    pub(crate) fn new(def_maps: D, item_stores: I, body_ref: BodyRef, body: &'a BodyData) -> Self {
        Self {
            def_maps,
            item_stores,
            body_ref,
            body,
        }
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(crate) fn body(&self) -> &'a BodyData {
        self.body
    }
}

impl<D, I> DefMapSource for BodyQuerySource<'_, D, I>
where
    D: DefMapSource,
{
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Target(_) => self.def_maps.def_map_for_origin(origin),
            DefMapRef::Body(body_ref) if body_ref == self.body_ref => Ok(self.body.body_def_map()),
            DefMapRef::Body(_) => Ok(None),
        }
    }

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.extern_root(target, name)
    }

    fn prelude_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.prelude_module(target)
    }

    fn root_module(&self, target: TargetRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.root_module(target)
    }
}

impl<'a, D, I> ItemStoreSource<'a> for BodyQuerySource<'a, D, I>
where
    I: ItemStoreSource<'a, Error = PackageStoreError>,
{
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        match origin {
            DefMapRef::Target(_) => self.item_stores.item_store_for_origin(origin),
            DefMapRef::Body(body_ref) if body_ref == self.body_ref => {
                Ok(self.body.body_item_store())
            }
            DefMapRef::Body(_) => Ok(None),
        }
    }

    fn visible_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        self.item_stores.visible_stores()
    }
}
