//! Type-ref resolution from an explicit body use site.
//!
//! `TypeRef` resolution is the same operation everywhere, but the lookup anchor changes: a let
//! annotation starts from a body scope, an associated item signature starts from its owner context,
//! and a field type starts from its declaring module. Keeping that anchor as data prevents the
//! resolver API from growing one method per caller shape.

use rg_ir_model::{
    DefMapRef, FunctionRef, ModuleRef, Path, ScopeId, TypePathResolution,
    items::{GenericArg as ItemGenericArg, Mutability, PrimitiveTy, TypePath, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{GenericArg, NominalTy, RefMutability, Ty, TypeSubst};

use crate::resolution::BodyResolutionContext;

#[derive(Debug, Clone, Copy)]
pub(crate) enum TypeRefUseSite {
    Scope(ScopeId),
    Module(ModuleRef),
    OwnerContext(TypePathContext),
    Function(FunctionRef),
}

#[derive(Debug, Clone, Copy)]
enum TypeRefAnchor {
    Scope(ScopeId),
    BodyContext(TypePathContext),
    PlainContext(TypePathContext),
}

pub(crate) struct TypeRefResolutionQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    use_site: TypeRefUseSite,
    subst: TypeSubst,
}

impl<'query, D, I> TypeRefResolutionQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(
        context: BodyResolutionContext<'query, D, I>,
        use_site: TypeRefUseSite,
    ) -> Self {
        Self {
            context,
            use_site,
            subst: TypeSubst::new(),
        }
    }

    pub(crate) fn with_subst(mut self, subst: &TypeSubst) -> Self {
        self.subst = subst.clone();
        self
    }

    pub(crate) fn resolve(&self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        let anchor = self.anchor_for_use_site(self.use_site)?;
        self.resolve_at(ty, anchor)
    }

    fn anchor_for_use_site(
        &self,
        use_site: TypeRefUseSite,
    ) -> Result<TypeRefAnchor, PackageStoreError> {
        match use_site {
            TypeRefUseSite::Scope(scope) => Ok(TypeRefAnchor::Scope(scope)),
            TypeRefUseSite::Module(module) => Ok(self.anchor_for_module(module)),
            TypeRefUseSite::OwnerContext(context) => Ok(self.anchor_for_owner_context(context)),
            TypeRefUseSite::Function(function) => {
                let context = self.context.type_contexts().for_function(function)?;
                Ok(self.anchor_for_owner_context(context))
            }
        }
    }

    fn anchor_for_module(&self, module: ModuleRef) -> TypeRefAnchor {
        if let Some(scope) = self
            .context
            .body()
            .scope_for_module(self.context.body_ref(), module)
        {
            return TypeRefAnchor::Scope(scope);
        }

        if let DefMapRef::Body(_) = module.origin {
            TypeRefAnchor::BodyContext(TypePathContext::module(module))
        } else {
            TypeRefAnchor::PlainContext(TypePathContext::module(module))
        }
    }

    fn anchor_for_owner_context(&self, context: TypePathContext) -> TypeRefAnchor {
        if context.module.origin == DefMapRef::Body(self.context.body_ref()) {
            return self.anchor_for_module(context.module);
        }

        if let DefMapRef::Body(_) = context.module.origin {
            TypeRefAnchor::BodyContext(context)
        } else {
            TypeRefAnchor::PlainContext(context)
        }
    }

    fn resolve_at(&self, ty: &TypeRef, anchor: TypeRefAnchor) -> Result<Ty, PackageStoreError> {
        if let TypeRefAnchor::PlainContext(context) = anchor {
            return self.resolve_in_plain_context(ty, context);
        }

        let TypeRef::Path(type_path) = ty else {
            return self.resolve_structural_type(ty, anchor);
        };

        self.resolve_path_at(ty, type_path, anchor)
    }

    fn resolve_path_at(
        &self,
        original_ty: &TypeRef,
        type_path: &TypePath,
        anchor: TypeRefAnchor,
    ) -> Result<Ty, PackageStoreError> {
        let path = Path::from_type_path(type_path);
        if let Some(ty) = self.subst_for_single_segment(&path) {
            return Ok(ty);
        }
        if path.is_self_type() {
            let self_tys = self.self_tys_for_anchor(anchor)?;
            return Ok(Ty::self_ty(self_tys));
        }

        let args = self.generic_args_from_type_path(type_path, anchor)?;
        if let Some(ty) = self.ty_from_associated_alias_path(type_path, &path, anchor, &args)? {
            return Ok(ty);
        }

        let resolution = self.resolve_type_path(anchor, &path)?;
        self.ty_from_resolution(original_ty, &path, resolution, args)
    }

    fn self_tys_for_anchor(
        &self,
        anchor: TypeRefAnchor,
    ) -> Result<UniqueVec<NominalTy>, PackageStoreError> {
        let type_contexts = self.context.type_contexts();
        let context = match anchor {
            TypeRefAnchor::Scope(_) => type_contexts.for_body_owner()?,
            TypeRefAnchor::BodyContext(context) | TypeRefAnchor::PlainContext(context) => context,
        };
        type_contexts.nominal_self_tys_for_context(context)
    }

    fn resolve_type_path(
        &self,
        anchor: TypeRefAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        match anchor {
            TypeRefAnchor::Scope(scope) => {
                self.context.type_path_query().resolve_in_scope(scope, path)
            }
            TypeRefAnchor::BodyContext(context) => self
                .context
                .type_path_query()
                .resolve_in_context(context, path),
            TypeRefAnchor::PlainContext(context) => {
                self.context.item_paths().resolve_type_path(context, path)
            }
        }
    }

    fn resolve_in_plain_context(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
    ) -> Result<Ty, PackageStoreError> {
        self.context
            .item_paths()
            .resolve_type_ref(ty, context, Ty::syntax(ty.clone()), &self.subst)
    }

    fn resolve_structural_type(
        &self,
        ty: &TypeRef,
        anchor: TypeRefAnchor,
    ) -> Result<Ty, PackageStoreError> {
        match ty {
            TypeRef::Unit => Ok(Ty::Unit),
            TypeRef::Never => Ok(Ty::Never),
            TypeRef::Reference {
                mutability, inner, ..
            } => Ok(Ty::reference(
                match mutability {
                    Mutability::Shared => RefMutability::Shared,
                    Mutability::Mutable => RefMutability::Mutable,
                },
                self.resolve_at(inner, anchor)?,
            )),
            TypeRef::Unknown(_) | TypeRef::Infer => Ok(Ty::Unknown),
            TypeRef::Tuple(types) if types.is_empty() => Ok(Ty::Unit),
            TypeRef::Tuple(types) => Ok(Ty::tuple(
                types
                    .iter()
                    .map(|ty| self.resolve_at(ty, anchor))
                    .collect::<Result<_, _>>()?,
            )),
            TypeRef::Slice(inner) => Ok(Ty::slice(self.resolve_at(inner, anchor)?)),
            TypeRef::Array { inner, len } => {
                Ok(Ty::array(self.resolve_at(inner, anchor)?, len.clone()))
            }
            _ => Ok(Ty::syntax(ty.clone())),
        }
    }

    fn ty_from_associated_alias_path(
        &self,
        type_path: &TypePath,
        path: &Path,
        prefix_anchor: TypeRefAnchor,
        args: &[GenericArg],
    ) -> Result<Option<Ty>, PackageStoreError> {
        let Some((_, name)) = path.split_prefix_name() else {
            return Ok(None);
        };
        let Some(prefix_ty_ref) = prefix_type_ref(type_path) else {
            return Ok(None);
        };
        let prefix_ty = self.resolve_at(&prefix_ty_ref, prefix_anchor)?;

        for ty in prefix_ty.as_nominals() {
            let Some(alias_ref) = self
                .context
                .type_aliases()
                .associated_alias_for_type(ty, name)?
            else {
                continue;
            };
            return self
                .context
                .type_aliases()
                .ty_from_associated_alias(alias_ref, ty, args)
                .map(Some);
        }

        Ok(None)
    }

    fn ty_from_resolution(
        &self,
        original_ty: &TypeRef,
        path: &Path,
        resolution: TypePathResolution,
        args: Vec<GenericArg>,
    ) -> Result<Ty, PackageStoreError> {
        if let TypePathResolution::TypeAliases(aliases) = &resolution {
            return self.context.type_aliases().ty_from_aliases(
                aliases.as_slice(),
                &args,
                &self.subst,
            );
        }
        let is_unknown = matches!(resolution, TypePathResolution::Unknown);
        Ok(
            Ty::from_type_path_resolution(resolution, args).unwrap_or_else(|| {
                if is_unknown {
                    path.single_name()
                        .and_then(PrimitiveTy::from_name)
                        .map(Ty::Primitive)
                        .unwrap_or_else(|| Ty::syntax(original_ty.clone()))
                } else {
                    Ty::syntax(original_ty.clone())
                }
            }),
        )
    }

    fn generic_args_from_type_path(
        &self,
        type_path: &TypePath,
        anchor: TypeRefAnchor,
    ) -> Result<Vec<GenericArg>, PackageStoreError> {
        let Some(segment) = type_path.segments.last() else {
            return Ok(Vec::new());
        };

        let mut generic_args = Vec::new();
        for arg in &segment.args {
            generic_args.push(self.generic_arg_at(arg, anchor)?);
        }
        Ok(generic_args)
    }

    pub(super) fn resolve_generic_arg(
        &self,
        arg: &ItemGenericArg,
    ) -> Result<GenericArg, PackageStoreError> {
        let anchor = self.anchor_for_use_site(self.use_site)?;
        self.generic_arg_at(arg, anchor)
    }

    fn generic_arg_at(
        &self,
        arg: &ItemGenericArg,
        anchor: TypeRefAnchor,
    ) -> Result<GenericArg, PackageStoreError> {
        match arg {
            ItemGenericArg::Type(ty) => {
                Ok(GenericArg::Type(Box::new(self.resolve_at(ty, anchor)?)))
            }
            ItemGenericArg::Lifetime(lifetime) => Ok(GenericArg::Lifetime(lifetime.clone())),
            ItemGenericArg::Const(value) => Ok(GenericArg::Const(value.clone())),
            ItemGenericArg::FnTraitArgs { params, ret } => Ok(GenericArg::FnTraitArgs {
                params: params
                    .iter()
                    .map(|ty| self.resolve_at(ty, anchor))
                    .collect::<Result<_, _>>()?,
                ret: Box::new(self.resolve_at(ret, anchor)?),
            }),
            ItemGenericArg::AssocType { name, ty } => Ok(GenericArg::AssocType {
                name: name.clone(),
                ty: ty
                    .as_ref()
                    .map(|ty| self.resolve_at(ty, anchor).map(Box::new))
                    .transpose()?,
            }),
            ItemGenericArg::Unsupported(text) => Ok(GenericArg::Unsupported(text.clone())),
        }
    }

    fn subst_for_single_segment(&self, path: &Path) -> Option<Ty> {
        path.single_name()
            .and_then(|name| self.subst.type_param(name))
    }
}

fn prefix_type_ref(path: &TypePath) -> Option<TypeRef> {
    let prefix_len = path.segments.len().checked_sub(1)?;
    if prefix_len == 0 {
        return None;
    }

    Some(TypeRef::Path(TypePath {
        source_span: path.source_span,
        absolute: path.absolute,
        segments: path.segments[..prefix_len].to_vec(),
    }))
}
