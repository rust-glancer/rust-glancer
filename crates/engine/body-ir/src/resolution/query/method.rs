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
use rg_ir_model::{ScopeId, TraitDefRef};
use rg_item_tree::GenericArg;
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_std::UniqueVec;
use rg_text::Name;
use rg_ty::{
    Ty,
    lookup::MemberMethodCandidateRef,
    solver::{Outcome, SemanticDeclarations, TyShape},
};

use super::live::{FunctionLookup, LiveBodyQuery, MemberCandidate, MethodPriority};
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
        expected: Option<(&Ty, &[GenericArg])>,
    ) -> Result<Vec<MemberMethodCandidateRef>, ReadError> {
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        let items = self.context.item_query();
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(ty, params);
            let lookup = self.context.live();
            let self_types = table.method_receivers(receiver).collect::<Vec<_>>();
            let adjustments = expected.map(|_| {
                table
                    .method_receiver_adjustments(&self_types)
                    .collect::<Vec<_>>()
            });
            let mut candidates = Vec::new();
            // Keep the broad list of declarations. Compare return types only at a receiver
            // adjustment that could actually call the method.
            for self_ty in self_types {
                for candidate in lookup.function_candidates(
                    scope,
                    self_ty,
                    name.map_or(FunctionLookup::Completion, FunctionLookup::Method),
                    &table,
                )? {
                    let priority = if let Some(receivers) = &adjustments
                        && let Some(data) = items.function_data(candidate.function)?
                    {
                        candidate
                            .match_receiver(data, self_ty, receivers)
                            .map(|(priority, _)| priority)
                    } else {
                        None
                    };
                    Self::push_candidate(
                        &mut candidates,
                        &lookup,
                        scope,
                        &candidate,
                        priority,
                        expected,
                    )?;
                }
            }
            Ok(candidates.into_iter().map(|entry| entry.method).collect())
        })?
    }

    /// Check out-of-scope methods from one batch of matching names.
    ///
    /// Screening by receiver shape keeps unrelated traits out of the shortlist. Each matching
    /// suggestible function with a receiver spends one slot from the budget shared by all batches.
    pub fn extension_methods_for_ty<'name>(
        &self,
        scope: ScopeId,
        ty: &Ty,
        names: impl Iterator<Item = &'name (Name, TraitDefRef)>,
        mut budget: Option<(&mut usize, Instant)>,
        expected: Option<(&Ty, &[GenericArg])>,
    ) -> Result<Vec<MemberMethodCandidateRef>, ReadError> {
        if matches!(&budget, Some((remaining, _)) if **remaining == 0) {
            return Ok(Vec::new());
        }
        let deadline = budget.as_ref().map(|(_, deadline)| *deadline);
        let expired = || deadline.is_some_and(|end| Instant::now() >= end);
        let traits_in_scope = self.context.traits().traits_in_scope(scope)?;
        let use_crate = self.context.body_ref().crate_ref;
        let items = self.context.item_query();
        let lookup = self.context.live();
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
            // For `impl Ext for &u32 { fn borrowed(self) { ... } }`, a `u32` value
            // can call `borrowed()` with `Self = &u32`. Include those borrows in both
            // the impl-index lookup and the trait proof.
            let deref_types = table.method_receivers(receiver).collect::<Vec<_>>();
            let receivers = table
                .method_receiver_adjustments(&deref_types)
                .collect::<UniqueVec<_>>();

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
                        receivers.as_slice(),
                    )?),
                };
                if !may_apply {
                    continue;
                }
                for (function, data) in items.associated_functions(trait_ref.member_store())? {
                    if data.name == *name
                        && data.has_self_receiver()
                        && data
                            .user_facing_attrs
                            .is_suggestible_from(function.store.origin.origin_crate(), use_crate)
                    {
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
            let mut candidates: Vec<RankedMethodCandidate> = Vec::new();
            if functions.is_empty() {
                return Ok(Vec::new());
            }
            // A proof for `impl Render for Widget` establishes `Self = Widget`.
            // For `render(&self)`, also check that an adjustment produces an `&Widget`.
            'receivers: for &self_ty in &receivers {
                let bound_traits = table.bound_traits(self_ty);
                for &(function, trait_ref) in &functions {
                    rg_std::check_cancel!(self.context, "extension method completion");
                    if expired() {
                        break 'receivers;
                    }
                    if bound_traits.contains(&trait_ref)
                        || (expected.is_none()
                            && candidates
                                .iter()
                                .any(|entry| entry.method.function() == function))
                    {
                        continue;
                    }
                    let Some(mut candidate) = LiveBodyQuery::<D, I>::trait_function_candidate(
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
                    let Some((priority, _)) =
                        candidate.match_receiver(data, self_ty, receivers.as_slice())
                    else {
                        continue;
                    };
                    if candidate
                        .table
                        .method_receiver_bounds_hold(function, &mut candidate.subst)
                    {
                        Self::push_candidate(
                            &mut candidates,
                            &lookup,
                            scope,
                            &candidate,
                            Some(priority),
                            expected,
                        )?;
                    }
                }
            }
            // A partial search can keep its suggestions. Later adjustments may still
            // change the winning return type, so omit the boost if the deadline cut it short.
            let finished = !expired();
            Ok(candidates
                .into_iter()
                .map(|entry| {
                    let method = entry.method;
                    if finished {
                        method
                    } else {
                        MemberMethodCandidateRef::new(method.function(), method.origin(), false)
                    }
                })
                .collect())
        })?
    }

    fn push_candidate(
        candidates: &mut Vec<RankedMethodCandidate>,
        lookup: &LiveBodyQuery<'query, D, I>,
        scope: ScopeId,
        candidate: &MemberCandidate<'_>,
        priority: Option<MethodPriority>,
        expected: Option<(&Ty, &[GenericArg])>,
    ) -> Result<(), ReadError> {
        let Some(existing) = candidates
            .iter_mut()
            .find(|entry| entry.method.function() == candidate.function)
        else {
            let method = lookup.method_candidate_ref(scope, candidate, priority.and(expected))?;
            candidates.push(RankedMethodCandidate { method, priority });
            return Ok(());
        };
        let best_priority = existing.priority.into_iter().chain(priority).min();
        // Tied receiver adjustments can expose different instantiations of the same method.
        // Only check the new return type while its adjustment can still earn the boost.
        let previous_matches =
            existing.priority != best_priority || existing.method.return_type_matches();
        let compare = expected
            .filter(|_| priority.is_some() && priority == best_priority && previous_matches);
        let method = lookup.method_candidate_ref(scope, candidate, compare)?;
        let matches = best_priority.is_some()
            && previous_matches
            && (priority != best_priority || method.return_type_matches());
        existing.method = MemberMethodCandidateRef::new(
            method.function(),
            existing.method.origin().min(method.origin()),
            matches,
        );
        existing.priority = best_priority;
        Ok(())
    }
}

/// Keep receiver priority only while collecting methods. Applicability describes the
/// suggestion, while the best receiver adjustment supplies its return type.
struct RankedMethodCandidate {
    method: MemberMethodCandidateRef,
    priority: Option<MethodPriority>,
}
