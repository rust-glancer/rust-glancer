//! Function declaration properties needed while resolving a body.

use rg_def_map::DefMapSource;
use rg_ir_model::FunctionRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{AdtTy, Ty};

use crate::resolution::BodyResolutionContext;

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

    /// Return the ADT `Self` type visible from a function's owner context.
    pub(crate) fn self_adt_ty(
        &self,
        function: FunctionRef,
    ) -> Result<ExpectedUnique<AdtTy>, PackageStoreError> {
        let type_contexts = self.context.type_contexts();
        let context = type_contexts.for_function(function)?;
        let Some(impl_ref) = context.impl_ref else {
            return Ok(ExpectedUnique::new());
        };
        let item_query = self.context.item_query();
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(ExpectedUnique::new());
        };

        // A method's `self` parameter needs the complete impl type. In particular,
        // `impl<T> Wrapper<T>` must produce `Wrapper<T>`, not a bare `Wrapper` identity.
        let mut self_tys = ExpectedUnique::new();
        if let Some(header) = self.context.impl_matcher().impl_header(impl_ref)? {
            for ty in header.self_ty.as_adts() {
                if impl_data.resolved_self_ty.is(&ty.def) {
                    self_tys.push(ty.clone());
                }
            }
        }

        // Definition lookup is still useful when full type lowering cannot recover an ADT. It
        // gives body resolution a conservative type without inventing generic arguments.
        if self_tys.is_empty()
            && let Some(ty) = impl_data.resolved_self_ty.as_option()
        {
            self_tys.push(AdtTy::bare(*ty));
        }

        Ok(self_tys)
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
        if function_data.signature.ret_ty().is_none() {
            return Ok(None);
        }
        Ok(self
            .context
            .signatures()
            .function(function_ref)?
            .map(|signature| signature.ret))
    }
}
