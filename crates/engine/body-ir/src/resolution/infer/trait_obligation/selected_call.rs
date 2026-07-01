//! Trait obligations exposed by already-selected calls.
//!
//! Selected calls give us precise signature facts: which function was called, how its generics
//! were instantiated, and which receiver type selected a trait method. This module turns those
//! facts into shallow trait goals and commits only unique solutions back into body inference.

use rg_ir_model::{
    FunctionRef, ItemOwner,
    items::{GenericParams, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{
    TraitGoal, Ty, TypeSubst,
    inference::{InferTy, InferTypeSubst},
};

use crate::resolution::{
    TypeRefUseSite,
    query::TypeRefResolutionQuery,
    support::{CallableTypeRefExpectation, SelectedTraitMethodContext},
};

use super::super::{BodyCallableGoalSolver, BodyInferenceCtx, projection::BodyTypeRefProjector};
use super::BodyTraitObligationSolver;

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
pub(crate) struct SelectedCallObligationInput<'input> {
    function: FunctionRef,
    owner: ItemOwner,
    generics: &'input GenericParams,
    subst: &'input InferTypeSubst,
    signature_subst: &'input TypeSubst,
    selected_self_ty: Option<&'input Ty>,
}

impl<'input> SelectedCallObligationInput<'input> {
    pub(crate) fn new(
        function: FunctionRef,
        owner: ItemOwner,
        generics: &'input GenericParams,
        subst: &'input InferTypeSubst,
        signature_subst: &'input TypeSubst,
        selected_self_ty: Option<&'input Ty>,
    ) -> Self {
        Self {
            function,
            owner,
            generics,
            subst,
            signature_subst,
            selected_self_ty,
        }
    }
}

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Solve obligations exposed by one already-selected generic call.
    ///
    /// Continuing `bar.iter().collect::<Vec<_>>()`, this lowers collect's where-clause into the
    /// goal `Vec<?T>: FromIterator<IterItem>` and commits the resulting `?T = IterItem` only when
    /// exactly one visible impl proves the goal.
    pub(crate) fn solve_selected_call(
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
            let subject_ty = {
                let mut self_assoc = |assoc_name: &str| {
                    let Some(selected_trait_method) = selected_trait_method.as_ref() else {
                        return Ok(None);
                    };
                    self.project_selected_trait_associated_alias(
                        inference,
                        selected_trait_method,
                        assoc_name,
                    )
                };
                let mut projector = BodyTypeRefProjector::new(input.subst, &bound_resolver)
                    .with_self_associated_ty(&mut self_assoc);
                projector.ty_or_fallback(ty)?
            };
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

        let args = {
            let mut self_assoc = |assoc_name: &str| {
                let Some(selected_trait_method) = selected_trait_method else {
                    return Ok(None);
                };
                self.project_selected_trait_associated_alias(
                    inference,
                    selected_trait_method,
                    assoc_name,
                )
            };
            let mut projector =
                BodyTypeRefProjector::new(subst, resolver).with_self_associated_ty(&mut self_assoc);
            segment
                .args
                .iter()
                .zip(&resolved_args)
                .map(|(arg, resolved_arg)| projector.generic_arg_or_fallback(arg, resolved_arg))
                .collect::<Result<Vec<_>, _>>()?
        };
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

        let (params, ret) = {
            let mut self_assoc = |assoc_name: &str| {
                let Some(selected_trait_method) = selected_trait_method else {
                    return Ok(None);
                };
                self.project_selected_trait_associated_alias(
                    inference,
                    selected_trait_method,
                    assoc_name,
                )
            };
            let mut projector =
                BodyTypeRefProjector::new(subst, resolver).with_self_associated_ty(&mut self_assoc);
            let params = expectation
                .params()
                .iter()
                .map(|param| projector.ty_or_fallback(param))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = projector.ty_or_fallback(expectation.return_ty())?;
            (params, ret)
        };

        BodyCallableGoalSolver::new(self.context)
            .solve_fn_trait_goal(inference, self_ty, &params, &ret)
    }
}
