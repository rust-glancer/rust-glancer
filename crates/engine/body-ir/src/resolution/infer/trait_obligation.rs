//! Trait-obligation solving that is allowed to interact with body inference.
//!
//! This layer is intentionally between Body IR and `rg_ty::TraitSelectionQuery`: it understands
//! where bounds were written and can commit inference-table changes, but the actual impl matching
//! still lives in the shared type layer.

use rg_ir_model::{
    AssocItemId, FunctionRef, ItemOwner, TraitRef, TypeAliasRef,
    items::{GenericArg as ItemGenericArg, GenericParams, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{
    GenericArg, TraitGoal, TraitSelection, TraitSelectionQuery, Ty, TypeSubst,
    inference::{InferGenericArg, InferTy, InferTypeRefProjector, InferTypeSubst},
};

use crate::resolution::query::TypeRefResolutionQuery;
use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

use super::BodyInferenceCtx;

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedTraitMethodObligation {
    trait_ref: TraitRef,
    self_ty: InferTy,
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
        let selected_trait_method = self.selected_trait_method_obligation(
            &input.owner,
            input.function.origin,
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
        selected_trait_method: Option<&SelectedTraitMethodObligation>,
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
        selected_trait_method: Option<&SelectedTraitMethodObligation>,
        self_ty: InferTy,
        bound: &TypeBound,
    ) -> Result<(), PackageStoreError> {
        let TypeBound::Trait(bound_ty) = bound else {
            return Ok(());
        };
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
        let selection = self.probe_trait_goal(&goal, inference)?;
        if let ExpectedUnique::One(selection) = selection {
            inference.table = selection.table;
        }

        Ok(())
    }

    /// Build the extra context needed to interpret associated types on selected trait methods.
    ///
    /// Example: if `iter.collect()` selected `Iterator::collect` for receiver `Iter<User>`, later
    /// projection can ask whether exactly one `impl Iterator for Iter<User>` defines `Item`.
    fn selected_trait_method_obligation(
        &self,
        owner: &ItemOwner,
        origin: rg_ir_model::DefMapRef,
        selected_self_ty: Option<&Ty>,
    ) -> Result<Option<SelectedTraitMethodObligation>, PackageStoreError> {
        let ItemOwner::Trait(trait_id) = owner else {
            return Ok(None);
        };
        let Some(selected_self_ty) = selected_self_ty else {
            return Ok(None);
        };

        let trait_ref = TraitRef {
            origin,
            id: *trait_id,
        };
        let Some(trait_data) = self.context.item_query().trait_data(trait_ref)? else {
            return Ok(None);
        };
        if !trait_data.generics.lifetimes.is_empty()
            || !trait_data.generics.types.is_empty()
            || !trait_data.generics.consts.is_empty()
        {
            // TODO: Thread trait-level generic args from method selection once we need
            // associated projections for traits shaped like `Trait<T>`.
            return Ok(None);
        }

        Ok(Some(SelectedTraitMethodObligation {
            trait_ref,
            self_ty: InferTy::from_ty(selected_self_ty),
        }))
    }

    /// Project a trait-bound generic argument while preserving inference variables.
    ///
    /// Most args use the ordinary `InferTypeRefProjector`: `Vec<_>` stays `Vec<?T>`. The special
    /// case is an exact `Self::Item` type arg from a selected trait method, which is replaced by
    /// the receiver impl's associated type before matching the bound impl.
    fn project_obligation_generic_arg(
        &self,
        inference: &mut BodyInferenceCtx,
        subst: &InferTypeSubst,
        selected_trait_method: Option<&SelectedTraitMethodObligation>,
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

    /// Replace exact `Self::Assoc` with the associated type from a unique receiver impl.
    ///
    /// Example: for `Iterator::collect` on `Iter<User>`, this probes `Iter<User>: Iterator` and
    /// reads `type Item = User` from the unique impl. Ambiguous receiver impls return `None` so
    /// the outer obligation remains unsolved instead of guessing.
    fn project_selected_trait_associated_ty(
        &self,
        inference: &mut BodyInferenceCtx,
        selected_trait_method: Option<&SelectedTraitMethodObligation>,
        ty: &TypeRef,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some(selected_trait_method) = selected_trait_method else {
            return Ok(None);
        };
        let Some(assoc_name) = Self::self_associated_type_name(ty) else {
            return Ok(None);
        };

        let goal = TraitGoal {
            self_ty: selected_trait_method.self_ty.clone(),
            trait_ref: selected_trait_method.trait_ref,
            args: Vec::new(),
        };
        let ExpectedUnique::One(selection) = self.probe_trait_goal(&goal, inference)? else {
            return Ok(None);
        };
        let Some(projected_ty) =
            self.project_associated_type_from_selection(&selection, assoc_name)?
        else {
            return Ok(None);
        };

        inference.table = selection.table;
        Ok(Some(projected_ty))
    }

    /// Read one associated type from a selected impl and project it through the impl substitution.
    ///
    /// Example: with `impl<T> Iterator for Iter<T> { type Item = T; }` and the selection subst
    /// `T = User`, reading `Item` returns `User` in inference form.
    fn project_associated_type_from_selection(
        &self,
        selection: &TraitSelection,
        assoc_name: &str,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some(impl_data) = self
            .context
            .item_query()
            .impl_data(selection.trait_impl.impl_ref)?
        else {
            return Ok(None);
        };

        for item in &impl_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: selection.trait_impl.impl_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) =
                self.context.item_query().type_alias_data(type_alias_ref)?
            else {
                continue;
            };
            if type_alias_data.name.as_str() != assoc_name {
                continue;
            }
            let Some(aliased_ty) = type_alias_data.signature.aliased_ty() else {
                continue;
            };

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(selection.trait_impl.impl_ref),
            };
            let resolved_ty = self
                .context
                .type_refs(TypeRefUseSite::OwnerContext(context))
                .resolve(aliased_ty)?;
            return Ok(Some(
                InferTypeRefProjector::new(&selection.subst)
                    .ty_from_type_ref(aliased_ty, &resolved_ty),
            ));
        }

        Ok(None)
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

    fn self_associated_type_name(ty: &TypeRef) -> Option<&str> {
        // TODO: Generalize this replacement to nested shapes such as `Option<Self::Item>` when
        // selected-call obligations need them.
        let TypeRef::Path(path) = ty else {
            return None;
        };
        let [self_segment, assoc_segment] = path.segments.as_slice() else {
            return None;
        };
        if path.absolute
            || self_segment.name.as_str() != "Self"
            || !self_segment.args.is_empty()
            || !assoc_segment.args.is_empty()
        {
            return None;
        }

        Some(assoc_segment.name.as_str())
    }
}
