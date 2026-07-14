//! Callable expectations extracted from canonical selected signatures.

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{Clause, GenericArg, TraitSelectionCache, Ty, inference::InferenceTable};

use crate::{
    ir::ExprKind,
    resolution::{BodyResolutionContext, support::BodyAssocProjector},
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
    trait_selection_cache: TraitSelectionCache,
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
    let projection = calls.signature(&target).project(args)?;
    if projection.written_param_tys().len() != args.len() {
        return Ok(Vec::new());
    }
    let mut expectations = Vec::new();
    for (arg, param_ty) in args.iter().copied().zip(
        projection
            .signature()
            .params
            .iter()
            .skip(target.first_written_param_idx()),
    ) {
        if !matches!(
            &context.body().expr_unchecked(arg).kind,
            ExprKind::Closure { .. }
        ) {
            continue;
        }
        let Some(expectation) = CallableExpectation::from_semantic_param(
            context,
            param_ty,
            &projection,
            trait_selection_cache.clone(),
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
    /// Read the callable application and `Output` equality already lowered for one parameter.
    fn from_semantic_param<'query, D, I>(
        context: BodyResolutionContext<'query, D, I>,
        param_ty: &Ty,
        projection: &crate::resolution::query::CallProjection,
        trait_selection_cache: TraitSelectionCache,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let mut candidates = ExpectedUnique::new();
        let subst = projection.subst();
        for clause in &projection.signature().clauses {
            let Clause::Implemented(application) = clause else {
                continue;
            };
            if application.self_ty() != Some(param_ty)
                || !Self::is_fn_trait(context, application.def)?
            {
                continue;
            }
            let [GenericArg::Type(input)] = &application.args.as_slice()[1..] else {
                continue;
            };
            let Ty::Tuple(params) = input.as_ref() else {
                continue;
            };
            let Some(return_ty) = projection.signature().clauses.iter().find_map(|candidate| {
                let Clause::AliasEq { alias, ty } = candidate else {
                    return None;
                };
                (alias.args == application.args).then_some((alias.associated_ty, ty))
            }) else {
                continue;
            };
            let Some(output_data) = context.item_query().type_alias_data(return_ty.0)? else {
                continue;
            };
            if output_data.name.as_str() != "Output" {
                continue;
            }
            let params = params
                .iter()
                .map(|param| subst.apply(param))
                .map(|param| Self::normalize_ty(context, trait_selection_cache.clone(), param))
                .collect::<Result<Vec<_>, _>>()?;
            let return_ty = Self::normalize_ty(
                context,
                trait_selection_cache.clone(),
                subst.apply(return_ty.1),
            )?;

            // `Fn`, `FnMut`, and `FnOnce` may all describe the same callable contract. Trait
            // identity validates the source clause above, but the closure consumer only needs the
            // resulting parameter and return types, so equivalent contracts are one candidate.
            candidates.push(Self { params, return_ty });
        }
        Ok(candidates.into_option())
    }

    fn is_fn_trait<'query, D, I>(
        context: BodyResolutionContext<'query, D, I>,
        trait_ref: TraitDefRef,
    ) -> Result<bool, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        Ok(context
            .item_query()
            .trait_data(trait_ref)?
            .is_some_and(|data| matches!(data.name.as_str(), "Fn" | "FnMut" | "FnOnce")))
    }

    fn normalize_ty<'query, D, I>(
        context: BodyResolutionContext<'query, D, I>,
        cache: TraitSelectionCache,
        ty: Ty,
    ) -> Result<Ty, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let table = InferenceTable::new();
        let (ty, table) = BodyAssocProjector::new(context)
            .with_cache(cache)
            .normalize_ty(&ty, &table)?;
        Ok(table.finalize(&ty))
    }
}
