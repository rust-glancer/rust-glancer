//! Trait-obligation solving that is allowed to interact with body inference.
//!
//! This layer is intentionally between Body IR and `rg_ty::TraitSelectionQuery`: it understands
//! where bounds were written and can commit inference-table changes, but the actual impl matching
//! still lives in the shared type layer.

use rg_ir_model::{
    FunctionRef, ItemOwner,
    hir::items::ImplData,
    items::{GenericArg as ItemGenericArg, GenericParams, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{
    GenericArg, TraitGoal, TraitSelection, TraitSelectionOptions, TraitSelectionQuery, Ty,
    TypeSubst,
    inference::{InferGenericArg, InferTy, InferTypeRefProjector, InferTypeSubst},
};

use crate::resolution::{
    BodyResolutionContext, TypeRefUseSite,
    query::TypeRefResolutionQuery,
    support::{
        CallableTypeRefExpectation, SelectedTraitAssocProjector, SelectedTraitMethodContext,
        self_associated_type_name,
    },
};

use super::{BodyCallableGoalSolver, BodyInferenceCtx};

/// Signature facts from an already-selected call that can expose trait obligations.
///
/// Example: for `let xs = bar.iter().collect::<Vec<_>>()`, call inference has already selected
/// `Iterator::collect`, instantiated its return as `Vec<?T>`, and bound the function generic
/// `B = Vec<?T>`. The input then carries:
/// - `function`: the selected `Iterator::collect` item;
/// - `owner`: the trait owner `Iterator`;
/// - `generics`: collect's params and `where B: FromIterator<Self::Item>`;
/// - `subst`: inference bindings such as `B = Vec<?T>`;
/// - `signature_subst`: ordinary signature substitutions used to resolve written paths;
/// - `selected_self_ty`: the receiver iterator type, such as `Iter<BarItem>`.
pub(super) struct SelectedCallObligationInput<'input> {
    pub(super) function: FunctionRef,
    pub(super) owner: ItemOwner,
    pub(super) generics: &'input GenericParams,
    pub(super) subst: &'input InferTypeSubst,
    pub(super) signature_subst: &'input TypeSubst,
    pub(super) selected_self_ty: Option<&'input Ty>,
}

struct CallableImplWhereObligation {
    self_ty: InferTy,
    params: Vec<InferTy>,
    ret: InferTy,
}

/// Solves bounded trait obligations while preserving inference-table semantics.
pub(super) struct BodyTraitObligationSolver<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Solve obligations exposed by one already-selected generic call.
    ///
    /// Continuing `bar.iter().collect::<Vec<_>>()`, this lowers collect's where-clause into the
    /// goal `Vec<?T>: FromIterator<IterItem>` and commits the resulting `?T = IterItem` only when
    /// exactly one visible impl proves the goal.
    pub(super) fn solve_selected_call(
        &self,
        inference: &mut BodyInferenceCtx,
        input: SelectedCallObligationInput<'_>,
    ) -> Result<(), PackageStoreError> {
        // Stage 1: capture the selected trait method context. This lets later projection read
        // `Self::Item` from the unique receiver impl, while inherent calls and free functions
        // simply proceed without that extra context.
        let selected_trait_method = SelectedTraitMethodContext::from_function(
            self.context,
            input.function,
            input.owner,
            input.selected_self_ty,
        )?;
        let bound_resolver = self
            .context
            .type_refs(TypeRefUseSite::Function(input.function))
            .with_subst(input.signature_subst);

        // Stage 2: solve bounds written directly on type params, such as `fn collect<B: Bound>`.
        // Each unique solution may refine variables in the shared inference table.
        for param in &input.generics.types {
            let Some(subject_ty) = input.subst.type_param(param.name.as_str()) else {
                continue;
            };
            for bound in &param.bounds {
                self.solve_trait_bound_obligation(
                    inference,
                    input.subst,
                    &bound_resolver,
                    selected_trait_method.as_ref(),
                    subject_ty.clone(),
                    bound,
                )?;
            }
        }

        // Stage 3: solve where-predicate obligations, such as `where B: FromIterator<Self::Item>`.
        // The left-hand side may itself need projection before it can become the goal self type.
        for predicate in &input.generics.where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                continue;
            };
            let subject_ty = self.project_obligation_ty(
                inference,
                input.subst,
                &bound_resolver,
                selected_trait_method.as_ref(),
                ty,
            )?;
            for bound in bounds {
                self.solve_trait_bound_obligation(
                    inference,
                    input.subst,
                    &bound_resolver,
                    selected_trait_method.as_ref(),
                    subject_ty.clone(),
                    bound,
                )?;
            }
        }

        Ok(())
    }

    /// Project the subject type of a where-predicate into inference form.
    ///
    /// For `where B: FromIterator<T>`, this returns the current binding for `B`, such as
    /// `Vec<?T>`. For a selected trait method, it can also turn an exact `Self::Item` subject into
    /// the associated type from the uniquely matched receiver impl.
    fn project_obligation_ty(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        ty: &TypeRef,
    ) -> Result<InferTy, PackageStoreError> {
        if let Some(projected_ty) =
            self.project_selected_trait_associated_ty(inference, selected_trait_method, ty)?
        {
            return Ok(projected_ty);
        }

        let resolved_ty = resolver.resolve(ty)?;
        Ok(InferTypeRefProjector::new(subst).ty_from_type_ref(ty, &resolved_ty))
    }

    /// Solve one trait bound after the subject type is already known.
    ///
    /// Example: after `B` is projected to `Vec<?T>`, the bound `FromIterator<Self::Item>` becomes
    /// the goal `Vec<?T>: FromIterator<Item>`. A unique visible impl commits its trial inference
    /// table; zero or ambiguous impls leave the caller's table unchanged.
    fn solve_trait_bound_obligation(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        self_ty: InferTy,
        bound: &TypeBound,
    ) -> Result<(), PackageStoreError> {
        let TypeBound::Trait(bound_ty) = bound else {
            return Ok(());
        };
        if self.solve_callable_syntax_obligation(
            inference,
            subst,
            resolver,
            selected_trait_method,
            &self_ty,
            bound_ty,
        )? {
            return Ok(());
        }

        let Some((trait_ref, resolved_args)) = resolver.resolve_trait_bound(bound_ty)? else {
            return Ok(());
        };
        let TypeRef::Path(bound_path) = bound_ty else {
            return Ok(());
        };
        let Some(segment) = bound_path.segments.last() else {
            return Ok(());
        };
        if segment.args.len() != resolved_args.len() {
            return Ok(());
        }

        let args = segment
            .args
            .iter()
            .zip(&resolved_args)
            .map(|(arg, resolved_arg)| {
                self.project_obligation_generic_arg(
                    inference,
                    subst,
                    selected_trait_method,
                    arg,
                    resolved_arg,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let goal = TraitGoal {
            self_ty,
            trait_ref,
            args,
        };

        // Note: trait solver is not powerful enough to "properly" solve obligations typically
        // seen in closures, like `F: FnOnce(T) -> B` where `B` does not participate in impl
        // header directly (only exposed through callable bounds).
        // However, it is a very common piece of functionality, so we add a "lightweight"
        // solver that would attempt solving it through the closure evidence.
        if BodyCallableGoalSolver::new(self.context).solve_goal(inference, &goal)? {
            return Ok(());
        }

        let selection = self.probe_trait_goal(&goal, inference)?;
        if let ExpectedUnique::One(selection) = selection {
            inference.table = selection.table;
        }

        Ok(())
    }

    /// Turn written `Fn*` bounds into closure evidence before ordinary trait solving.
    ///
    /// The trait solver does not model callable traits deeply enough to prove this on its own yet:
    /// `where F: FnOnce(T) -> R`.
    ///
    /// But selected-call inference may already know that `F` is a particular closure:
    /// `apply(user, |user| user.name())` gives `F = Closure#n`.
    ///
    /// In that case we can project `T` and `R` through the selected-call substitution and apply
    /// the same closure-local goal as the normal trait path:
    /// `Closure#n: FnOnce(User) -> R`.
    fn solve_callable_syntax_obligation(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        self_ty: &InferTy,
        bound_ty: &TypeRef,
    ) -> Result<bool, PackageStoreError> {
        let Some(expectation) = CallableTypeRefExpectation::from_fn_trait_bound(bound_ty) else {
            return Ok(false);
        };
        if !matches!(inference.root_resolved_ty(self_ty), InferTy::Closure(_)) {
            return Ok(false);
        }

        let params = expectation
            .params()
            .iter()
            .map(|param| {
                self.project_obligation_ty(inference, subst, resolver, selected_trait_method, param)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ret = self.project_obligation_ty(
            inference,
            subst,
            resolver,
            selected_trait_method,
            expectation.return_ty(),
        )?;

        BodyCallableGoalSolver::new(self.context)
            .solve_fn_trait_goal(inference, self_ty, &params, &ret)
    }

    /// Project a trait-bound generic argument while preserving inference variables.
    ///
    /// Most args use the ordinary `InferTypeRefProjector`: `Vec<_>` stays `Vec<?T>`. The special
    /// case is `Self::Item` from a selected trait method, which is replaced by the receiver impl's
    /// associated type before matching the bound impl. Simple reference wrappers are preserved, so
    /// `&Self::Item` can still project through bounds like `FnMut(&Self::Item)`.
    fn project_obligation_generic_arg(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferTypeSubst,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        arg: &ItemGenericArg,
        resolved_arg: &GenericArg,
    ) -> Result<InferGenericArg, PackageStoreError> {
        if let (ItemGenericArg::Type(ty), GenericArg::Type(resolved_ty)) = (arg, resolved_arg) {
            let projected_ty = match self.project_selected_trait_associated_ty(
                inference,
                selected_trait_method,
                ty,
            )? {
                Some(ty) => ty,
                None => InferTypeRefProjector::new(subst).ty_from_type_ref(ty, resolved_ty),
            };
            return Ok(InferGenericArg::Type(Box::new(projected_ty)));
        }

        Ok(InferTypeRefProjector::new(subst).generic_arg_from_arg(arg, resolved_arg))
    }

    /// Replace `Self::Assoc` with the associated type from a unique receiver impl.
    ///
    /// Example: for `Iterator::collect` on `Iter<User>`, this probes `Iter<User>: Iterator` and
    /// reads `type Item = User` from the unique impl. Ambiguous receiver impls return `None` so
    /// the outer obligation remains unsolved instead of guessing.
    fn project_selected_trait_associated_ty(
        &self,
        inference: &mut BodyInferenceCtx,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        ty: &TypeRef,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some(selected_trait_method) = selected_trait_method else {
            return Ok(None);
        };
        if let Some(assoc_name) = self_associated_type_name(ty) {
            return self.project_selected_trait_associated_alias(
                inference,
                selected_trait_method,
                assoc_name,
            );
        }

        // Callable bounds often wrap associated types in references, e.g. `FnMut(&Self::Item)`.
        // This is not autoderef: we only peel written reference wrappers until the inner type can
        // be projected, then rebuild the same reference shape around the projected type.
        if let TypeRef::Reference {
            mutability, inner, ..
        } = ty
            && let Some(inner_ty) = self.project_selected_trait_associated_ty(
                inference,
                Some(selected_trait_method),
                inner,
            )?
        {
            return Ok(Some(InferTy::Reference {
                mutability: *mutability,
                inner: Box::new(inner_ty),
            }));
        }

        Ok(None)
    }

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
    pub(super) fn project_selected_trait_associated_alias(
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
        let Some(projected_ty) =
            assoc_projector.project_associated_type_from_selection(&selection, assoc_name)?
        else {
            return Ok(None);
        };

        let Some(obligations) = self.callable_impl_where_obligations(&selection, &impl_data)?
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

    fn callable_impl_where_obligations(
        &self,
        selection: &TraitSelection,
        impl_data: &ImplData,
    ) -> Result<Option<Vec<CallableImplWhereObligation>>, PackageStoreError> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(selection.trait_impl.impl_ref),
        };
        let resolver = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context));
        let mut obligations = Vec::new();

        for predicate in &impl_data.generics.where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                return Ok(None);
            };
            let self_ty = self.project_impl_obligation_ty(selection, &resolver, ty)?;
            for bound in bounds {
                let TypeBound::Trait(bound_ty) = bound else {
                    return Ok(None);
                };
                let Some(expectation) = CallableTypeRefExpectation::from_fn_trait_bound(bound_ty)
                else {
                    return Ok(None);
                };

                let params = expectation
                    .params()
                    .iter()
                    .map(|param| self.project_impl_obligation_ty(selection, &resolver, param))
                    .collect::<Result<Vec<_>, _>>()?;
                let ret =
                    self.project_impl_obligation_ty(selection, &resolver, expectation.return_ty())?;
                obligations.push(CallableImplWhereObligation {
                    self_ty: self_ty.clone(),
                    params,
                    ret,
                });
            }
        }

        Ok(Some(obligations))
    }

    fn project_impl_obligation_ty(
        &self,
        selection: &TraitSelection,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        ty: &TypeRef,
    ) -> Result<InferTy, PackageStoreError> {
        let resolved_ty = resolver.resolve(ty)?;
        Ok(InferTypeRefProjector::new(&selection.subst).ty_from_type_ref(ty, &resolved_ty))
    }

    /// Probe a trait goal using the target lookup index persisted with Body IR.
    ///
    /// Keeping this as probe mode matters: callers decide when an `ExpectedUnique::One` result is
    /// strong enough to commit the returned inference table.
    fn probe_trait_goal(
        &self,
        goal: &TraitGoal,
        inference: &BodyInferenceCtx,
    ) -> Result<ExpectedUnique<TraitSelection>, PackageStoreError> {
        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        .probe(goal, &inference.table)
    }
}
