//! Type-resolution context lookup.

use rg_def_map::DefMapSource;
use rg_ir_model::FunctionRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStoreSource, TypePathContext};

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
}
