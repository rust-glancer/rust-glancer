//! Callable input expectations used by pattern inference.

use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{Clause, GenericArg, TraitSelectionCache, Ty, inference::InferenceTable};

use crate::{ir::ExprKind, resolution::BodyResolutionContext};

/// Parameter expectations promised by callable syntax.
///
/// For example, `impl FnOnce(User) -> bool` becomes `params = [User]`. The return type is not
/// retained because this pass only pushes expected types into closure parameter patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CallableInputExpectation {
    pub(super) params: Vec<Ty>,
}

impl CallableInputExpectation {
    /// Return callable input expectations aligned to closure arguments at one selected call.
    pub(super) fn for_call<'query, D, I>(
        context: BodyResolutionContext<'query, D, I>,
        call: ExprId,
        args: &[ExprId],
        receiver_ty: Option<&Ty>,
    ) -> Result<Vec<(ExprId, Self)>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        // Most calls have no closure arguments. Avoid signature projection and trait selection in
        // that overwhelmingly common case.
        if !args.iter().any(|arg| {
            matches!(
                &context.body().expr_unchecked(*arg).kind,
                ExprKind::Closure { .. }
            )
        }) {
            return Ok(Vec::new());
        }

        let calls = context.calls();
        let Some(target) = calls.target_with_receiver_ty(call, receiver_ty)? else {
            return Ok(Vec::new());
        };
        let projection = calls.signature(&target).project(args)?;
        let written_params = projection
            .signature()
            .params
            .get(target.first_written_param_idx()..)
            .unwrap_or_default();
        if written_params.len() != args.len() {
            return Ok(Vec::new());
        }

        let cache = context.trait_selection_cache();
        let mut expectations = Vec::new();
        for (arg, param_ty) in args.iter().copied().zip(written_params) {
            if !matches!(
                &context.body().expr_unchecked(arg).kind,
                ExprKind::Closure { .. }
            ) {
                continue;
            }
            let Some(expectation) =
                Self::from_semantic_param(context, param_ty, &projection, &cache)?
            else {
                continue;
            };
            expectations.push((arg, expectation));
        }
        Ok(expectations)
    }

    /// Read the callable input application already lowered for one parameter.
    fn from_semantic_param<'query, D, I>(
        context: BodyResolutionContext<'query, D, I>,
        param_ty: &Ty,
        projection: &crate::resolution::query::CallProjection,
        trait_selection_cache: &TraitSelectionCache,
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
            let params = params
                .iter()
                .map(|param| subst.apply(param))
                .map(|param| Self::normalize_ty(context, trait_selection_cache.clone(), param))
                .collect::<Result<Vec<_>, _>>()?;

            // This syntax-side path has no access to the body's live inference table. Only
            // propagate semantic inputs that the shared solver settled completely; persisting an
            // opaque projection such as `Filter<I, P>::Item` in a binding would prevent the later
            // body-aware obligation pass from replacing it with the projected concrete type.
            if params
                .iter()
                .any(|param| param.has_unknown() || param.has_projection())
            {
                continue;
            }

            // `Fn`, `FnMut`, and `FnOnce` may all describe the same input contract. Trait identity
            // validates the source clause above, while this pass deliberately compares only the
            // parameter facts it can propagate.
            candidates.push(Self { params });
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
        let (ty, table) = context
            .trait_selection_with_cache(cache)
            .normalize_ty(&ty, &table)?;
        Ok(table.finalize(&ty))
    }
}
