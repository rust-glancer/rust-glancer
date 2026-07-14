//! Body adapter for the shared semantic type lowerer.

use rg_def_map::DefMapSource;
use rg_ir_model::items::{GenericArg as ItemGenericArg, TypeRef};
use rg_ir_model::{GenericDefRef, ModuleRef, Path, ScopeId, TypePathResolution};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStoreSource, TypePathContext};
use rg_ty::{
    GenericArgs, Substitution, TraitRefLowering, Ty, TypeLoweringAnchor, TypeLoweringEnv,
    TypeLoweringQuery, TypePathResolver, inference::InferenceTable,
};

use crate::ir::BodyOwner;
use crate::resolution::BodyResolutionContext;

/// Place where a source type was written.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TypeRefUseSite {
    Scope(ScopeId),
    Module(ModuleRef),
    OwnerContext(TypePathContext),
}

/// Body-scoped entry point to the canonical lowerer.
pub(crate) struct TypeRefResolutionQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    use_site: TypeRefUseSite,
    subst: Substitution,
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
            subst: Substitution::new(),
        }
    }

    pub(crate) fn with_subst(mut self, subst: &Substitution) -> Self {
        self.subst = subst.clone();
        self
    }

    pub(crate) fn resolve(&self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let resolver = BodyTypePathResolver::new(self.context);
        let lowering = TypeLoweringQuery::new(&item_paths, &resolver);
        let owner = self.owner();
        lowering.lower(
            ty,
            TypeLoweringEnv::new(owner, self.anchor()).with_substitution(self.subst.clone()),
        )
    }

    pub(crate) fn resolve_with_inference(
        &self,
        ty: &TypeRef,
        table: &mut InferenceTable,
    ) -> Result<Ty, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let resolver = BodyTypePathResolver::new(self.context);
        let lowering = TypeLoweringQuery::new(&item_paths, &resolver);
        let owner = self.owner();
        lowering
            .session(
                TypeLoweringEnv::new(owner, self.anchor()).with_substitution(self.subst.clone()),
            )?
            .lower_type_ref_with_inference(ty, table)
    }

    pub(crate) fn resolve_generic_args_for(
        &self,
        target: GenericDefRef,
        args: &[ItemGenericArg],
    ) -> Result<GenericArgs, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let resolver = BodyTypePathResolver::new(self.context);
        let lowering = TypeLoweringQuery::new(&item_paths, &resolver);
        let owner = self.owner();
        let mut session = lowering.session(
            TypeLoweringEnv::new(owner, self.anchor()).with_substitution(self.subst.clone()),
        )?;
        session.lower_generic_args_for(target, args)
    }

    pub(crate) fn resolve_trait_ref(
        &self,
        bound: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<TraitRefLowering>, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let resolver = BodyTypePathResolver::new(self.context);
        let lowering = TypeLoweringQuery::new(&item_paths, &resolver);
        let owner = self.owner();
        let mut session = lowering.session(
            TypeLoweringEnv::new(owner, self.anchor()).with_substitution(self.subst.clone()),
        )?;
        session.lower_trait_ref(bound, self_ty)
    }

    fn owner(&self) -> GenericDefRef {
        match self.use_site {
            TypeRefUseSite::OwnerContext(context) => context
                .impl_ref
                .map(GenericDefRef::Impl)
                .unwrap_or(self.body_owner()),
            TypeRefUseSite::Scope(_) | TypeRefUseSite::Module(_) => self.body_owner(),
        }
    }

    fn body_owner(&self) -> GenericDefRef {
        match self.context.body().owner() {
            BodyOwner::Function(owner) => GenericDefRef::Function(owner),
            BodyOwner::Const(owner) => GenericDefRef::Const(owner),
            BodyOwner::Static(owner) => GenericDefRef::Static(owner),
        }
    }

    fn anchor(&self) -> TypeLoweringAnchor {
        match self.use_site {
            TypeRefUseSite::Scope(scope) => TypeLoweringAnchor::Scope(scope),
            TypeRefUseSite::Module(module) => {
                if let Some(scope) = self
                    .context
                    .body()
                    .scope_for_module(self.context.body_ref(), module)
                {
                    TypeLoweringAnchor::Scope(scope)
                } else {
                    TypeLoweringAnchor::Context(TypePathContext::module(module))
                }
            }
            TypeRefUseSite::OwnerContext(context) => TypeLoweringAnchor::Context(context),
        }
    }
}

/// Body-specific path lookup used by the shared lowering visitor.
pub(crate) struct BodyTypePathResolver<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyTypePathResolver<'query, D, I> {
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }
}

impl<'query, D, I> TypePathResolver for BodyTypePathResolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    type Error = PackageStoreError;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, Self::Error> {
        match anchor {
            TypeLoweringAnchor::Scope(scope) => {
                self.context.type_path_query().resolve_in_scope(scope, path)
            }
            TypeLoweringAnchor::Context(context) => self
                .context
                .type_path_query()
                .resolve_in_context(context, path),
        }
    }
}
