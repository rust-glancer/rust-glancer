//! Body adapter for the shared semantic type lowerer.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ScopeId};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{
    TraitRefLowering, Ty,
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
};

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

    pub(crate) fn resolve_trait_ref(
        &self,
        bound: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<TraitRefLowering>, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        lowering.with_storage(|cx| {
            let self_ty = cx.lower_ty(&self_ty, cx.params(self.body_owner().into()));
            Ok(lowering
                .session(cx, TypeLoweringEnv::new(self.body_owner(), self.anchor()))?
                .lower_trait_ref(bound, self_ty)?
                .map(|bound| bound.raise(cx)))
        })
    }

    fn body_owner(&self) -> GenericDefRef {
        self.context.body().owner().generic_def()
    }

    fn anchor(&self) -> TypeLoweringAnchor {
        TypeLoweringAnchor::Scope(self.scope)
    }
}
