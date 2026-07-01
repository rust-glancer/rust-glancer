//! Associated alias projection through selected impl where-clauses.
//!
//! This module handles the second step after trait selection has picked a receiver impl:
//! projecting a selected trait method return such as `Self::Item`. The harder cases are adapter
//! impls where the associated alias mentions an impl-only generic, and a callable where-clause
//! must solve that generic from a closure witness before the alias is useful.

use rg_ir_model::{
    hir::items::ImplData,
    items::{GenericParams, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_text::Name;
use rg_ty::{
    TraitGoal, TraitSelection, TraitSelectionOptions,
    inference::{InferTy, InferTypeRefProjector},
};

use crate::resolution::{
    TypeRefUseSite,
    query::TypeRefResolutionQuery,
    support::{
        CallableTypeRefExpectation, SelectedTraitAssocProjector, SelectedTraitMethodContext,
    },
};

use super::super::{BodyCallableGoalSolver, BodyInferenceCtx, projection::BodyTypeRefProjector};
use super::BodyTraitObligationSolver;

struct CallableImplWhereObligation {
    self_ty: InferTy,
    params: Vec<InferTy>,
    ret: InferTy,
}

/// Non-callable where-predicate that exists to project an associated type used by a callable one.
///
/// Example: in `S: Stream, F: FnMut(S::Item) -> B`, the `S: Stream` predicate lets us resolve
/// `S::Item` before applying the callable predicate to the closure witness.
struct ImplWhereProjectionSupport {
    param_name: Name,
    goal: TraitGoal,
    used: bool,
}

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Project `Self::Assoc` from the selected receiver impl, including shallow callable where
    /// clauses that can solve impl-only generics.
    ///
    /// Example:
    ///
    /// ```text
    /// impl<F, R> Produces for Adapter<F>
    /// where
    ///     F: FnOnce() -> R,
    /// {
    ///     type Output = R;
    /// }
    /// ```
    ///
    /// Matching `Adapter<Closure#n>: Produces` binds `F = Closure#n`, but `R` only appears in the
    /// where-clause and alias. We give `R` a fresh slot, solve the callable where-clause from the
    /// closure body, and then project `type Output = R`.
    pub(crate) fn project_selected_trait_associated_alias(
        &self,
        inference: &mut BodyInferenceCtx,
        selected_trait_method: &SelectedTraitMethodContext<'_>,
        assoc_name: &str,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let assoc_projector = SelectedTraitAssocProjector::new(self.context);
        let Some(mut selection) = assoc_projector.select_infer_trait_impl(
            selected_trait_method,
            &inference.table,
            TraitSelectionOptions::new().ignore_where_predicates(),
        )?
        else {
            return Ok(None);
        };
        let Some(impl_data) = self
            .context
            .item_query()
            .impl_data(selection.trait_impl.impl_ref)?
            .cloned()
        else {
            return Ok(None);
        };

        self.bind_missing_impl_type_params(&mut selection, &impl_data.generics);
        let Some((_, aliased_ty)) =
            assoc_projector.associated_type_alias_from_selection(&selection, assoc_name)?
        else {
            return Ok(None);
        };

        let Some((projected_ty, obligations)) = self
            .project_associated_alias_and_callable_impl_where(
                &mut selection,
                &impl_data,
                &aliased_ty,
            )?
        else {
            return Ok(None);
        };
        if obligations.iter().any(|obligation| {
            !matches!(
                selection.table.resolve_root_var(&obligation.self_ty),
                InferTy::Closure(_)
            )
        }) {
            return Ok(None);
        }

        let previous_table = inference.table.clone();
        inference.table = selection.table;
        for obligation in obligations {
            if !BodyCallableGoalSolver::new(self.context).solve_fn_trait_goal(
                inference,
                &obligation.self_ty,
                &obligation.params,
                &obligation.ret,
            )? {
                inference.table = previous_table;
                return Ok(None);
            }
        }

        Ok(Some(projected_ty))
    }

    fn bind_missing_impl_type_params(
        &self,
        selection: &mut TraitSelection,
        generics: &GenericParams,
    ) {
        for param in &generics.types {
            if selection.subst.type_param(param.name.as_str()).is_some() {
                continue;
            }
            let ty = selection.table.new_type_var();
            selection
                .subst
                .push(&mut selection.table, param.name.clone(), ty);
        }
    }

    fn project_associated_alias_and_callable_impl_where(
        &self,
        selection: &mut TraitSelection,
        impl_data: &ImplData,
        aliased_ty: &TypeRef,
    ) -> Result<Option<(InferTy, Vec<CallableImplWhereObligation>)>, PackageStoreError> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(selection.trait_impl.impl_ref),
        };
        let resolver = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context));

        let mut supports = Vec::new();
        let mut callable_predicates = Vec::new();
        for predicate in &impl_data.generics.where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                return Ok(None);
            };
            if bounds.is_empty() {
                return Ok(None);
            }

            let callable_expectations = bounds
                .iter()
                .map(|bound| match bound {
                    TypeBound::Trait(bound_ty) => {
                        CallableTypeRefExpectation::from_fn_trait_bound(bound_ty)
                    }
                    TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => None,
                })
                .collect::<Option<Vec<_>>>();
            if let Some(expectations) = callable_expectations {
                callable_predicates.push((ty, expectations));
                continue;
            }

            let Some(support) =
                self.impl_where_projection_support(selection, &resolver, ty, bounds)?
            else {
                return Ok(None);
            };
            supports.push(support);
        }

        let Some(projected_ty) =
            self.project_impl_where_ty(selection, &mut supports, &resolver, aliased_ty)?
        else {
            return Ok(None);
        };

        // Only two predicate families are accepted in this shallow projection path: callable
        // predicates that can solve closure-return generics, and support predicates used to project
        // `S::Item`-style inputs. Anything left unused is a real extra obligation, so we keep the
        // associated type unknown instead of ignoring it.
        let mut obligations = Vec::new();

        for (ty, expectations) in callable_predicates {
            let Some(self_ty) =
                self.project_impl_where_ty(selection, &mut supports, &resolver, ty)?
            else {
                return Ok(None);
            };
            for expectation in expectations {
                let mut params = Vec::new();
                for param in expectation.params() {
                    let Some(param) =
                        self.project_impl_where_ty(selection, &mut supports, &resolver, param)?
                    else {
                        return Ok(None);
                    };
                    params.push(param);
                }
                let Some(ret) = self.project_impl_where_ty(
                    selection,
                    &mut supports,
                    &resolver,
                    expectation.return_ty(),
                )?
                else {
                    return Ok(None);
                };
                obligations.push(CallableImplWhereObligation {
                    self_ty: self_ty.clone(),
                    params,
                    ret,
                });
            }
        }

        if supports.iter().any(|support| !support.used) {
            return Ok(None);
        }

        Ok(Some((projected_ty, obligations)))
    }

    fn impl_where_projection_support(
        &self,
        selection: &TraitSelection,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        ty: &TypeRef,
        bounds: &[TypeBound],
    ) -> Result<Option<ImplWhereProjectionSupport>, PackageStoreError> {
        if let Some(param_name) = ty.type_param_name()
            && let Some(self_ty) = selection.subst.type_param(param_name.as_str())
            && let [TypeBound::Trait(bound_ty)] = bounds
            && CallableTypeRefExpectation::from_fn_trait_bound(bound_ty).is_none()
            && let Some((trait_ref, resolved_args)) = resolver.resolve_trait_bound(bound_ty)?
            && let TypeRef::Path(bound_path) = bound_ty
            && let Some(segment) = bound_path.segments.last()
            && segment.args.len() == resolved_args.len()
        {
            let args = segment
                .args
                .iter()
                .zip(&resolved_args)
                .map(|(arg, resolved_arg)| {
                    InferTypeRefProjector::new(&selection.subst)
                        .generic_arg_from_arg(arg, resolved_arg)
                })
                .collect();
            return Ok(Some(ImplWhereProjectionSupport {
                param_name,
                goal: TraitGoal {
                    self_ty,
                    trait_ref,
                    args,
                },
                used: false,
            }));
        }

        Ok(None)
    }

    fn project_impl_where_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ImplWhereProjectionSupport],
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        ty: &TypeRef,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let subst = selection.subst.clone();
        let mut associated_ty = |param_name: &Name, assoc_name: &Name| {
            self.project_impl_generic_associated_ty(selection, supports, param_name, assoc_name)
        };
        BodyTypeRefProjector::new(&subst, resolver)
            .with_type_param_associated_ty(&mut associated_ty)
            .ty_if_supported(ty)
    }

    fn project_impl_generic_associated_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ImplWhereProjectionSupport],
        param_name: &Name,
        assoc_name: &Name,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        // `S::Item` is useful only if some support predicate proves which `Stream` impl applies
        // to `S`. We probe those predicates in the same trial table as the outer impl selection,
        // so any inference refinements stay local until the whole projection succeeds.
        let mut candidate = None;
        let assoc_projector = SelectedTraitAssocProjector::new(self.context);

        for (support_idx, support) in supports.iter().enumerate() {
            if support.param_name.as_str() == param_name.as_str()
                && let ExpectedUnique::One(support_selection) =
                    self.probe_trait_goal_in_table(&support.goal, &selection.table)?
                && let Some(projected_ty) = assoc_projector.project_associated_type_from_selection(
                    &support_selection,
                    assoc_name.as_str(),
                )?
            {
                if candidate.is_some() {
                    return Ok(None);
                }
                candidate = Some((support_idx, support_selection, projected_ty));
            }
        }

        let Some((support_idx, support_selection, projected_ty)) = candidate else {
            return Ok(None);
        };
        selection.table = support_selection.table;
        supports[support_idx].used = true;
        Ok(Some(projected_ty))
    }
}
