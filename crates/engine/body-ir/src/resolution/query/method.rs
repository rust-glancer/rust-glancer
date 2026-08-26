//! Method lookup for receiver types.
//!
//! Ref-level member lookup can stop after identifying a function. Body inference also keeps the
//! receiver substitutions and trait-selection evidence needed to instantiate its signature. For
//! `[User; 3].into_iter()`, the selected array impl carries `T = User` and `N = 3`, allowing the
//! return projection to become `array::IntoIter<User, 3>`.

use rg_def_map::DefMapSource;
use rg_ir_model::ScopeId;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{
    AutoderefMode, MemberMethodCandidateRef, MemberMethodOrigin, Ty, inference::InferenceTable,
};

use crate::resolution::BodyResolutionContext;

use super::BodyCallableCandidate;

/// Resolves methods for receiver types.
pub struct BodyMethodQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyMethodQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return all methods that can be reached from this receiver type.
    pub fn method_candidates_for_ty(
        &self,
        scope: ScopeId,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let table = InferenceTable::new();
        let mut candidates = Vec::new();
        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, ty)
        {
            let candidate = candidate?;
            let receiver = self.context.impls().matches_for_receiver_with_functions(
                scope,
                candidate.ty(),
                &table,
            )?;
            for function in matcher.function_candidates_for_matches(receiver.matches(), None)? {
                let Some(function_data) = self
                    .context
                    .item_query()
                    .function_data(function.function())?
                else {
                    continue;
                };
                if !function_data.has_self_receiver()
                    || receiver.saved_inherent_function_is_shadowed(&function, &function_data.name)
                {
                    continue;
                }

                let candidate = match function.trait_selection() {
                    Some(selection) => MemberMethodCandidateRef::trait_method(
                        function.function(),
                        selection.applicability,
                    ),
                    None => MemberMethodCandidateRef::inherent(function.function()),
                };
                Self::push_candidate(&mut candidates, candidate);
            }
        }

        Ok(candidates)
    }

    /// Return named method candidates at the first matching autoderef depth.
    pub(crate) fn named_method_candidates_for_ty(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        method_name: &str,
        table: &InferenceTable,
    ) -> Result<Vec<BodyCallableCandidate>, PackageStoreError> {
        let item_query = self.context.item_query();
        let matcher = self.context.impl_matcher();
        let mut current_depth = None;
        let mut candidates = Vec::new();

        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, receiver_ty)
        {
            let candidate = candidate?;
            // Method calls select the first autoderef depth that has matching methods. Completion
            // can be more generous, but call inference must not mix receiver substitutions across
            // different depths.
            if current_depth.is_some_and(|depth| depth != candidate.depth())
                && !candidates.is_empty()
            {
                return Ok(candidates);
            }
            current_depth = Some(candidate.depth());

            let receiver = self
                .context
                .impls()
                .matches_for_receiver_with_function_name(
                    scope,
                    candidate.ty(),
                    method_name,
                    table,
                )?;
            for function in
                matcher.function_candidates_for_matches(receiver.matches(), Some(method_name))?
            {
                let Some(function_data) = item_query.function_data(function.function())? else {
                    continue;
                };
                if function_data.name != method_name
                    || !function_data.has_self_receiver()
                    || receiver.saved_inherent_function_is_shadowed(&function, &function_data.name)
                {
                    continue;
                }

                let Some(candidate) = BodyCallableCandidate::from_receiver_function(
                    &self.context,
                    receiver.receiver_ty(),
                    function,
                    None,
                )?
                else {
                    continue;
                };
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
        }

        Ok(candidates)
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
