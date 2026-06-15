//! Type-resolution context lookup.

use rg_ir_model::FunctionRef;
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{NominalTy, Ty, TypeSubst};

use crate::{ir::BodyOwner, resolution::BodyResolutionContext};

/// Finds the module/impl context used for type resolution.
pub(crate) struct BodyTypeContextQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyTypeContextQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Find the module/impl context that anchors a function signature.
    pub(crate) fn for_function(
        &self,
        function: FunctionRef,
    ) -> Result<TypePathContext, PackageStoreError> {
        let fallback_module = self.context.body().owner_module();
        Ok(self
            .context
            .item_query()
            .type_path_context_for_function(function)?
            .unwrap_or_else(|| TypePathContext::module(fallback_module)))
    }

    /// Find the module/impl context that anchors the current body owner.
    pub(crate) fn for_body_owner(&self) -> Result<TypePathContext, PackageStoreError> {
        let fallback_module = self.context.body().owner_module();
        match self.context.body().owner() {
            BodyOwner::Function(function) => self.for_function(function),
            BodyOwner::Const(const_ref) => {
                let item_query = self.context.item_query();
                let Some(data) = item_query.const_data(const_ref)? else {
                    return Ok(TypePathContext::module(fallback_module));
                };
                item_query
                    .type_path_context_for_owner(const_ref.origin, data.owner)?
                    .map_or_else(|| Ok(TypePathContext::module(fallback_module)), Ok)
            }
            BodyOwner::Static(_) => Ok(TypePathContext::module(fallback_module)),
        }
    }

    /// Resolve type-level `Self` inside an impl context.
    pub(crate) fn nominal_self_ty_for_context(
        &self,
        context: TypePathContext,
    ) -> Result<ExpectedUnique<NominalTy>, PackageStoreError> {
        let Some(impl_ref) = context.impl_ref else {
            return Ok(ExpectedUnique::new());
        };
        let item_query = self.context.item_query();
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(ExpectedUnique::new());
        };

        let resolved = self.context.item_paths().resolve_type_ref(
            &impl_data.self_ty,
            context,
            Ty::Unknown,
            &TypeSubst::new(),
        )?;

        let mut self_tys = ExpectedUnique::new();
        for ty in resolved.as_nominals() {
            if impl_data.resolved_self_ty.is(&ty.def) {
                self_tys.push(ty.clone());
            }
        }

        if self_tys.is_empty() {
            if let Some(ty) = impl_data.resolved_self_ty.as_option() {
                self_tys.push(NominalTy::bare(*ty));
            }
        }

        Ok(self_tys)
    }
}
