//! Shared read handle for indexed-data views.

use rg_body_ir::BodyIrReadTxn;
use rg_def_map::{DefMap, DefMapReadTxn, DefMapSource};
use rg_ir_model::{DefMapRef, ModuleRef, TargetRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource, SemanticIrReadTxn};

/// Read-only database handle used by all indexed-data views.
///
/// The handle deliberately contains the concrete frozen storage transactions. That keeps views
/// easy to extract as one crate first; a trait facade can replace these fields later once the
/// method surface settles.
#[derive(Debug, Clone)]
pub struct IndexedViewDb<'db> {
    pub(crate) def_map: DefMapReadTxn<'db>,
    pub(crate) semantic_ir: SemanticIrReadTxn<'db>,
    pub(crate) body_ir: BodyIrReadTxn<'db>,
}

impl<'db> IndexedViewDb<'db> {
    pub fn new(
        def_map: DefMapReadTxn<'db>,
        semantic_ir: SemanticIrReadTxn<'db>,
        body_ir: BodyIrReadTxn<'db>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ir,
        }
    }
}

impl<'a, 'db> ItemStoreSource<'a> for &'a IndexedViewDb<'db> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, PackageStoreError> {
        match origin {
            DefMapRef::Target(target) => self.semantic_ir.items(target),
            DefMapRef::Body(body_ref) => Ok(self
                .body_ir
                .body_data(body_ref)?
                .and_then(|body| body.body_item_store())),
        }
    }

    fn visible_stores(&self) -> Result<Vec<&'a ItemStore>, PackageStoreError> {
        self.semantic_ir.included_stores()
    }
}

impl DefMapSource for &IndexedViewDb<'_> {
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, PackageStoreError> {
        match origin {
            DefMapRef::Target(target) => self.def_map.def_map(target),
            DefMapRef::Body(body_ref) => Ok(self
                .body_ir
                .body_data(body_ref)?
                .and_then(|body| body.body_def_map())),
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
