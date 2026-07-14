//! Body-aware routing for shared DefMap and item-store queries.

use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource};

use crate::ir::body::ResolvedBodyData;

/// Routes semantic-shaped queries while keeping the active body available for lexical lookup.
///
/// DefMap and item-store storage is owned by the provider. During indexing that provider reads the
/// build state; after indexing it reads frozen crate_ref body-local storage.
#[derive(Clone, Copy)]
pub(crate) struct BodyQuerySource<'a, D, I> {
    def_maps: D,
    item_stores: I,
    body_ref: BodyRef,
    body: &'a ResolvedBodyData,
}

impl<'a, D, I> BodyQuerySource<'a, D, I> {
    pub(crate) fn new(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: &'a ResolvedBodyData,
    ) -> Self {
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

    pub(crate) fn body(&self) -> &'a ResolvedBodyData {
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
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.extern_root(crate_ref, name)
    }

    fn extern_roots(
        &self,
        crate_ref: CrateRef,
    ) -> Result<Vec<(String, ModuleRef)>, PackageStoreError> {
        self.def_maps.extern_roots(crate_ref)
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.prelude_module(crate_ref)
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, PackageStoreError> {
        self.def_maps.root_module(crate_ref)
    }
}

impl<'a, D, I> ItemStoreSource<'a> for BodyQuerySource<'a, D, I>
where
    D: Clone,
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
