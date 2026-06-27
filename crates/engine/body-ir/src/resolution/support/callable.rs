//! Callable syntax extracted from selected signatures.
//!
//! Several body-resolution stages need to understand the same source shape:
//! a selected function parameter written as `impl FnOnce(T) -> R`, or a generic
//! parameter `F` whose selected function declares `F: FnOnce(T) -> R`. This
//! module keeps that syntax handling in one place so early pattern propagation
//! and final inference agree on the parameter and return types they see.

use rg_ir_model::{
    ExprId,
    items::{GenericArg, GenericParams, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::Ty;

use crate::{
    ir::ExprKind,
    resolution::{BodyResolutionContext, TypeRefUseSite, query::TypeRefResolutionQuery},
};

/// Return callable expectations aligned to closure arguments written at a call site.
///
/// This owns the shared call-site setup:
///
/// 1. Use only the unique selected target for the call.
/// 2. Project the selected signature so written params line up with written args.
/// 3. Resolve callable shapes at the selected function use site.
///
/// Most calls do not pass closures, so the helper checks that first and avoids the selected-call
/// work when there is nothing for closure inference to consume. Callers can then decide what part
/// of the expectation matters: pattern propagation uses `params`; final inference uses `return_ty`.
pub(crate) fn callable_arg_expectations<'query, D, I>(
    context: BodyResolutionContext<'query, D, I>,
    call: ExprId,
    args: &[ExprId],
) -> Result<Vec<(ExprId, CallableExpectation)>, PackageStoreError>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    if !args.iter().any(|arg| {
        matches!(
            &context.body().expr_unchecked(*arg).kind,
            ExprKind::Closure { .. }
        )
    }) {
        return Ok(Vec::new());
    }

    let calls = context.calls();
    let Some(target) = calls.target(call)? else {
        return Ok(Vec::new());
    };
    let projection = calls.signature(&target).project(args)?;
    if projection.written_param_refs().len() != args.len() {
        return Ok(Vec::new());
    }

    let resolver = context
        .type_refs(TypeRefUseSite::Function(target.function()))
        .with_subst(projection.subst());
    let mut expectations = Vec::new();
    for (arg, param_ty) in args.iter().copied().zip(projection.written_param_refs()) {
        if !matches!(
            &context.body().expr_unchecked(arg).kind,
            ExprKind::Closure { .. }
        ) {
            continue;
        }
        let Some(param_ty) = param_ty else {
            continue;
        };
        let Some(expectation) = CallableExpectation::from_written_param(
            param_ty,
            projection.function_generics(),
            &resolver,
        )?
        else {
            continue;
        };
        expectations.push((arg, expectation));
    }

    Ok(expectations)
}

/// Parameter and return expectations promised by callable syntax.
///
/// Example: `impl FnOnce(User) -> bool`, or `F` plus `F: FnOnce(User) -> bool`,
/// becomes `params = [User]` and `return_ty = bool`. This is not a closure
/// type; it is the expectation a selected call can push into a closure
/// argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallableExpectation {
    pub(crate) params: Vec<Ty>,
    pub(crate) return_ty: Ty,
}

impl CallableExpectation {
    /// Resolve a selected parameter's callable shape.
    ///
    /// Direct `impl Fn*(...)` params carry the callable syntax inline. Generic
    /// callable params need one extra hop: the selected parameter is `F`, and
    /// the callable syntax lives on `F`'s inline bounds or where predicates.
    fn from_written_param<'query, D, I>(
        ty: &TypeRef,
        generics: Option<&GenericParams>,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        if let Some(expectation) = Self::from_direct_impl_trait(ty, resolver)? {
            return Ok(Some(expectation));
        }

        Self::from_generic_param_bounds(ty, generics, resolver)
    }

    /// Resolve one unambiguous parenthesized `Fn`, `FnMut`, or `FnOnce` bound.
    ///
    /// This intentionally handles the direct argument shape only. Generic
    /// callable params are handled separately after we know which generic
    /// parameter the written argument uses.
    fn from_direct_impl_trait<'query, D, I>(
        ty: &TypeRef,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let TypeRef::ImplTrait(bounds) = ty else {
            return Ok(None);
        };

        let mut candidates = ExpectedUnique::new();
        Self::push_fn_trait_bound_expectations(bounds, resolver, &mut candidates)?;

        Ok(candidates.into_option())
    }

    /// Resolve callable bounds from a generic parameter used as a call argument.
    ///
    /// Example: for `fn visit<T, F: FnOnce(T)>(value: T, f: F)`, the closure
    /// argument lines up with written param `F`. By the time this runs, the call
    /// projection may already know `T = User` from the ordinary `value` arg, so
    /// resolving `F`'s bound through that projection gives the closure param type.
    fn from_generic_param_bounds<'query, D, I>(
        ty: &TypeRef,
        generics: Option<&GenericParams>,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let Some(generics) = generics else {
            return Ok(None);
        };
        let Some(param_name) = ty.type_param_name() else {
            return Ok(None);
        };

        let mut candidates = ExpectedUnique::new();
        for param in &generics.types {
            if param.name.as_str() == param_name.as_str() {
                Self::push_fn_trait_bound_expectations(&param.bounds, resolver, &mut candidates)?;
            }
        }

        for predicate in &generics.where_predicates {
            if let WherePredicate::Type {
                ty: predicate_ty,
                bounds,
            } = predicate
                && predicate_ty
                    .type_param_name()
                    .is_some_and(|name| name.as_str() == param_name.as_str())
            {
                Self::push_fn_trait_bound_expectations(bounds, resolver, &mut candidates)?;
            }
        }

        Ok(candidates.into_option())
    }

    fn push_fn_trait_bound_expectations<'query, D, I>(
        bounds: &[TypeBound],
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        candidates: &mut ExpectedUnique<Self>,
    ) -> Result<(), PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        for bound in bounds {
            if let TypeBound::Trait(TypeRef::Path(path)) = bound
                && let Some(segment) = path.segments.last()
                && matches!(segment.name.as_str(), "Fn" | "FnMut" | "FnOnce")
                && let [GenericArg::FnTraitArgs { params, ret }] = segment.args.as_slice()
            {
                let params = params
                    .iter()
                    .map(|param| resolver.resolve(param))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_ty = resolver.resolve(ret)?;
                candidates.push(Self { params, return_ty });
            }
        }

        Ok(())
    }
}
