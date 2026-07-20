//! Body adapter for the shared semantic type lowerer.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ScopeId};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{
    GenericArgs, TraitRefLowering, Ty, TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery,
    inference::InferenceTable,
};

use crate::ir::BodyOwner;
use crate::resolution::BodyResolutionContext;

/// Body-scoped entry point to the canonical lowerer.
pub(crate) struct TypeRefResolutionQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    scope: ScopeId,
}

impl<'query, D, I> TypeRefResolutionQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>, scope: ScopeId) -> Self {
        Self { context, scope }
    }

    pub(crate) fn resolve(&self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        lowering.lower(ty, TypeLoweringEnv::new(self.body_owner(), self.anchor()))
    }

    pub(crate) fn resolve_with_inference(
        &self,
        ty: &TypeRef,
        table: &mut InferenceTable,
    ) -> Result<Ty, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        lowering
            .session(TypeLoweringEnv::new(self.body_owner(), self.anchor()))?
            .lower_type_ref_with_inference(ty, table)
    }

    pub(crate) fn resolve_generic_args_for(
        &self,
        target: GenericDefRef,
        args: &[ItemGenericArg],
    ) -> Result<GenericArgs, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        let mut session =
            lowering.session(TypeLoweringEnv::new(self.body_owner(), self.anchor()))?;
        session.lower_generic_args_for(target, args)
    }

    pub(crate) fn resolve_trait_ref(
        &self,
        bound: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<TraitRefLowering>, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        let mut session =
            lowering.session(TypeLoweringEnv::new(self.body_owner(), self.anchor()))?;
        session.lower_trait_ref(bound, self_ty)
    }

    fn body_owner(&self) -> GenericDefRef {
        match self.context.body().owner() {
            BodyOwner::Function(owner) => GenericDefRef::Function(owner),
            BodyOwner::Const(owner) => GenericDefRef::Const(owner),
            BodyOwner::Static(owner) => GenericDefRef::Static(owner),
        }
    }

    fn anchor(&self) -> TypeLoweringAnchor {
        TypeLoweringAnchor::Scope(self.scope)
    }
}
