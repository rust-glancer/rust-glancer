//! Body-aware routing for shared DefMap and item-store queries.

use rg_def_map::{DefMap, DefMapSource};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource};
use rg_std::UniqueVec;

use crate::{BodyData, BodyView, ir::BodyQueryView};

/// Body state available to one query context.
///
/// Structural build steps run before semantic sidecars exist. Semantic queries retain a narrow
/// view of the facts available in their phase without making the build pipeline allocate
/// placeholders for finalized facts.
#[derive(Clone, Copy)]
enum BodyQueryBody<'a> {
    Structural(&'a BodyData),
    Query(BodyQueryView<'a>),
}

impl<'a> BodyQueryBody<'a> {
    fn structure(self) -> &'a BodyData {
        match self {
            Self::Structural(body) => body,
            Self::Query(body) => body.structure(),
        }
    }

    fn query_view(self) -> BodyQueryView<'a> {
        match self {
            Self::Query(body) => body,
            Self::Structural(_) => {
                panic!("semantic body facts should exist before this query is used")
            }
        }
    }
}

/// Routes semantic-shaped queries while keeping the active body available for lexical lookup.
///
/// DefMap and item-store storage is owned by the provider. During indexing that provider reads the
/// build state; after indexing it reads frozen crate_ref body-local storage.
#[derive(Clone, Copy)]
pub(crate) struct BodyQuerySource<'a, D, I> {
    def_maps: D,
    item_stores: I,
    body_ref: BodyRef,
    body: BodyQueryBody<'a>,
}

impl<'a, D, I> BodyQuerySource<'a, D, I> {
    pub(crate) fn new(def_maps: D, item_stores: I, body_ref: BodyRef, body: BodyView<'a>) -> Self {
        Self {
            def_maps,
            item_stores,
            body_ref,
            body: BodyQueryBody::Query(body.query_view()),
        }
    }

    pub(crate) fn for_query(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: BodyQueryView<'a>,
    ) -> Self {
        Self {
            def_maps,
            item_stores,
            body_ref,
            body: BodyQueryBody::Query(body),
        }
    }

    pub(crate) fn for_structure(
        def_maps: D,
        item_stores: I,
        body_ref: BodyRef,
        body: &'a BodyData,
    ) -> Self {
        Self {
            def_maps,
            item_stores,
            body_ref,
            body: BodyQueryBody::Structural(body),
        }
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(crate) fn body(&self) -> &'a BodyData {
        self.body.structure()
    }

    pub(crate) fn query_body(&self) -> BodyQueryView<'a> {
        self.body.query_view()
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

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, PackageStoreError> {
        self.def_maps.crate_is_proc_macro(crate_ref)
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

    fn item_lookup_dependencies(
        &self,
        crate_ref: CrateRef,
    ) -> Result<UniqueVec<CrateRef>, PackageStoreError> {
        self.def_maps.item_lookup_dependencies(crate_ref)
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
