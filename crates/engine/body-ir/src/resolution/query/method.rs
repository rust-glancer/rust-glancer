//! Method candidates for editor completion on an owned receiver type.
//!
//! Completion can offer methods from every reachable dereference depth and from both inherent
//! impls and visible traits. It keeps the declarations and their applicability, combining entries
//! when several receiver adjustments expose the same method.
//!
//! For an out-of-scope trait, the editor can offer a `use` once the solver proves the method
//! applies. This query starts from a small set of matching names to keep that check bounded.

use std::{
    collections::{HashMap, hash_map::Entry},
    time::Instant,
};

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, ScopeId, TraitDefRef};
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_text::Name;
use rg_ty::{
    Ty,
    lookup::{MemberMethodCandidateRef, MemberMethodOrigin},
    solver::{Outcome, SemanticDeclarations, TyShape},
};

use super::live::{FunctionLookup, LiveBodyQuery};
use crate::resolution::BodyResolutionContext;

/// Collects methods visible from the body scope for an editor's receiver-type query.
pub struct BodyMethodQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyMethodQuery<'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return all methods that can be reached from this receiver type.
    pub fn method_candidates_for_ty(
        &self,
        scope: ScopeId,
        ty: &Ty,
        name: Option<&str>,
    ) -> Result<Vec<MemberMethodCandidateRef>, ReadError> {
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(ty, params);
            let lookup = self.context.live();
            let mut candidates = Vec::new();
            // Completion keeps both origins at every dereference step. A named call uses the
            // same declarations, then compares their self parameters to choose one adjustment.
            for receiver in table.method_receivers(receiver) {
                for candidate in lookup.function_candidates(
                    scope,
                    receiver,
                    name.map_or(FunctionLookup::Completion, FunctionLookup::Method),
                    &table,
                )? {
                    Self::push_candidate(&mut candidates, candidate.method_ref());
                }
            }
            Ok(candidates)
        })?
    }

    /// Check out-of-scope methods from one batch of matching names.
    ///
    /// Screening by receiver shape keeps unrelated traits out of the shortlist. Each matching
    /// function with a receiver spends one slot from the budget shared by all batches.
    pub fn extension_methods_for_ty<'name>(
        &self,
        scope: ScopeId,
        ty: &Ty,
        names: impl Iterator<Item = &'name (Name, TraitDefRef)>,
        mut budget: Option<(&mut usize, Instant)>,
    ) -> Result<Vec<FunctionRef>, ReadError> {
        if matches!(&budget, Some((remaining, _)) if **remaining == 0) {
            return Ok(Vec::new());
        }
        let deadline = budget.as_ref().map(|(_, deadline)| *deadline);
        let expired = || deadline.is_some_and(|end| Instant::now() >= end);
        let traits_in_scope = self.context.traits().traits_in_scope(scope)?;
        let items = self.context.item_query();
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(ty, params);
            if matches!(
                receiver.shape(),
                TyShape::Unknown | TyShape::InferVar { .. }
            ) {
                return Ok(Vec::new());
            }
            let cx = table.interner();
            let self_types = table.method_receivers(receiver).collect::<Vec<_>>();

            // A name can match several different traits. Use every autoderef receiver when
            // screening the impl index, so unrelated traits can't fill the shortlist.
            let mut applicable = HashMap::new();
            let mut functions = Vec::new();
            'names: for (name, trait_ref) in names {
                rg_std::check_cancel!(self.context, "extension method names");
                if expired() {
                    break;
                }
                if traits_in_scope.contains(trait_ref) {
                    continue;
                }
                let may_apply = match applicable.entry(*trait_ref) {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(entry) => *entry.insert(declarations.trait_may_apply(
                        &table,
                        *trait_ref,
                        &self_types,
                    )?),
                };
                if !may_apply {
                    continue;
                }
                for (function, data) in items.associated_functions(trait_ref.member_store())? {
                    if data.name == *name && data.has_self_receiver() {
                        functions.push((function, *trait_ref));
                        if let Some((remaining, _)) = &mut budget {
                            **remaining -= 1;
                            if **remaining == 0 {
                                break 'names;
                            }
                        }
                    }
                }
            }
            let mut candidates = Vec::new();
            if functions.is_empty() {
                return Ok(candidates);
            }
            // A proof for `impl Render for Widget` establishes `Self = Widget`.
            // For `render(&self)`, also check that an adjustment produces an `&Widget`.
            let adjustments = table
                .method_receiver_adjustments(&self_types)
                .collect::<Vec<_>>();
            for self_ty in self_types {
                let bound_traits = table.bound_traits(self_ty);
                for &(function, trait_ref) in &functions {
                    rg_std::check_cancel!(self.context, "extension method completion");
                    if expired() {
                        return Ok(candidates);
                    }
                    if candidates.contains(&function) || bound_traits.contains(&trait_ref) {
                        continue;
                    }
                    let Some(candidate) = LiveBodyQuery::<D, I>::trait_function_candidate(
                        trait_ref, function, self_ty, None, &table,
                    ) else {
                        continue;
                    };
                    if candidate.outcome != Outcome::Proven {
                        continue;
                    }
                    let Some(data) = items.function_data(function)? else {
                        continue;
                    };
                    let Some(parameter) = LiveBodyQuery::<D, I>::method_receiver_type(
                        cx,
                        function,
                        data,
                        &candidate.subst,
                        self_ty,
                    ) else {
                        continue;
                    };
                    if adjustments
                        .iter()
                        .any(|&ty| candidate.table.try_unify(ty, parameter).is_ok())
                    {
                        candidates.push(function);
                    }
                }
            }
            Ok(candidates)
        })?
    }

    /// Deduplicate a method candidate and keep the stronger origin.
    fn push_candidate(
        candidates: &mut Vec<MemberMethodCandidateRef>,
        candidate: MemberMethodCandidateRef,
    ) {
        let Some(existing) = candidates
            .iter_mut()
            .find(|existing| existing.function() == candidate.function())
        else {
            candidates.push(candidate);
            return;
        };

        *existing = Self::merge_candidates(*existing, candidate);
    }

    /// Merge duplicate candidates from inherent and trait lookup.
    fn merge_candidates(
        left: MemberMethodCandidateRef,
        right: MemberMethodCandidateRef,
    ) -> MemberMethodCandidateRef {
        match (left.origin(), right.origin()) {
            (MemberMethodOrigin::Inherent, _) => left,
            (_, MemberMethodOrigin::Inherent) => right,
            (
                MemberMethodOrigin::Trait {
                    applicability: left_applicability,
                },
                MemberMethodOrigin::Trait {
                    applicability: right_applicability,
                },
            ) => MemberMethodCandidateRef::trait_method(
                left.function(),
                left_applicability.or(right_applicability),
            ),
        }
    }
}
