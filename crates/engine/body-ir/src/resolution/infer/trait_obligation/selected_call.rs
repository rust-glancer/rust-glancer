//! Trait obligations exposed by already-selected calls.
//!
//! Selected calls give us precise signature facts: which function was called, how its generics
//! were instantiated, and which receiver type selected a trait method. This module turns those
//! facts into shallow trait goals and commits only unique solutions back into body inference.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    FunctionRef, ItemOwner,
    items::{GenericParams, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::ItemStoreSource;
use rg_package_store::PackageStoreError;
use rg_ty::{TraitGoal, Ty, TypeSubst, inference::InferenceTypeSubst};

use crate::resolution::{
    TypeRefUseSite,
    query::TypeRefResolutionQuery,
    support::{BodyTypeRefProjector, CallableTypeRefExpectation, SelectedTraitMethodContext},
};

use super::super::BodyInferenceCtx;
use super::{BodyCallableObligation, BodyObligation, BodyTraitObligationSolver};

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
    subst: &'input InferenceTypeSubst,
    signature_subst: &'input TypeSubst,
    selected_self_ty: Option<&'input Ty>,
    selected_self_infer_ty: Option<Ty>,
}

impl<'input> SelectedCallObligationInput<'input> {
    pub(crate) fn new(
        function: FunctionRef,
        owner: ItemOwner,
        generics: &'input GenericParams,
        subst: &'input InferenceTypeSubst,
        signature_subst: &'input TypeSubst,
        selected_self_ty: Option<&'input Ty>,
        selected_self_infer_ty: Option<Ty>,
    ) -> Self {
        Self {
            function,
            owner,
            generics,
            subst,
            signature_subst,
            selected_self_ty,
            selected_self_infer_ty,
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

        // Stage 2: lower and evaluate bounds written directly on type params, such as
        // `fn collect<B: Bound>`. Keeping this as a separate batch preserves the previous evidence
        // order: these obligations may refine inference before where-predicate subjects are
        // projected below.
        let obligations = self.selected_call_type_param_obligations(
            inference,
            input.generics,
            input.subst,
            &bound_resolver,
            selected_trait_method.as_ref(),
            input.selected_self_infer_ty.as_ref(),
        )?;
        self.evaluate_obligations(inference, obligations)?;

        // Stage 3: lower and evaluate where-predicate obligations, such as
        // `where B: FromIterator<Self::Item>`. The left-hand side may need projection before it can
        // become the goal self type.
        let obligations = self.selected_call_where_predicate_obligations(
            inference,
            input.generics,
            input.subst,
            &bound_resolver,
            selected_trait_method.as_ref(),
            input.selected_self_infer_ty.as_ref(),
        )?;
        self.evaluate_obligations(inference, obligations)?;

        Ok(())
    }

    /// Lower bounds written directly on selected-call type parameters.
    ///
    /// These are the simple `fn collect<B: FromIterator<_>>`-style obligations: the subject is
    /// already available from the selected call substitution, so no left-hand-side projection is
    /// needed before building the trait or callable goal.
    fn selected_call_type_param_obligations(
        &self,
        inference: &mut BodyInferenceCtx,
        generics: &GenericParams,
        subst: &InferenceTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        selected_self_infer_ty: Option<&Ty>,
    ) -> Result<Vec<BodyObligation>, PackageStoreError> {
        let mut obligations = Vec::new();

        for param in &generics.types {
            let Some(subject_ty) = subst.type_param(param.name.as_str()) else {
                continue;
            };
            for bound in &param.bounds {
                if let Some(obligation) = self.trait_bound_obligation(
                    inference,
                    subst,
                    resolver,
                    selected_trait_method,
                    selected_self_infer_ty,
                    subject_ty.clone(),
                    bound,
                )? {
                    obligations.push(obligation);
                }
            }
        }

        Ok(obligations)
    }

    /// Lower selected-call `where` predicates after resolving their written subject type.
    ///
    /// A where predicate can put the interesting type on the left-hand side, for example
    /// `where B: FromIterator<Self::Item>`. Before lowering the bounds, we first project that
    /// subject through the selected-call substitution and any available selected-trait context.
    fn selected_call_where_predicate_obligations(
        &self,
        inference: &mut BodyInferenceCtx,
        generics: &GenericParams,
        subst: &InferenceTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        selected_self_infer_ty: Option<&Ty>,
    ) -> Result<Vec<BodyObligation>, PackageStoreError> {
        let mut obligations = Vec::new();

        for predicate in &generics.where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                continue;
            };
            let subject_ty = self.project_selected_call_bound_subject(
                inference,
                subst,
                resolver,
                ty,
                selected_trait_method,
                selected_self_infer_ty,
            )?;
            for bound in bounds {
                if let Some(obligation) = self.trait_bound_obligation(
                    inference,
                    subst,
                    resolver,
                    selected_trait_method,
                    selected_self_infer_ty,
                    subject_ty.clone(),
                    bound,
                )? {
                    obligations.push(obligation);
                }
            }
        }

        Ok(obligations)
    }

    /// Lower one trait bound after the subject type is already known.
    ///
    /// Example: after `B` is projected to `Vec<?T>`, the bound `FromIterator<Self::Item>` becomes
    /// the goal `Vec<?T>: FromIterator<Item>`. Evaluation will later decide whether a unique
    /// visible impl can commit inference-table evidence.
    #[allow(clippy::too_many_arguments)]
    fn trait_bound_obligation(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferenceTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        selected_self_infer_ty: Option<&Ty>,
        self_ty: Ty,
        bound: &TypeBound,
    ) -> Result<Option<BodyObligation>, PackageStoreError> {
        let TypeBound::Trait(bound_ty) = bound else {
            return Ok(None);
        };

        if let Some(obligation) = self.callable_syntax_obligation(
            inference,
            subst,
            resolver,
            selected_trait_method,
            selected_self_infer_ty,
            &self_ty,
            bound_ty,
        )? {
            return Ok(Some(obligation));
        }

        let Some((trait_ref, resolved_args)) = resolver.resolve_trait_bound(bound_ty)? else {
            return Ok(None);
        };
        let TypeRef::Path(bound_path) = bound_ty else {
            return Ok(None);
        };
        let Some(segment) = bound_path.segments.last() else {
            return Ok(None);
        };
        if segment.args.len() != resolved_args.len() {
            return Ok(None);
        }

        let args = {
            let mut self_assoc = |assoc_name: &str| {
                let Some(selected_trait_method) = selected_trait_method else {
                    return Ok(None);
                };
                self.project_selected_trait_associated_alias(
                    inference,
                    selected_trait_method,
                    selected_self_infer_ty,
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

        Ok(Some(BodyObligation::trait_goal(goal)))
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
    #[allow(clippy::too_many_arguments)]
    fn callable_syntax_obligation(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferenceTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        selected_self_infer_ty: Option<&Ty>,
        self_ty: &Ty,
        bound_ty: &TypeRef,
    ) -> Result<Option<BodyObligation>, PackageStoreError> {
        let Some(expectation) = CallableTypeRefExpectation::from_fn_trait_bound(bound_ty) else {
            return Ok(None);
        };
        if !matches!(
            inference.root_resolved_ty(self_ty),
            Ty::Closure(_) | Ty::FunctionItem(_)
        ) {
            return Ok(None);
        }

        let (params, ret) = {
            let mut self_assoc = |assoc_name: &str| {
                let Some(selected_trait_method) = selected_trait_method else {
                    return Ok(None);
                };
                self.project_selected_trait_associated_alias(
                    inference,
                    selected_trait_method,
                    selected_self_infer_ty,
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

        Ok(Some(BodyObligation::callable(BodyCallableObligation::new(
            self_ty.clone(),
            params,
            ret,
        ))))
    }

    fn project_selected_call_bound_subject(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferenceTypeSubst,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        ty: &TypeRef,
        selected_trait_method: Option<&SelectedTraitMethodContext<'_>>,
        selected_self_infer_ty: Option<&Ty>,
    ) -> Result<Ty, PackageStoreError> {
        let mut self_assoc = |assoc_name: &str| {
            let Some(selected_trait_method) = selected_trait_method else {
                return Ok(None);
            };
            self.project_selected_trait_associated_alias(
                inference,
                selected_trait_method,
                selected_self_infer_ty,
                assoc_name,
            )
        };
        let mut projector =
            BodyTypeRefProjector::new(subst, resolver).with_self_associated_ty(&mut self_assoc);
        projector.ty_or_fallback(ty)
    }
}
