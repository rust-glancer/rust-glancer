//! Function declaration properties needed while resolving a body.

use rg_ir_model::FunctionRef;
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{ExpectedNominalTyExt, NominalTy, Ty, function_generic_shadow_subst};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

/// Answers function-specific type questions.
pub(crate) struct BodyFunctionQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyFunctionQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return the nominal `Self` type visible from a function's owner context.
    pub(crate) fn self_nominal_ty(
        &self,
        function: FunctionRef,
    ) -> Result<ExpectedUnique<NominalTy>, PackageStoreError> {
        let type_contexts = self.context.type_contexts();
        let context = type_contexts.for_function(function)?;
        type_contexts.nominal_self_ty_for_context(context)
    }

    /// Return the written `-> T`.
    ///
    /// If no arrow was written, return `None` instead of forcing unit here.
    pub(crate) fn declared_return_ty(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(None);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(None);
        };
        let subst = function_generic_shadow_subst(function_data.signature.generics());

        if ret_ty.is_self_type() {
            return Ok(Some(self.self_nominal_ty(function_ref)?.into_self_ty()));
        }

        self.context
            .type_refs(TypeRefUseSite::Function(function_ref))
            .with_subst(&subst)
            .resolve(ret_ty)
            .map(Some)
    }
}
