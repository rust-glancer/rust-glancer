use std::convert::Infallible;

use rg_def_map::{DefMap, DefMapDb, DefMapSource};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ModuleRef};
use rg_parse::ParseDb;
use rg_semantic_ir::{ItemStore, ItemStoreSource, SemanticIrDb, testonly::SemanticIrFixture};
use rg_text::PackageNameInterners;

use crate::{BodyIrBuildPolicy, BodyIrDb, BodyView};

/// End-to-end fixture for tests that need body lowering and type propagation data.
pub struct BodyIrFixture {
    semantic_ir: SemanticIrFixture,
    body_ir: BodyIrDb,
}

impl BodyIrFixture {
    pub fn build(fixture: &str) -> Self {
        Self::build_with_policy(fixture, BodyIrBuildPolicy::default())
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        Self::build_from_semantic_ir(SemanticIrFixture::build_with_sysroot(fixture))
    }

    pub fn build_with_fake_sysroot(fixture: &str) -> Self {
        Self::build_from_semantic_ir(SemanticIrFixture::build_with_fake_sysroot(fixture))
    }

    pub fn build_with_policy(fixture: &str, policy: BodyIrBuildPolicy) -> Self {
        Self::build_from_semantic_ir_with_policy(SemanticIrFixture::build(fixture), policy)
    }

    pub fn build_from_semantic_ir(semantic_ir: SemanticIrFixture) -> Self {
        Self::build_from_semantic_ir_with_policy(semantic_ir, BodyIrBuildPolicy::default())
    }

    pub fn build_from_semantic_ir_with_policy(
        semantic_ir: SemanticIrFixture,
        policy: BodyIrBuildPolicy,
    ) -> Self {
        let mut names = PackageNameInterners::new(semantic_ir.parse_db().package_count());
        let body_ir = BodyIrDb::builder(
            semantic_ir.parse_db(),
            semantic_ir.def_map_db(),
            semantic_ir.semantic_ir_db(),
        )
        .name_interners(&mut names)
        .policy(policy)
        .build()
        .expect("fixture body ir db should build");

        Self {
            semantic_ir,
            body_ir,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        self.semantic_ir.parse_db()
    }

    pub fn def_map_db(&self) -> &DefMapDb {
        self.semantic_ir.def_map_db()
    }

    pub fn semantic_ir_db(&self) -> &SemanticIrDb {
        self.semantic_ir.semantic_ir_db()
    }

    pub fn body_ir_db(&self) -> &BodyIrDb {
        &self.body_ir
    }

    pub fn resident_def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.semantic_ir.resident_def_map(crate_ref)
    }

    pub fn resident_crate_ir(&self, crate_ref: CrateRef) -> Option<&ItemStore> {
        self.semantic_ir.resident_crate_ir(crate_ref)
    }

    pub fn resident_body(&self, body_ref: BodyRef) -> Option<BodyView<'_>> {
        self.body_ir
            .resident_package(body_ref.crate_ref.package)?
            .crate_bodies(body_ref.crate_ref.crate_id)?
            .body(body_ref.body)
    }

    pub fn resident_body_item_store(&self, body_ref: BodyRef) -> Option<&ItemStore> {
        self.body_ir
            .resident_package(body_ref.crate_ref.package)?
            .crate_bodies(body_ref.crate_ref.crate_id)?
            .body_item_store(body_ref.body)
    }

    pub fn resident_body_def_map(&self, body_ref: BodyRef) -> Option<&DefMap> {
        self.body_ir
            .resident_package(body_ref.crate_ref.package)?
            .crate_bodies(body_ref.crate_ref.crate_id)?
            .body_def_map(body_ref.body)
    }

    pub fn resident_item_store(&self, origin: DefMapRef) -> Option<&ItemStore> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.resident_crate_ir(crate_ref),
            DefMapRef::Body(body_ref) => self.resident_body_item_store(body_ref),
        }
    }
}

impl DefMapSource for BodyIrFixture {
    type Error = Infallible;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        Ok(match origin {
            DefMapRef::Crate(crate_ref) => self.resident_def_map(crate_ref),
            DefMapRef::Body(body_ref) => self.resident_body_def_map(body_ref),
        })
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, Self::Error> {
        Ok(self
            .def_map_db()
            .resident_package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .is_some_and(rg_def_map::CrateData::is_proc_macro))
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_db()
            .resident_package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        Ok(self
            .def_map_db()
            .resident_package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .map(|data| {
                data.extern_prelude()
                    .iter()
                    .map(|(name, module)| (name.to_string(), *module))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn prelude_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_db()
            .resident_package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .and_then(|data| data.prelude()))
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_db()
            .resident_package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .and_then(|data| {
                Some(ModuleRef {
                    origin: DefMapRef::Crate(crate_ref),
                    module: data.root_module()?,
                })
            }))
    }
}

impl<'a> ItemStoreSource<'a> for &'a BodyIrFixture {
    type Error = Infallible;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        Ok(self.resident_item_store(origin))
    }

    fn included_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        Ok((0..self.semantic_ir_db().package_count())
            .filter_map(|index| {
                self.semantic_ir_db()
                    .resident_package(rg_def_map::PackageSlot(index))
            })
            .flat_map(|package| package.crates())
            .collect())
    }
}
