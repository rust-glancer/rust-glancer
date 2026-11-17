use rg_def_map::{CrateData, CrateDataSource, DefMap, DefMapSource};
use rg_ir_model::{
    BodyKey, BodyRef, CrateRef, DefMapRef, ItemId, LocalDefRef, ModuleRef, SavedBodyOwner,
    SemanticItemRef,
};
use rg_parse::ParseDb;
use rg_semantic_ir::{
    ItemStoreQuery, SemanticItemSource, SemanticItemView,
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

    pub fn item_query(&self) -> ItemStoreQuery<'_, &Self> {
        ItemStoreQuery::new(self)
    }

    /// Resolve a named outer owner in an explicitly selected target. Tests with overloaded names
    /// can select an item from the declarations and construct its BodyKey directly.
    pub fn body_key(&self, target: CrateRef, name: &str) -> BodyKey {
        let items = self.semantic.all_items(DefMapRef::Crate(target));
        let declarations = rg_semantic_ir::target_declarations(self.semantic.db(), target)
            .expect("fixture declarations should load");
        let owners = declarations
            .body_owners
            .value_ref(self.semantic.db())
            .expect("fixture body owners should load");
        let mut matching = owners.iter().filter(|owner| {
            let item_ref = match owner.owner {
                SavedBodyOwner::Function(id) => ItemId::Function(id),
                SavedBodyOwner::Const(id) => ItemId::Const(id),
                SavedBodyOwner::Static(id) => ItemId::Static(id),
            }
            .semantic_ref(DefMapRef::Crate(target));
            items.iter().any(|item| {
                item.item() == item_ref
                    && item
                        .name()
                        .is_some_and(|candidate| candidate.as_str() == name)
            })
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

impl<'a> SemanticItemSource<'a> for &'a BodyIrView<'_> {
    type Error = ReadError;
    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        match item.origin() {
            DefMapRef::Crate(_) => self.semantic.items().item(item),
            DefMapRef::Body(_) => (&self.bodies).item(item),
        }
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        match definition.origin {
            DefMapRef::Crate(_) => self.semantic.items().item_for_local_def(definition),
            DefMapRef::Body(_) => (&self.bodies).item_for_local_def(definition),
        }
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        match origin {
            DefMapRef::Crate(_) => self.semantic.items().all_items(origin),
            DefMapRef::Body(_) => (&self.bodies).all_items(origin),
        }
    }
}
