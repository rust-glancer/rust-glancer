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
use rg_ty::{GenericArg, RefMutability, Ty, TypeSubst};

use crate::resolution::BodyResolutionContext;

use super::type_path::{prefix_type_ref, split_associated_path};

#[derive(Debug, Clone, Copy)]
pub(crate) enum TypeRefUseSite {
    Scope(ScopeId),
    Module(ModuleRef),
    OwnerContext(TypePathContext),
    Function(FunctionRef),
    BodyOwner,
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
        match self.use_site {
            TypeRefUseSite::Scope(scope) => self.resolve_in_scope(ty, scope),
            TypeRefUseSite::Module(module) => self.resolve_in_module(ty, module),
            TypeRefUseSite::OwnerContext(context) => self.resolve_in_owner_context(ty, context),
            TypeRefUseSite::Function(function) => {
                let type_paths = self.context.type_path_query();
                let context = type_paths
                    .context_for_function(function, self.context.body().owner_module())?;
                self.with_use_site(TypeRefUseSite::OwnerContext(context))
                    .resolve(ty)
            }
            TypeRefUseSite::BodyOwner => {
                let context = self.context.type_path_query().context_for_body_owner()?;
                self.with_use_site(TypeRefUseSite::OwnerContext(context))
                    .resolve(ty)
            }
        }
    }

    fn with_use_site(&self, use_site: TypeRefUseSite) -> Self {
        Self {
            context: self.context,
            use_site,
            subst: self.subst.clone(),
        }
    }

    fn resolve_in_scope(&self, ty: &TypeRef, scope: ScopeId) -> Result<Ty, PackageStoreError> {
        match ty {
            TypeRef::Path(type_path) => self.resolve_path_in_scope(ty, type_path, scope),
            _ => self.with_use_site(TypeRefUseSite::BodyOwner).resolve(ty),
        }
    }

    fn resolve_in_module(&self, ty: &TypeRef, module: ModuleRef) -> Result<Ty, PackageStoreError> {
        if let Some(scope) = self
            .context
            .body()
            .scope_for_module(self.context.body_ref(), module)
        {
            return self.with_use_site(TypeRefUseSite::Scope(scope)).resolve(ty);
        }

        if let DefMapRef::Body(_) = module.origin {
            return self.resolve_in_body_context(ty, TypePathContext::module(module));
        }

        self.resolve_in_plain_context(ty, TypePathContext::module(module))
    }

    fn resolve_in_owner_context(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
    ) -> Result<Ty, PackageStoreError> {
        if context.module.origin == DefMapRef::Body(self.context.body_ref()) {
            return self
                .with_use_site(TypeRefUseSite::Module(context.module))
                .resolve(ty);
        }

        if let DefMapRef::Body(_) = context.module.origin {
            return self.resolve_in_body_context(ty, context);
        }

        self.resolve_in_plain_context(ty, context)
    }

    fn resolve_path_in_scope(
        &self,
        original_ty: &TypeRef,
        type_path: &TypePath,
        scope: ScopeId,
    ) -> Result<Ty, PackageStoreError> {
        let path = Path::from_type_path(type_path);
        if let Some(ty) = self.subst_for_single_segment(&path) {
            return Ok(ty);
        }
        if path.is_self_type() {
            let type_paths = self.context.type_path_query();
            let context = type_paths.context_for_body_owner()?;
            let self_tys = type_paths.self_nominal_tys_for_context(context)?;
            return Ok(Ty::self_ty(self_tys));
        }

        let args = self.generic_args_from_type_path(type_path)?;
        if let Some(ty) = self.ty_from_local_associated_type_path(type_path, &path, scope, &args)? {
            return Ok(ty);
        }

        let resolution = self
            .context
            .type_path_query()
            .resolve_in_scope(scope, &path)?;
        self.ty_from_resolution(original_ty, &path, resolution, args)
    }

    fn resolve_in_body_context(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
    ) -> Result<Ty, PackageStoreError> {
        let TypeRef::Path(type_path) = ty else {
            return self.resolve_structural_type(ty);
        };

        let path = Path::from_type_path(type_path);
        if let Some(ty) = self.subst_for_single_segment(&path) {
            return Ok(ty);
        }
        if path.is_self_type() {
            let self_tys = self
                .context
                .type_path_query()
                .self_nominal_tys_for_context(context)?;
            return Ok(Ty::self_ty(self_tys));
        }

        let args = self.generic_args_from_type_path(type_path)?;
        let resolution = self.resolve_type_path_in_body_context(context, &path)?;
        self.ty_from_resolution(ty, &path, resolution, args)
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

    fn resolve_structural_type(&self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
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
                self.resolve(inner)?,
            )),
            TypeRef::Unknown(_) | TypeRef::Infer => Ok(Ty::Unknown),
            TypeRef::Tuple(types) if types.is_empty() => Ok(Ty::Unit),
            TypeRef::Tuple(types) => Ok(Ty::tuple(
                types
                    .iter()
                    .map(|ty| self.resolve(ty))
                    .collect::<Result<_, _>>()?,
            )),
            TypeRef::Slice(inner) => Ok(Ty::slice(self.resolve(inner)?)),
            TypeRef::Array { inner, len } => Ok(Ty::array(self.resolve(inner)?, len.clone())),
            _ => Ok(Ty::syntax(ty.clone())),
        }
    }

    fn resolve_type_path_in_body_context(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(TypePathResolution::Unknown);
            };
            let types = self
                .context
                .item_query()
                .impl_data(impl_ref)?
                .map(|data| data.resolved_self_tys.clone())
                .unwrap_or_default();
            return Ok(if types.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::SelfType(types)
            });
        }

        if let Some((prefix, name)) = split_associated_path(path) {
            let prefix_resolution = self.resolve_type_path_in_body_context(context, &prefix)?;
            let prefix_ty =
                Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
            let mut aliases = UniqueVec::new();
            for ty in prefix_ty.as_nominals() {
                if let Some(alias) = self
                    .context
                    .type_path_query()
                    .associated_type_alias_for_type(ty, name)?
                {
                    aliases.push(alias);
                }
            }
            if !aliases.is_empty() {
                return Ok(TypePathResolution::TypeAliases(aliases));
            }
        }

        let body_items = self
            .context
            .type_path_query()
            .resolve_body_type_items_from_module(context.module, path)?;
        Ok(self
            .context
            .type_path_query()
            .type_resolution_from_items(body_items))
    }

    fn ty_from_local_associated_type_path(
        &self,
        type_path: &TypePath,
        path: &Path,
        scope: ScopeId,
        args: &[GenericArg],
    ) -> Result<Option<Ty>, PackageStoreError> {
        let Some((_, name)) = split_associated_path(path) else {
            return Ok(None);
        };
        let Some(prefix_ty_ref) = prefix_type_ref(type_path) else {
            return Ok(None);
        };
        let prefix_ty = self
            .with_use_site(TypeRefUseSite::Scope(scope))
            .resolve(&prefix_ty_ref)?;

        for ty in prefix_ty.as_nominals() {
            let Some(alias_ref) = self
                .context
                .type_path_query()
                .associated_type_alias_for_type(ty, name)?
            else {
                continue;
            };
            return self
                .context
                .type_path_query()
                .ty_from_associated_type_alias(alias_ref, ty, args)
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
            return self.context.type_path_query().ty_from_type_aliases(
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
    ) -> Result<Vec<GenericArg>, PackageStoreError> {
        let Some(segment) = type_path.segments.last() else {
            return Ok(Vec::new());
        };

        let mut generic_args = Vec::new();
        for arg in &segment.args {
            generic_args.push(self.generic_arg(arg)?);
        }
        Ok(generic_args)
    }

    pub(crate) fn generic_arg(
        &self,
        arg: &ItemGenericArg,
    ) -> Result<GenericArg, PackageStoreError> {
        match arg {
            ItemGenericArg::Type(ty) => Ok(GenericArg::Type(Box::new(self.resolve(ty)?))),
            ItemGenericArg::Lifetime(lifetime) => Ok(GenericArg::Lifetime(lifetime.clone())),
            ItemGenericArg::Const(value) => Ok(GenericArg::Const(value.clone())),
            ItemGenericArg::FnTraitArgs { params, ret } => Ok(GenericArg::FnTraitArgs {
                params: params
                    .iter()
                    .map(|ty| self.resolve(ty))
                    .collect::<Result<_, _>>()?,
                ret: Box::new(self.resolve(ret)?),
            }),
            ItemGenericArg::AssocType { name, ty } => Ok(GenericArg::AssocType {
                name: name.clone(),
                ty: ty
                    .as_ref()
                    .map(|ty| self.resolve(ty).map(Box::new))
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
