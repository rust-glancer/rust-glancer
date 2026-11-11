use rg_def_map::{CrateData, CrateDataSource, DefMap, DefMapSource};
use rg_ir_model::{BodyKey, BodyRef, CrateRef, DefMapRef, ModuleRef, SavedBodyOwner};
use rg_parse::ParseDb;
use rg_semantic_ir::{
    CrateItemSource, ItemStore, ItemStoreSource,
    testonly::{SemanticIrFixture, SemanticIrView},
};
use rg_source::ReadError;
use rg_std::UniqueVec;

use crate::{BodyReader, BodyView};

pub struct BodyIrFixture {
    semantic: SemanticIrFixture,
}

impl BodyIrFixture {
    pub fn build(spec: &str) -> Self {
        Self {
            semantic: SemanticIrFixture::build(spec),
        }
    }

    pub fn build_with_sysroot(spec: &str) -> Self {
        Self {
            semantic: SemanticIrFixture::build_with_sysroot(spec),
        }
    }

    pub fn build_with_fake_sysroot(spec: &str) -> Self {
        Self {
            semantic: SemanticIrFixture::build_with_fake_sysroot(spec),
        }
    }

    pub fn inspect<R>(&mut self, observe: impl FnOnce(BodyIrView<'_>) -> R) -> R {
        self.semantic.inspect(|semantic| {
            let bodies = BodyReader::new(semantic.db(), semantic.declarations());
            observe(BodyIrView { semantic, bodies })
        })
    }
}

pub struct BodyIrView<'db> {
    semantic: SemanticIrView<'db>,
    bodies: BodyReader<'db>,
}

impl<'db> BodyIrView<'db> {
    pub fn semantic(&self) -> SemanticIrView<'db> {
        self.semantic
    }

    pub fn parse_db(&self) -> &'db ParseDb {
        self.semantic.parse_db()
    }

    pub fn body_reader(&self) -> BodyReader<'db> {
        self.bodies.clone()
    }

    pub fn def_map(&self, target: CrateRef) -> Option<&'db DefMap> {
        self.semantic.def_map(target)
    }

    pub fn crate_items(&self, target: CrateRef) -> Option<&'db ItemStore> {
        self.semantic.crate_items(target)
    }

    /// Demand every owner in the selected target, including each owner's nested bodies.
    pub fn bodies(&self, target: CrateRef) -> Vec<(BodyRef, BodyView<'_>)> {
        self.bodies
            .bodies(target, None)
            .expect("fixture bodies should load")
    }

    pub fn body(&self, body: BodyRef) -> Option<BodyView<'_>> {
        self.bodies.body(body).expect("fixture body should load")
    }

    pub fn body_def_map(&self, body: BodyRef) -> Option<&DefMap> {
        self.bodies
            .body_def_map(body)
            .expect("fixture body definitions should load")
    }

    pub fn item_store(&self, origin: DefMapRef) -> Option<&ItemStore> {
        match origin {
            DefMapRef::Crate(target) => self.crate_items(target),
            DefMapRef::Body(body) => self
                .bodies
                .body_item_store(body)
                .expect("fixture body items should load"),
        }
    }

    /// Resolve a named outer owner in an explicitly selected target. Tests with overloaded names
    /// can select an item from the declarations and construct its BodyKey directly.
    pub fn body_key(&self, target: CrateRef, name: &str) -> BodyKey {
        let items = self
            .crate_items(target)
            .expect("fixture target should exist");
        let declarations = rg_semantic_ir::target_declarations(self.semantic.db(), target)
            .expect("fixture declarations should load");
        let owners = declarations
            .body_owners
            .value_ref(self.semantic.db())
            .expect("fixture body owners should load");
        let mut matching = owners.iter().filter(|owner| {
            let item_name = match owner.owner {
                SavedBodyOwner::Function(id) => items.function_data(id).map(|data| &data.name),
                SavedBodyOwner::Const(id) => items.const_data(id).map(|data| &data.name),
                SavedBodyOwner::Static(id) => items.static_data(id).map(|data| &data.name),
            };
            item_name.is_some_and(|candidate| candidate.as_str() == name)
        });
        let owner = matching
            .next()
            .expect("fixture body owner should exist")
            .owner;
        assert!(
            matching.next().is_none(),
            "fixture body owner name should be unique"
        );
        BodyKey {
            krate: target,
            owner,
        }
    }
}

impl DefMapSource for BodyIrView<'_> {
    type Error = ReadError;
    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, ReadError> {
        match origin {
            DefMapRef::Crate(target) => self
                .semantic
                .declarations()
                .crate_data(target)
                .map(|data| data.map(CrateData::def_map)),
            DefMapRef::Body(body) => self.bodies.body_def_map(body),
        }
    }
    fn crate_is_proc_macro(&self, target: CrateRef) -> Result<bool, ReadError> {
        self.semantic.definitions().crate_is_proc_macro(target)
    }
    fn target_pointer_width(&self, target: CrateRef) -> Result<Option<u32>, ReadError> {
        self.semantic.definitions().target_pointer_width(target)
    }
    fn extern_root(&self, target: CrateRef, name: &str) -> Result<Option<ModuleRef>, ReadError> {
        self.semantic.definitions().extern_root(target, name)
    }
    fn extern_roots(&self, target: CrateRef) -> Result<Vec<(String, ModuleRef)>, ReadError> {
        self.semantic.definitions().extern_roots(target)
    }
    fn prelude_module(&self, target: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.semantic.definitions().prelude_module(target)
    }
    fn root_module(&self, target: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        self.semantic.definitions().root_module(target)
    }
    fn item_lookup_dependencies(&self, target: CrateRef) -> Result<UniqueVec<CrateRef>, ReadError> {
        self.semantic.definitions().item_lookup_dependencies(target)
    }
}

impl<'a> ItemStoreSource<'a> for &'a BodyIrView<'_> {
    type Error = ReadError;
    fn item_store_for_origin(&self, origin: DefMapRef) -> Result<Option<&'a ItemStore>, ReadError> {
        match origin {
            DefMapRef::Crate(target) => self.semantic.declarations().items(target),
            DefMapRef::Body(body) => self.bodies.body_item_store(body),
        }
    }
}
