//! Body-aware routing for shared DefMap and item-store queries.

use rg_ir_model::{BodyRef, DefMapRef, ModuleRef, TargetRef};
use rg_ir_storage::{DefMap, DefMapSource, ItemStore, ItemStoreSource};
use rg_package_store::PackageStoreError;

use crate::ir::body::BodyData;

/// Routes semantic-shaped queries while keeping the active body available for lexical lookup.
///
/// DefMap and item-store storage is owned by the provider. During indexing that provider reads the
/// build state; after indexing it reads frozen target body-local storage.
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
    D: DefMapSource<Error = PackageStoreError>,
{
    type Error = PackageStoreError;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        self.def_maps.def_map_for_origin(origin)
    }

    fn extern_root(
        &self,
        target: TargetRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.extern_root(target, name)
    }

    fn extern_roots(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        self.def_maps.extern_roots(target)
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
        self.item_stores.item_store_for_origin(origin)
    }

    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        self.item_stores.included_stores()
    }
}
