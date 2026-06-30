//! Callable syntax extracted from selected signatures.
//!
//! Several body-resolution stages need to understand the same source shape:
//! a selected function parameter written as `impl FnOnce(T) -> R`, or a generic
//! parameter `F` whose selected function declares `F: FnOnce(T) -> R`. This
//! module keeps that syntax handling in one place so early pattern propagation
//! and final inference agree on the parameter and return types they see.
//!
//! This module stops at selected signature syntax. It does not decide whether a
//! closure really implements `Fn`, `FnMut`, or `FnOnce`; final inference turns
//! the parsed syntax into body-local callable goals.

use rg_ir_model::{
    ExprId, FunctionRef, ItemOwner,
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

use super::selected_trait_assoc::{
    SelectedTraitAssocProjector, SelectedTraitMethodContext, self_associated_type_name,
};

/// Return callable expectations aligned to closure arguments written at a call site.
///
/// This owns the shared call-site setup:
///
/// 1. Use only the unique selected target for the call.
/// 2. Project the selected signature so written params line up with written args.
/// 3. Resolve callable shapes at the selected function use site.
pub(crate) fn callable_arg_expectations<'query, D, I>(
    context: BodyResolutionContext<'query, D, I>,
    call: ExprId,
    args: &[ExprId],
) -> Result<Vec<(ExprId, CallableExpectation)>, PackageStoreError>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    // Most call arguments are not closures, so it makes sense to quickly check
    // before doing any work.
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
    let Some(function_data) = context.item_query().function_data(target.function())? else {
        return Ok(Vec::new());
    };
    let projection = calls.signature(&target).project(args)?;
    if projection.written_param_refs().len() != args.len() {
        return Ok(Vec::new());
    }

    let resolver = context
        .type_refs(TypeRefUseSite::Function(target.function()))
        .with_subst(projection.subst());
    let callable_resolver = CallableTypeResolver::new(
        context,
        &resolver,
        target.function(),
        function_data.owner,
        projection.selected_self_ty(),
    )?;
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
            &callable_resolver,
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
        resolver: &CallableTypeResolver<'_, 'query, D, I>,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let Some(expectation) =
            CallableTypeRefExpectation::from_written_param(ty, generics).into_option()
        else {
            return Ok(None);
        };

        let params = expectation
            .params()
            .iter()
            .map(|param| resolver.resolve(param))
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = resolver.resolve(expectation.return_ty())?;
        Ok(Some(Self { params, return_ty }))
    }
}

/// Resolves callable expectation syntax in the selected-call context.
///
/// Most callable expectation types are ordinary type refs, so this is a thin
/// wrapper over `TypeRefResolutionQuery`. The extra selected-trait context is
/// for signatures like `Iterator::map`, where the closure param or return is
/// written as `Self::Item`. Plain type-ref resolution cannot know which
/// receiver impl selected the method, but the call projection can.
pub(crate) struct CallableTypeResolver<'a, 'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    resolver: &'a TypeRefResolutionQuery<'query, D, I>,
    selected_trait_method: Option<SelectedTraitMethodContext<'a>>,
}

impl<'a, 'query, D, I> CallableTypeResolver<'a, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(
        context: BodyResolutionContext<'query, D, I>,
        resolver: &'a TypeRefResolutionQuery<'query, D, I>,
        function: FunctionRef,
        owner: ItemOwner,
        selected_self_ty: Option<&'a Ty>,
    ) -> Result<Self, PackageStoreError> {
        let selected_trait_method =
            SelectedTraitMethodContext::from_function(context, function, owner, selected_self_ty)?;
        Ok(Self {
            context,
            resolver,
            selected_trait_method,
        })
    }

    pub(crate) fn resolve(&self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        if let Some(projected_ty) = self.project_selected_trait_associated_ty(ty)? {
            return Ok(projected_ty);
        }

        self.resolver.resolve(ty)
    }

    fn project_selected_trait_associated_ty(
        &self,
        ty: &TypeRef,
    ) -> Result<Option<Ty>, PackageStoreError> {
        if let Some(assoc_name) = self_associated_type_name(ty) {
            let Some(selected_trait_method) = self.selected_trait_method.as_ref() else {
                return Ok(None);
            };
            return SelectedTraitAssocProjector::new(self.context)
                .project_concrete_ty(selected_trait_method, assoc_name)
                .map(|ty| Some(ty.unwrap_or(Ty::Unknown)));
        }

        // Predicate adapters such as `filter` usually write `FnMut(&Self::Item)`.
        // Keep that reference wrapper instead of smoothing it into just the item type.
        if let TypeRef::Reference {
            mutability, inner, ..
        } = ty
            && let Some(inner_ty) = self.project_selected_trait_associated_ty(inner)?
        {
            return Ok(Some(Ty::reference(*mutability, inner_ty)));
        }

        Ok(None)
    }
}

/// Callable expectation that still points at the written signature syntax.
///
/// This is the shared parser output for parenthesized Fn-trait syntax. For
/// `F: FnMut(&T) -> R`, it keeps `&T` and `R` as written `TypeRef`s. The early
/// pattern pass can resolve those refs into plain `Ty`, while final inference
/// can project them into `InferTy` so a return such as `R` keeps its `?R` slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CallableTypeRefExpectation<'ty> {
    params: &'ty [TypeRef],
    return_ty: &'ty TypeRef,
}

impl<'ty> CallableTypeRefExpectation<'ty> {
    pub(crate) fn params(&self) -> &'ty [TypeRef] {
        self.params
    }

    pub(crate) fn return_ty(&self) -> &'ty TypeRef {
        self.return_ty
    }

    /// Resolve a selected parameter's callable syntax.
    pub(crate) fn from_written_param(
        ty: &'ty TypeRef,
        generics: Option<&'ty GenericParams>,
    ) -> ExpectedUnique<Self> {
        let direct = Self::from_direct_impl_trait(ty);
        if !direct.is_empty() {
            return direct;
        }

        Self::from_generic_param_bounds(ty, generics)
    }

    /// Resolve one parenthesized `Fn`, `FnMut`, or `FnOnce` trait bound.
    pub(crate) fn from_fn_trait_bound(ty: &'ty TypeRef) -> Option<Self> {
        let TypeRef::Path(path) = ty else {
            return None;
        };
        let Some(segment) = path.segments.last() else {
            return None;
        };
        if !matches!(segment.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
            return None;
        }
        let [GenericArg::FnTraitArgs { params, ret }] = segment.args.as_slice() else {
            return None;
        };

        Some(Self {
            params,
            return_ty: ret,
        })
    }

    /// Resolve one unambiguous parenthesized `Fn`, `FnMut`, or `FnOnce` bound.
    ///
    /// This intentionally handles the direct argument shape only. Generic
    /// callable params are handled separately after we know which generic
    /// parameter the written argument uses.
    pub(crate) fn from_direct_impl_trait(ty: &'ty TypeRef) -> ExpectedUnique<Self> {
        let TypeRef::ImplTrait(bounds) = ty else {
            return ExpectedUnique::new();
        };

        let mut candidates = ExpectedUnique::new();
        Self::push_fn_trait_bound_expectations(bounds, &mut candidates);
        candidates
    }

    /// Resolve callable bounds from a generic parameter used as a call argument.
    ///
    /// Example: for `fn visit<T, F: FnOnce(T)>(value: T, f: F)`, the closure
    /// argument lines up with written param `F`. By the time this runs, the call
    /// projection may already know `T = User` from the ordinary `value` arg, so
    /// resolving `F`'s bound through that projection gives the closure param type.
    fn from_generic_param_bounds(
        ty: &'ty TypeRef,
        generics: Option<&'ty GenericParams>,
    ) -> ExpectedUnique<Self> {
        let Some(generics) = generics else {
            return ExpectedUnique::new();
        };
        let Some(param_name) = ty.type_param_name() else {
            return ExpectedUnique::new();
        };

        let mut candidates = ExpectedUnique::new();
        for param in &generics.types {
            if param.name.as_str() == param_name.as_str() {
                Self::push_fn_trait_bound_expectations(&param.bounds, &mut candidates);
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
                Self::push_fn_trait_bound_expectations(bounds, &mut candidates);
            }
        }

        candidates
    }

    fn push_fn_trait_bound_expectations(
        bounds: &'ty [TypeBound],
        candidates: &mut ExpectedUnique<Self>,
    ) {
        for bound in bounds {
            if let TypeBound::Trait(ty) = bound
                && let Some(expectation) = Self::from_fn_trait_bound(ty)
            {
                candidates.push(expectation);
            }
        }
    }
}
