use std::{collections::HashMap, convert::Infallible, sync::Arc};

use rg_def_map::{DefMap, DefMapSource, testonly::DefMapDb};
use rg_ir_model::{
    BodyRef, BodyRoot, ConstOrStaticRef, CrateRef, DefMapRef, EvaluatedConstValue, FileId,
    ModuleRef,
};
use rg_parse::ParseDb;
use rg_semantic_ir::{
    ItemStore, ItemStoreSource,
    testonly::{SemanticIrDb, SemanticIrFixture},
};
use rg_source::ReadError;
use rg_std::UniqueVec;

use crate::{
    BodyLocalItems, BodyReader, BodyStore, BodyView, build::fixture, store::reader::BodySource,
};

#[derive(Debug)]
struct FixtureBodies(HashMap<CrateRef, BodyStore>);

impl BodySource for FixtureBodies {
    fn files(&self, krate: CrateRef) -> Result<UniqueVec<FileId>, ReadError> {
        Ok(self
            .0
            .get(&krate)
            .into_iter()
            .flat_map(|store| store.bodies().iter().map(|body| body.source().file_id))
            .collect())
    }

    fn bodies(
        &self,
        krate: CrateRef,
        file: Option<FileId>,
        masked: &dyn Fn(FileId) -> bool,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        Ok(self
            .0
            .get(&krate)
            .into_iter()
            .flat_map(BodyStore::body_views)
            .filter(|(_, body)| {
                file.is_none_or(|file| body.source().file_id == file)
                    && !masked(body.source().file_id)
            })
            .map(|(body, view)| (BodyRef::local(krate, body), view))
            .collect())
    }

    fn body(&self, body: BodyRef) -> Result<Option<(BodyView<'_>, &BodyLocalItems)>, ReadError> {
        let Some(store) = self
            .0
            .get(&body.crate_ref)
            .filter(|_| body.root == BodyRoot::Local)
        else {
            return Ok(None);
        };
        Ok(store.body(body.body).zip(store.body_local_items(body.body)))
    }

    fn initializer(&self, _: ConstOrStaticRef) -> Result<Option<&EvaluatedConstValue>, ReadError> {
        // Kernel fixtures attach evaluated values directly to their local item arenas.
        Ok(None)
    }
}

/// End-to-end fixture for tests that need body lowering and type propagation data.
///
/// Declare crate roots with headers such as `//- /src/lib.rs crate:demo`. The shared semantic
/// setup builds workspace metadata from those declarations; Rust files still live on disk.
pub struct BodyIrFixture {
    semantic_ir: SemanticIrFixture,
    bodies: Arc<FixtureBodies>,
}

impl BodyIrFixture {
    pub fn build(fixture: &str) -> Self {
        Self::build_from_semantic_ir(SemanticIrFixture::build(fixture))
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        Self::build_from_semantic_ir(SemanticIrFixture::build_with_sysroot(fixture))
    }

    pub fn build_with_fake_sysroot(fixture: &str) -> Self {
        Self::build_from_semantic_ir(SemanticIrFixture::build_with_fake_sysroot(fixture))
    }

    pub fn build_from_semantic_ir(mut semantic_ir: SemanticIrFixture) -> Self {
        let bodies =
            fixture::build_bodies(&mut semantic_ir).expect("fixture body kernels should complete");
        Self {
            semantic_ir,
            bodies: Arc::new(FixtureBodies(bodies)),
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

    pub fn bodies(&self, krate: CrateRef) -> Option<&BodyStore> {
        self.bodies.0.get(&krate)
    }

    pub fn body_reader(&self) -> BodyReader<'_> {
        BodyReader::from_source(self.bodies.clone())
    }

    pub fn def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.semantic_ir.def_map(crate_ref)
    }

    pub fn crate_items(&self, crate_ref: CrateRef) -> Option<&ItemStore> {
        self.semantic_ir.crate_items(crate_ref)
    }

    pub fn body(&self, body_ref: BodyRef) -> Option<BodyView<'_>> {
        self.bodies(body_ref.crate_ref)?.body(body_ref.body)
    }

    pub fn body_item_store(&self, body_ref: BodyRef) -> Option<&ItemStore> {
        self.bodies(body_ref.crate_ref)?
            .body_item_store(body_ref.body)
    }

    pub fn body_def_map(&self, body_ref: BodyRef) -> Option<&DefMap> {
        self.bodies(body_ref.crate_ref)?.body_def_map(body_ref.body)
    }

    pub fn item_store(&self, origin: DefMapRef) -> Option<&ItemStore> {
        match origin {
            DefMapRef::Crate(crate_ref) => self.crate_items(crate_ref),
            DefMapRef::Body(body_ref) => self.body_item_store(body_ref),
        }
    }
}

impl DefMapSource for BodyIrFixture {
    type Error = Infallible;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        Ok(match origin {
            DefMapRef::Crate(crate_ref) => self.def_map(crate_ref),
            DefMapRef::Body(body_ref) => self.body_def_map(body_ref),
        })
    }

    fn crate_is_proc_macro(&self, crate_ref: CrateRef) -> Result<bool, Self::Error> {
        Ok(self
            .def_map_db()
            .package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .is_some_and(rg_def_map::CrateData::is_proc_macro))
    }

    fn target_pointer_width(&self, crate_ref: CrateRef) -> Result<Option<u32>, Self::Error> {
        Ok(self
            .def_map_db()
            .package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .and_then(rg_def_map::CrateData::target_pointer_width))
    }

    fn extern_root(
        &self,
        crate_ref: CrateRef,
        name: &str,
    ) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_db()
            .package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn extern_roots(&self, crate_ref: CrateRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        Ok(self
            .def_map_db()
            .package(crate_ref.package)
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
            .package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .and_then(|data| data.prelude()))
    }

    fn root_module(&self, crate_ref: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .def_map_db()
            .package(crate_ref.package)
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
        Ok(self.item_store(origin))
    }
}
