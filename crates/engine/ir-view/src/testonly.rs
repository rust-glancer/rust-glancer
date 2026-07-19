use rg_body_ir::{BodyIrLoader, BodyOwner, BodyView, ExprData, testonly::BodyIrFixture};
use rg_def_map::DefMap;
use rg_def_map::DefMapDb;
use rg_ir_model::{
    BodyRef, BodySource, CrateRef, DefMapRef, ExprId, FunctionRef, GenericParamRef, ItemOwner,
    ModuleRef, TraitDefRef, TypeDefId, TypeDefRef,
};
use rg_package_store::PackageLoader;
use rg_parse::ParseDb;
use rg_semantic_ir::{
    GenericParamSource, GenericsQuery, ItemStore, ItemStoreQuery, SemanticIrDb,
    testonly::SemanticIrFixture,
};
use rg_ty::{
    AdtTy, AliasTy, GenericArg, Lifetime, OpaqueTy, SemanticSignatureQuery, TraitRefLowering, Ty,
};

use crate::{IndexedViewDb, ty::IndexedType};

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
                .read_txn(BodyIrLoader::resident_only("resident view fixture")),
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

    pub fn resident_def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.body_ir.resident_def_map(crate_ref)
    }

    pub fn resident_crate_ir(&self, crate_ref: CrateRef) -> Option<&ItemStore> {
        self.body_ir.resident_crate_ir(crate_ref)
    }

    pub fn resident_body(&self, body_ref: BodyRef) -> Option<BodyView<'_>> {
        self.body_ir.resident_body(body_ref)
    }

    pub fn resident_body_source(&self, body_ref: BodyRef) -> Option<BodySource> {
        self.resident_body(body_ref).map(|body| body.source())
    }

    pub fn resident_expr(&self, body_ref: BodyRef, expr: ExprId) -> Option<&ExprData> {
        self.resident_body(body_ref)?.expr(expr)
    }

    pub fn resident_body_owner(&self, body_ref: BodyRef) -> Option<BodyOwner> {
        self.resident_body(body_ref).map(BodyView::owner)
    }

    pub fn resident_body_item_store(&self, body_ref: BodyRef) -> Option<&ItemStore> {
        self.body_ir.resident_body_item_store(body_ref)
    }

    pub fn first_body_ref(&self, crate_ref: CrateRef) -> Option<BodyRef> {
        self.body_refs_for_crate(crate_ref).into_iter().next()
    }

    pub fn body_refs_for_crate(&self, crate_ref: CrateRef) -> Vec<BodyRef> {
        let Some(package) = self
            .body_ir
            .body_ir_db()
            .resident_package(crate_ref.package)
        else {
            return Vec::new();
        };
        let Some(crate_bodies) = package.crate_bodies(crate_ref.crate_id) else {
            return Vec::new();
        };

        crate_bodies
            .body_views()
            .map(|(body, _)| BodyRef { crate_ref, body })
            .collect()
    }

    pub fn crate_owns_file(&self, crate_ref: CrateRef, file_id: rg_parse::FileId) -> bool {
        self.resident_def_map(crate_ref).is_some_and(|def_map| {
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

    pub fn render_trait_ref(&self, trait_ref: TraitDefRef) -> String {
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

    /// Render the detailed compiler type vocabulary used by view and analysis snapshots.
    ///
    /// Product code sees `IndexedType` as an opaque projection. Snapshot tests deliberately need
    /// more detail to distinguish inference regressions, so that privileged rendering stays in the
    /// facade's test support instead of reopening the compiler representation in `rg_analysis`.
    pub fn render_indexed_type(&self, ty: &IndexedType) -> String {
        self.render_ty(ty.raw())
    }

    fn render_ty(&self, ty: &Ty) -> String {
        match ty {
            Ty::Unit => "()".to_string(),
            Ty::Never => "!".to_string(),
            Ty::Primitive(primitive) => primitive.label().to_string(),
            Ty::Tuple(fields) => {
                let fields = fields
                    .iter()
                    .map(|ty| self.render_ty(ty))
                    .collect::<Vec<_>>();
                let suffix = if fields.len() == 1 { "," } else { "" };
                format!("({}{suffix})", fields.join(", "))
            }
            Ty::Array { inner, len } => format!("[{}; {}]", self.render_ty(inner), len),
            Ty::Slice(inner) => format!("[{}]", self.render_ty(inner)),
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                let lifetime = match lifetime {
                    Lifetime::Erased => String::new(),
                    lifetime => format!("{lifetime} "),
                };
                format!(
                    "&{lifetime}{}{}",
                    if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                        "mut "
                    } else {
                        ""
                    },
                    self.render_ty(inner)
                )
            }
            Ty::RawPointer { mutability, inner } => {
                let qualifier = if matches!(mutability, rg_ir_model::Mutability::Mutable) {
                    "mut"
                } else {
                    "const"
                };
                format!("*{qualifier} {}", self.render_ty(inner))
            }
            Ty::FnPointer { params, ret } => {
                let params = params
                    .iter()
                    .map(|param| self.render_ty(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({params}) -> {}", self.render_ty(ret))
            }
            Ty::Closure(closure) => format!("closure #{}", closure.id),
            Ty::FnDef(function) => format!(
                "function item {:?}{}",
                function.def,
                self.render_generic_args(&function.args)
            ),
            Ty::Adt(ty) => format!("nominal {}", self.render_body_nominal_ty(ty)),
            Ty::Param(param) => self.render_type_param(*param),
            Ty::Alias(AliasTy::Projection(alias)) => format!(
                "projection {}{}",
                self.render_associated_ty_name(alias.associated_ty),
                self.render_generic_args(&alias.args)
            ),
            Ty::Alias(AliasTy::Opaque(opaque)) => self.render_opaque(opaque),
            Ty::InferVar { kind, id } => format!("infer {kind:?} {id:?}"),
            Ty::Unknown => "<unknown>".to_string(),
        }
    }

    fn render_type_param(&self, param: rg_ir_model::TypeParamRef) -> String {
        let db = self.view_db();
        let generics = GenericsQuery::new(&db)
            .generics(param.owner)
            .expect("fixture generic declarations should be available while rendering a type");
        let Some(data) = generics
            .iter()
            .find(|data| data.param() == GenericParamRef::Type(param))
        else {
            return "param <missing>".to_string();
        };
        match data.source() {
            GenericParamSource::Type(source) => format!("param {}", source.name),
            GenericParamSource::TraitSelf => "param Self".to_string(),
            GenericParamSource::ArgumentImplTrait(_) => {
                let mut bounds = SemanticSignatureQuery::new(&db, &db)
                    .function_type_param_bounds(param)
                    .expect("fixture APIT predicates should lower while rendering a type")
                    .iter()
                    .map(|bound| self.render_opaque_bound(&db, bound))
                    .collect::<Vec<_>>();
                bounds.sort();
                if bounds.is_empty() {
                    "param <argument impl Trait>".to_string()
                } else {
                    format!("impl {}", bounds.join(" + "))
                }
            }
            GenericParamSource::Lifetime(_) | GenericParamSource::Const(_) => {
                unreachable!("a type parameter should have type-like provenance")
            }
        }
    }

    fn render_opaque(&self, opaque: &OpaqueTy) -> String {
        let db = self.view_db();
        let mut bounds = SemanticSignatureQuery::new(&db, &db)
            .opaque_bounds(opaque)
            .expect("fixture opaque predicates should lower while rendering a type")
            .unwrap_or_default()
            .iter()
            .map(|bound| self.render_opaque_bound(&db, bound))
            .collect::<Vec<_>>();
        bounds.sort();
        if bounds.is_empty() {
            "impl _".to_string()
        } else {
            format!("impl {}", bounds.join(" + "))
        }
    }

    fn render_opaque_bound(&self, db: &IndexedViewDb<'_>, bound: &TraitRefLowering) -> String {
        let mut args = bound
            .application
            .args
            .iter()
            .skip(1)
            .map(|arg| self.render_generic_arg(arg))
            .collect::<Vec<_>>();
        for binding in &bound.associated_types {
            let name = ItemStoreQuery::new(db)
                .type_alias_data(binding.associated_ty)
                .expect("fixture associated type should load while rendering an opaque bound")
                .map(|data| data.name.to_string())
                .unwrap_or_else(|| "<missing>".to_string());
            args.push(format!("{name} = {}", self.render_ty(&binding.ty)));
        }
        let args = if args.is_empty() {
            String::new()
        } else {
            format!("<{}>", args.join(", "))
        };
        format!("{}{args}", self.render_trait_ref(bound.application.def))
    }

    fn render_associated_ty_name(&self, associated_ty: rg_ir_model::TypeAliasRef) -> String {
        let db = self.view_db();
        ItemStoreQuery::new(&db)
            .type_alias_data(associated_ty)
            .expect("fixture associated type should load while rendering a projection")
            .map(|data| data.name.to_string())
            .unwrap_or_else(|| "<missing>".to_string())
    }

    fn render_body_nominal_ty(&self, ty: &AdtTy) -> String {
        format!(
            "{}{}",
            self.render_type_def_ref(ty.def),
            self.render_generic_args(&ty.args)
        )
    }

    fn render_generic_args(&self, args: &[GenericArg]) -> String {
        if args.is_empty() {
            return String::new();
        }

        format!(
            "<{}>",
            args.iter()
                .map(|arg| self.render_generic_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.render_ty(ty),
            GenericArg::Lifetime(lifetime) => lifetime.to_string(),
            GenericArg::Const(value) => value.to_string(),
        }
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

        let crate_ref = module_ref.origin.origin_crate();
        let package = self
            .parse_db()
            .packages()
            .get(crate_ref.package.0)
            .expect("package slot should exist while rendering view fixture module");
        let cargo_target = self
            .def_map_db()
            .resident_package(crate_ref.package)
            .and_then(|package| package.crate_data(crate_ref.crate_id))
            .expect("semantic crate should exist while rendering view fixture module")
            .cargo_target();
        let target = package
            .target(cargo_target)
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
            .resident_def_map(module_ref.origin.origin_crate())
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
