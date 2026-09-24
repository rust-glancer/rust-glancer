//! Method candidates for editor completion on an owned receiver type.
//!
//! Completion can offer methods from every reachable dereference depth and from both inherent
//! impls and visible traits. It keeps the declarations and their applicability, combining entries
//! when several receiver adjustments expose the same method.

use rg_def_map::DefMapSource;
use rg_ir_model::ScopeId;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{
    Ty,
    autoderef::AutoderefMode,
    lookup::{MemberMethodCandidateRef, MemberMethodOrigin},
};

use crate::resolution::BodyResolutionContext;

/// Collects methods visible from the body scope for an editor's receiver-type query.
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
        let impl_query = self.context.impl_query();
        let mut candidates = Vec::new();
        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, ty)
        {
            let candidate = candidate?;
            let receiver = self
                .context
                .impls()
                .matches_for_receiver_with_functions(scope, candidate.ty())?;
            for function in impl_query.function_candidates_for_matches(receiver.matches(), None)? {
                let function_ref = function.function();
                let Some(function_data) = self.context.item_query().function_data(function_ref)?
                else {
                    continue;
                };
                if !function_data.has_self_receiver()
                    || receiver.saved_inherent_function_is_shadowed(&function, &function_data.name)
                {
                    continue;
                }

                let candidate = match function.into_trait_selection() {
                    Some(selection) => MemberMethodCandidateRef::trait_method(
                        function_ref,
                        selection.applicability,
                    ),
                    None => MemberMethodCandidateRef::inherent(function_ref),
                };
                Self::push_candidate(&mut candidates, candidate);
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
