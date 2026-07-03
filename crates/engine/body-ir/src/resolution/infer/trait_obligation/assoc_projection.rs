//! Associated alias projection through selected impl where-clauses.
//!
//! This module handles the second step after trait selection has picked a receiver impl:
//! projecting a selected trait method return such as `Self::Item`. The harder cases are adapter
//! impls where the associated alias mentions an impl-only generic, and a callable where-clause
//! must solve that generic from a closure witness before the alias is useful.

use rg_ir_model::{
    TraitRef,
    hir::items::ImplData,
    items::{GenericParams, TypeBound, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_text::Name;
use rg_ty::{
    TraitGoal, TraitSelection, TraitSelectionCache, TraitSelectionOptions,
    inference::{InferGenericArg, InferTy, InferTypeRefProjector},
};

use crate::resolution::{
    TypeRefUseSite,
    query::TypeRefResolutionQuery,
    support::{
        BodyTypeRefProjector, CallableTypeRefExpectation, ImplPredicateAssocProjector,
        ImplPredicateSubject, ProjectionSupport, SelectedTraitMethodContext,
        impl_projection_predicates, project_unique_support_assoc,
    },
};

use super::super::{BodyCallableGoalSolver, BodyInferenceCtx};
use super::{BodyCallableObligation, BodyTraitObligationSolver};

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
        selected_self_infer_ty: Option<&InferTy>,
        assoc_name: &str,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let assoc_projector = ImplPredicateAssocProjector::new(self.context);
        let goal = TraitGoal {
            self_ty: selected_self_infer_ty
                .cloned()
                .unwrap_or_else(|| InferTy::from_ty(selected_trait_method.selected_self_ty())),
            trait_ref: selected_trait_method.trait_ref(),
            args: Vec::new(),
        };
        let Some(mut selection) = assoc_projector.select_trait_impl(
            &goal,
            &inference.table,
            TraitSelectionOptions::new().caller_solves_impl_predicates(),
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
                &inference.trait_selection_cache,
            )?
        else {
            return Ok(None);
        };
        if obligations.iter().any(|obligation| {
            !matches!(
                selection.table.resolve_root_var(obligation.self_ty()),
                InferTy::Closure(_) | InferTy::FunctionItem(_)
            )
        }) {
            return Ok(None);
        }

        let previous_table = inference.table.clone();
        inference.table = selection.table;
        for obligation in obligations {
            if !BodyCallableGoalSolver::new(self.context).solve_fn_trait_goal(
                inference,
                obligation.self_ty(),
                obligation.params(),
                obligation.ret(),
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
        trait_selection_cache: &TraitSelectionCache,
    ) -> Result<Option<(InferTy, Vec<BodyCallableObligation>)>, PackageStoreError> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(selection.trait_impl.impl_ref),
        };
        let resolver = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context));

        // Split impl predicates into two small families this body-local path understands.
        // `S: Source`-style support predicates help project `S::Item`. Callable predicates such
        // as `F: FnMut(S::Item) -> B` become obligations that can solve impl-only return params
        // from closure bodies.
        let mut supports = Vec::new();
        let mut callable_predicates = Vec::new();
        let Some(predicates) = impl_projection_predicates(&impl_data.generics) else {
            return Ok(None);
        };
        for predicate in predicates {
            if predicate.bounds.is_empty() {
                return Ok(None);
            }

            let callable_expectations = predicate
                .bounds
                .iter()
                .map(|bound| match bound {
                    TypeBound::Trait(bound_ty) => {
                        CallableTypeRefExpectation::from_fn_trait_bound(bound_ty)
                    }
                    TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => None,
                })
                .collect::<Option<Vec<_>>>();
            if let Some(expectations) = callable_expectations {
                callable_predicates.push((predicate.subject, expectations));
                continue;
            }

            let Some(support) = self.impl_where_projection_support(
                selection,
                &resolver,
                &predicate.subject,
                predicate.bounds,
            )?
            else {
                return Ok(None);
            };
            supports.push(support);
        }

        let Some(projected_ty) = self.project_impl_where_ty(
            selection,
            &mut supports,
            &resolver,
            aliased_ty,
            trait_selection_cache,
        )?
        else {
            return Ok(None);
        };

        // Only two predicate families are accepted in this shallow projection path: callable
        // predicates that can solve closure-return generics, and support predicates used to project
        // `S::Item`-style inputs. Anything left unused is a real extra obligation, so we keep the
        // associated type unknown instead of ignoring it.
        let mut obligations = Vec::new();

        for (subject, expectations) in callable_predicates {
            let Some(self_ty) = self.project_impl_where_subject(
                selection,
                &mut supports,
                &resolver,
                &subject,
                trait_selection_cache,
            )?
            else {
                return Ok(None);
            };
            for expectation in expectations {
                let mut params = Vec::new();
                for param in expectation.params() {
                    let Some(param) = self.project_impl_where_ty(
                        selection,
                        &mut supports,
                        &resolver,
                        param,
                        trait_selection_cache,
                    )?
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
                    trait_selection_cache,
                )?
                else {
                    return Ok(None);
                };
                obligations.push(BodyCallableObligation::new(self_ty.clone(), params, ret));
            }
        }

        if supports.iter().any(ProjectionSupport::unused) {
            return Ok(None);
        }

        Ok(Some((projected_ty, obligations)))
    }

    /// Build support evidence from a non-callable impl predicate.
    ///
    /// The accepted shape is a single trait bound on an impl type parameter, such as
    /// `S: Source`. That gives later `S::Item` projection a concrete trait goal to normalize.
    fn impl_where_projection_support(
        &self,
        selection: &TraitSelection,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        subject: &ImplPredicateSubject,
        bounds: &[TypeBound],
    ) -> Result<Option<ProjectionSupport>, PackageStoreError> {
        if let Some(param_name) = subject.type_param_name()
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
            return Ok(Some(ProjectionSupport::new(
                param_name,
                TraitGoal {
                    self_ty,
                    trait_ref,
                    args,
                },
            )));
        }

        Ok(None)
    }

    /// Project the subject of a callable impl predicate.
    ///
    /// Inline `F: FnMut(...)` predicates already name the impl parameter, so the subject is just
    /// the selected substitution for `F`. Where-clause subjects can be richer syntax and need the
    /// normal impl-where type projector.
    fn project_impl_where_subject(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        subject: &ImplPredicateSubject,
        trait_selection_cache: &TraitSelectionCache,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        match subject {
            ImplPredicateSubject::TypeParam(name) => Ok(selection.subst.type_param(name.as_str())),
            ImplPredicateSubject::TypeRef(ty) => {
                self.project_impl_where_ty(selection, supports, resolver, ty, trait_selection_cache)
            }
        }
    }

    fn project_impl_where_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        ty: &TypeRef,
        trait_selection_cache: &TraitSelectionCache,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let subst = selection.subst.clone();
        let mut associated_ty = |param_name: &Name, qualified_trait, assoc_name: &Name| {
            if let Some((trait_ref, args)) = qualified_trait {
                self.project_impl_qualified_generic_associated_ty(
                    selection,
                    supports,
                    param_name,
                    trait_ref,
                    args,
                    assoc_name,
                    trait_selection_cache,
                )
            } else {
                self.project_impl_generic_associated_ty(
                    selection,
                    supports,
                    param_name,
                    assoc_name,
                    trait_selection_cache,
                )
            }
        };
        BodyTypeRefProjector::new(&subst, resolver)
            .with_type_param_associated_ty(&mut associated_ty)
            .ty_if_supported(ty)
    }

    fn project_impl_generic_associated_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        param_name: &Name,
        assoc_name: &Name,
        trait_selection_cache: &TraitSelectionCache,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        // `S::Item` is useful only if some support predicate proves which `Stream` impl applies
        // to `S`. We probe those predicates in the same trial table as the outer impl selection,
        // so any inference refinements stay local until the whole projection succeeds.
        let Some((projection_table, projected_ty)) =
            project_unique_support_assoc(supports, param_name, None, |goal| {
                Ok(self
                    .normalize_assoc_type_in_table(
                        goal,
                        assoc_name.as_str(),
                        &selection.table,
                        trait_selection_cache,
                    )?
                    .map(|projection| (projection.table, projection.ty)))
            })?
        else {
            return Ok(None);
        };
        selection.table = projection_table;
        Ok(Some(projected_ty))
    }

    fn project_impl_qualified_generic_associated_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        param_name: &Name,
        trait_ref: TraitRef,
        args: Vec<InferGenericArg>,
        assoc_name: &Name,
        trait_selection_cache: &TraitSelectionCache,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some((projection_table, projected_ty)) =
            project_unique_support_assoc(supports, param_name, Some((trait_ref, &args)), |goal| {
                Ok(self
                    .normalize_assoc_type_in_table(
                        goal,
                        assoc_name.as_str(),
                        &selection.table,
                        trait_selection_cache,
                    )?
                    .map(|projection| (projection.table, projection.ty)))
            })?
        else {
            return Ok(None);
        };
        selection.table = projection_table;
        Ok(Some(projected_ty))
    }
}
