use rg_body_ir::{ResolvedBodyData, testonly::BodyIrFixture};
use rg_def_map::DefMapDb;
use rg_ir_model::{
    BodyId, BodyOwner, BodyRef, BodySource, DefMapRef, ExprData, ExprId, FunctionRef, ItemOwner,
    ModuleRef, TargetRef, TraitRef, TypeDefId, TypeDefRef,
};
use rg_ir_storage::{DefMap, ItemStore};
use rg_package_store::PackageLoader;
use rg_parse::ParseDb;
use rg_semantic_ir::{SemanticIrDb, testonly::SemanticIrFixture};

use crate::IndexedViewDb;

/// End-to-end fixture for tests that exercise view-level projections.
///
/// The lower IR fixtures still own construction of the indexed stores. This facade keeps tests
/// above `ir-view` from also knowing how to assemble read transactions for those stores.
pub struct ViewFixture {
    body_ir: BodyIrFixture,
}

impl ViewFixture {
    pub fn build(fixture: &str) -> Self {
        Self {
            body_ir: BodyIrFixture::build(fixture),
        }
    }

    pub fn build_from_semantic_ir(semantic_ir: SemanticIrFixture) -> Self {
        Self {
            body_ir: BodyIrFixture::build_from_semantic_ir(semantic_ir),
        }
    }

    pub fn view_db(&self) -> IndexedViewDb<'_> {
        IndexedViewDb::new(
            self.body_ir
                .def_map_db()
                .read_txn(PackageLoader::resident_only("resident view fixture")),
            self.body_ir
                .semantic_ir_db()
                .read_txn(PackageLoader::resident_only("resident view fixture")),
            self.body_ir
                .body_ir_db()
                .read_txn(PackageLoader::resident_only("resident view fixture")),
        )
    }

    pub fn parse_db(&self) -> &ParseDb {
        self.body_ir.parse_db()
    }

    pub fn def_map_db(&self) -> &DefMapDb {
        self.body_ir.def_map_db()
    }

    pub fn semantic_ir_db(&self) -> &SemanticIrDb {
        self.body_ir.semantic_ir_db()
    }

    pub fn resident_def_map(&self, target: TargetRef) -> Option<&DefMap> {
        self.body_ir.resident_def_map(target)
    }

    pub fn resident_target_ir(&self, target: TargetRef) -> Option<&ItemStore> {
        self.body_ir.resident_target_ir(target)
    }

    pub fn resident_body(&self, body_ref: BodyRef) -> Option<&ResolvedBodyData> {
        self.body_ir.resident_body(body_ref)
    }

    pub fn resident_body_source(&self, body_ref: BodyRef) -> Option<BodySource> {
        self.resident_body(body_ref).map(|body| body.source())
    }

    pub fn resident_expr(&self, body_ref: BodyRef, expr: ExprId) -> Option<&ExprData> {
        self.resident_body(body_ref)?.expr(expr)
    }

    pub fn resident_body_owner(&self, body_ref: BodyRef) -> Option<BodyOwner> {
        self.resident_body(body_ref).map(ResolvedBodyData::owner)
    }

    pub fn resident_body_item_store(&self, body_ref: BodyRef) -> Option<&ItemStore> {
        self.body_ir.resident_body_item_store(body_ref)
    }

    pub fn first_body_ref(&self, target: TargetRef) -> Option<BodyRef> {
        self.body_refs_for_target(target).into_iter().next()
    }

    pub fn body_refs_for_target(&self, target: TargetRef) -> Vec<BodyRef> {
        let Some(package) = self.body_ir.body_ir_db().resident_package(target.package) else {
            return Vec::new();
        };
        let Some(target_bodies) = package.target(target.target) else {
            return Vec::new();
        };

        target_bodies
            .bodies()
            .iter()
            .enumerate()
            .map(|(idx, _)| BodyRef {
                target,
                body: BodyId(idx),
            })
            .collect()
    }

    pub fn target_owns_file(&self, target: TargetRef, file_id: rg_parse::FileId) -> bool {
        self.resident_def_map(target).is_some_and(|def_map| {
            def_map
                .modules()
                .iter()
                .any(|module| module.origin.contains_file(file_id))
        })
    }

    pub fn render_type_def_ref(&self, ty: TypeDefRef) -> String {
        let items = self
            .body_ir
            .resident_item_store(ty.origin)
            .expect("type item store should exist while rendering view fixture type");

        match ty.id {
            TypeDefId::Struct(id) => {
                let data = items
                    .struct_data(id)
                    .expect("struct id should exist while rendering view fixture type");
                format!(
                    "struct {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            TypeDefId::Enum(id) => {
                let data = items
                    .enum_data(id)
                    .expect("enum id should exist while rendering view fixture type");
                format!("enum {}::{}", self.render_module_ref(data.owner), data.name)
            }
            TypeDefId::Union(id) => {
                let data = items
                    .union_data(id)
                    .expect("union id should exist while rendering view fixture type");
                format!(
                    "union {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    pub fn render_trait_ref(&self, trait_ref: TraitRef) -> String {
        let items = self
            .body_ir
            .resident_item_store(trait_ref.origin)
            .expect("trait item store should exist while rendering view fixture type");
        let data = items
            .trait_data(trait_ref.id)
            .expect("trait id should exist while rendering view fixture type");
        format!(
            "trait {}::{}",
            self.render_module_ref(data.owner),
            data.name
        )
    }

    pub fn render_module_ref(&self, module_ref: ModuleRef) -> String {
        if let DefMapRef::Body(body_ref) = module_ref.origin {
            // TODO: Preserve body-local module identity in fixture output. Distinct inline modules
            // owned by the same body currently render as the same owner label.
            let owner = self
                .resident_body_owner(body_ref)
                .expect("body module owner should exist while rendering view fixture module");
            return self.render_body_owner(owner);
        }

        let target_ref = module_ref.origin.origin_target();
        let package = self
            .parse_db()
            .packages()
            .get(target_ref.package.0)
            .expect("package slot should exist while rendering view fixture module");
        let target = package
            .target(target_ref.target)
            .expect("target id should exist while rendering view fixture module");

        format!(
            "{}[{}]::{}",
            package.package_name(),
            target.kind,
            self.module_path(module_ref),
        )
    }

    fn render_function_ref(&self, function_ref: FunctionRef) -> String {
        let items = self
            .body_ir
            .resident_item_store(function_ref.origin)
            .expect("function item store should exist while rendering view fixture body item");
        let data = items
            .function_data(function_ref.id)
            .expect("function ref should exist while rendering view fixture body item");
        let owner = self.render_item_owner(function_ref.origin, data.owner);

        format!("fn {owner}::{}", data.name)
    }

    fn render_body_owner(&self, owner: BodyOwner) -> String {
        match owner {
            BodyOwner::Function(function_ref) => self.render_function_ref(function_ref),
            BodyOwner::Const(const_ref) => {
                let items = self
                    .body_ir
                    .resident_item_store(const_ref.origin)
                    .expect("const item store should exist while rendering view fixture body item");
                let data = items
                    .const_data(const_ref.id)
                    .expect("const ref should exist while rendering view fixture body item");
                let owner = self.render_item_owner(const_ref.origin, data.owner);
                format!("const {owner}::{}", data.name)
            }
            BodyOwner::Static(static_ref) => {
                let items = self.body_ir.resident_item_store(static_ref.origin).expect(
                    "static item store should exist while rendering view fixture body item",
                );
                let data = items
                    .static_data(static_ref.id)
                    .expect("static ref should exist while rendering view fixture body item");
                format!(
                    "static {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    fn render_item_owner(&self, origin: DefMapRef, owner: ItemOwner) -> String {
        match owner {
            ItemOwner::Module(module_ref) => self.render_module_ref(module_ref),
            ItemOwner::Trait(trait_id) => {
                let items = self
                    .body_ir
                    .resident_item_store(origin)
                    .expect("trait item store should exist while rendering view fixture body item");
                let trait_data = items
                    .trait_data(trait_id)
                    .expect("trait owner should exist while rendering view fixture body item");
                format!(
                    "trait {}::{}",
                    self.render_module_ref(trait_data.owner),
                    trait_data.name
                )
            }
            // TODO: Render enough impl owner detail for snapshots to distinguish distinct impls.
            ItemOwner::Impl(_) => "impl".to_string(),
        }
    }

    fn module_path(&self, module_ref: ModuleRef) -> String {
        let module = self
            .resident_def_map(module_ref.origin.origin_target())
            .expect("target def map should exist while rendering view fixture module path")
            .module(module_ref.module)
            .expect("module id should exist while rendering view fixture module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(ModuleRef {
                    origin: module_ref.origin,
                    module: parent,
                });
                let name = module
                    .name
                    .as_deref()
                    .expect("non-root modules should have names");
                format!("{parent_path}::{name}")
            }
            None => "crate".to_string(),
        }
    }
}
