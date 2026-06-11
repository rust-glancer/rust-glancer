//! Function declaration properties needed while resolving a body.

use rg_ir_model::FunctionRef;
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{Ty, function_generic_shadow_subst};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

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

    /// Returns the explicitly declared return type for a function body, if one was written.
    ///
    /// This is the expected type for `return expr` and the body tail. Functions without `-> T`
    /// are left to ordinary expression typing so this pass does not erase useful invalid-code
    /// facts by forcing an implicit `()`.
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
            return Ok(Some(Ty::self_ty(
                self.context
                    .type_path_query()
                    .self_nominal_tys_for_function(function_ref)?,
            )));
        }

        self.context
            .type_path_query()
            .type_ref(TypeRefUseSite::Function(function_ref))
            .with_subst(&subst)
            .resolve(ret_ty)
            .map(Some)
    }
}
