use rg_body_ir::BodyOwner;
use rg_ir_model::{
    DefMapRef, FunctionRef, GenericParamRef, ItemOwner, ModuleRef, TraitDefRef, TypeDefId,
    TypeDefRef,
};
use rg_parse::ParseDb;
use rg_semantic_ir::{GenericParamSource, GenericsQuery, ItemStoreQuery};
use rg_ty::{
    AdtTy, AliasTy, GenericArg, Lifetime, OpaqueTy, TraitRefLowering, Ty,
    signature::SemanticSignatureQuery,
};

use crate::{IndexedViewDb, ty::IndexedType};

/// Show the compiler type details needed by inference snapshots using the same readers as the query.
pub fn render_indexed_type(db: &IndexedViewDb<'_>, parse: &ParseDb, ty: &IndexedType) -> String {
    TypeRenderer { db, parse }.render_ty(ty.raw())
}

struct TypeRenderer<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    parse: &'a ParseDb,
}

impl TypeRenderer<'_, '_> {
    fn render_type_def_ref(&self, ty: TypeDefRef) -> String {
        let items = ItemStoreQuery::new(self.db)
            .semantic_item_view(ty.into())
            .expect("fixture type should load")
            .expect("fixture type should exist");

        match ty.id {
            TypeDefId::Struct(_) => {
                let data = items
                    .struct_data()
                    .expect("struct id should exist while rendering view fixture type");
                format!(
                    "struct {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            TypeDefId::Enum(_) => {
                let data = items
                    .enum_data()
                    .expect("enum id should exist while rendering view fixture type");
                format!("enum {}::{}", self.render_module_ref(data.owner), data.name)
            }
            TypeDefId::Union(_) => {
                let data = items
                    .union_data()
                    .expect("union id should exist while rendering view fixture type");
                format!(
                    "union {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    fn render_trait_ref(&self, trait_ref: TraitDefRef) -> String {
        let data = ItemStoreQuery::new(self.db)
            .trait_data(trait_ref)
            .expect("fixture item should load")
            .expect("trait id should exist while rendering view fixture type");
        format!(
            "trait {}::{}",
            self.render_module_ref(data.owner),
            data.name
        )
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
            Ty::Dyn(object) => {
                let mut args = object
                    .args
                    .iter()
                    .map(|arg| self.render_generic_arg(arg))
                    .collect::<Vec<_>>();
                for binding in &object.associated_types {
                    args.push(format!(
                        "{} = {}",
                        self.render_associated_ty_name(binding.associated_ty),
                        self.render_ty(&binding.ty)
                    ));
                }
                let args = if args.is_empty() {
                    String::new()
                } else {
                    format!("<{}>", args.join(", "))
                };
                let lifetime = match object.lifetime {
                    Lifetime::Erased => String::new(),
                    lifetime => format!(" + {lifetime}"),
                };
                format!("dyn {}{args}{lifetime}", self.render_trait_ref(object.def))
            }
            Ty::Param(param) => self.render_type_param(*param),
            Ty::Alias(AliasTy::Projection(alias)) => format!(
                "projection {}{}",
                self.render_associated_ty_name(alias.associated_ty),
                self.render_generic_args(&alias.args)
            ),
            Ty::Alias(AliasTy::Opaque(opaque)) => self.render_opaque(opaque),
            Ty::Unknown => "<unknown>".to_string(),
        }
    }

    fn render_type_param(&self, param: rg_ir_model::TypeParamRef) -> String {
        let generics = GenericsQuery::new(self.db)
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
                let mut bounds = SemanticSignatureQuery::new(self.db, self.db)
                    .function_type_param_bounds(param)
                    .expect("fixture APIT predicates should lower while rendering a type")
                    .iter()
                    .map(|bound| self.render_opaque_bound(bound))
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
        let mut bounds = SemanticSignatureQuery::new(self.db, self.db)
            .opaque_bounds(opaque)
            .expect("fixture opaque predicates should lower while rendering a type")
            .unwrap_or_default()
            .iter()
            .map(|bound| self.render_opaque_bound(bound))
            .collect::<Vec<_>>();
        bounds.sort();
        if bounds.is_empty() {
            "impl _".to_string()
        } else {
            format!("impl {}", bounds.join(" + "))
        }
    }

    fn render_opaque_bound(&self, bound: &TraitRefLowering) -> String {
        let mut args = bound
            .application
            .args
            .iter()
            .skip(1)
            .map(|arg| self.render_generic_arg(arg))
            .collect::<Vec<_>>();
        for binding in &bound.associated_types {
            let name = ItemStoreQuery::new(self.db)
                .type_alias_data(binding.projection.associated_ty)
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
        ItemStoreQuery::new(self.db)
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

    fn render_module_ref(&self, module_ref: ModuleRef) -> String {
        if let DefMapRef::Body(body_ref) = module_ref.origin {
            // TODO: Preserve body-local module identity in fixture output. Distinct inline modules
            // owned by the same body currently render as the same owner label.
            let owner = self
                .db
                .body_ir
                .body(body_ref)
                .expect("fixture body should load")
                .expect("body module owner should exist while rendering view fixture module")
                .owner();
            return self.render_body_owner(owner);
        }

        let crate_ref = module_ref.origin.origin_crate();
        let package = self
            .parse
            .packages()
            .get(crate_ref.package.0)
            .expect("package slot should exist while rendering view fixture module");
        let cargo_target = self
            .db
            .def_map
            .crate_data(crate_ref)
            .expect("fixture declarations should load")
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
        let data = ItemStoreQuery::new(self.db)
            .function_data(function_ref)
            .expect("fixture item should load")
            .expect("function ref should exist while rendering view fixture body item");
        let owner = self.render_item_owner(function_ref.store.origin, data.owner);

        format!("fn {owner}::{}", data.name)
    }

    fn render_body_owner(&self, owner: BodyOwner) -> String {
        match owner {
            BodyOwner::Function(function_ref) => self.render_function_ref(function_ref),
            BodyOwner::Const(const_ref) => {
                let data = ItemStoreQuery::new(self.db)
                    .const_data(const_ref)
                    .expect("fixture item should load")
                    .expect("const ref should exist while rendering view fixture body item");
                let owner = self.render_item_owner(const_ref.store.origin, data.owner);
                format!("const {owner}::{}", data.name)
            }
            BodyOwner::Static(static_ref) => {
                let data = ItemStoreQuery::new(self.db)
                    .static_data(static_ref)
                    .expect("fixture item should load")
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
                let trait_data = ItemStoreQuery::new(self.db)
                    .trait_data(TraitDefRef {
                        origin,
                        id: trait_id,
                    })
                    .expect("fixture trait should load")
                    .expect("fixture trait owner exists");
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
            .db
            .def_map_for_origin(module_ref.origin)
            .expect("fixture def map should load")
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
