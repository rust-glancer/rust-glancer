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
    lookup::{MemberMethodCandidateRef, MemberMethodOrigin},
    solver::SemanticDeclarations,
};

use super::live::FunctionLookup;
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
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(ty, params);
            let lookup = self.context.live();
            let mut candidates = Vec::new();
            // Completion keeps both origins at every adjustment. Named inference lookup uses
            // the same candidate operation, but stops at the first depth and prefers inherent items.
            for receiver in table.method_receivers(receiver) {
                for candidate in lookup.function_candidates(
                    scope,
                    receiver,
                    FunctionLookup::Completion,
                    &table,
                )? {
                    Self::push_candidate(&mut candidates, candidate.method_ref());
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
