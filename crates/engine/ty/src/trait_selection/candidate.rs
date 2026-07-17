//! Native discovery of trait impls whose canonical headers fit a goal.
//!
//! A candidate is deliberately weaker than a selection. This module may enumerate visible impls,
//! compare their `Self` and positional trait arguments, and record the resulting substitution. It
//! does not prove impl predicates or associated-type equalities. Callers that need a semantic fact
//! must pass candidates through [`TraitSelectionQuery`](super::TraitSelectionQuery). An
//! editor-facing caller may retain a candidate as `Maybe` only when that semantic pass reports
//! ambiguity or unsupported body-local evidence, never merely because the header matched.

use rg_def_map::DefMapSource;
use rg_ir_model::{TraitApplicability, TraitDefRef, TraitImplRef};
use rg_semantic_ir::{ItemLookupIndex, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};

use super::matcher::{CandidateMatcher, TraitSelfHead};
use super::{TraitGoal, TraitSelectionSession};
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::{ItemPathQuery, Ty, TyContext};

/// One visible impl whose canonical header is compatible with a trait goal.
///
/// `applicability` describes only the header match. `Maybe` normally means that an alias or
/// otherwise incomplete semantic type prevented a definite structural comparison. The impl's
/// predicates have not been submitted to Chalk yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitCandidate {
    pub trait_impl: TraitImplRef,
    pub subst: InferenceSubstitution,
    pub applicability: TraitApplicability,
    /// Trial table after applying direct equality evidence from the impl header.
    pub table: InferenceTable,
}

/// Query for native trait-impl discovery without semantic proof.
pub struct TraitCandidateQuery<'query, D, I> {
    context: TyContext<'query, D, I>,
}

impl<'query, D, I> TraitCandidateQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Choose the unique best header for body-local obligation solving.
    ///
    /// Definite matches outrank speculative ones because this caller is preparing one impl's
    /// predicates for body-owned closure proof, not presenting an editor candidate set.
    pub fn probe_preferred(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitCandidate>, I::Error> {
        let candidates = self.probe_all(goal, table)?;
        let mut definite = ExpectedUnique::new();
        let mut speculative = ExpectedUnique::new();
        for candidate in candidates {
            if candidate.applicability == TraitApplicability::Yes {
                definite.push(candidate);
            } else {
                speculative.push(candidate);
            }
        }

        Ok(if definite.is_empty() {
            speculative
        } else {
            definite
        })
    }

    /// Enumerate every compatible header, including speculative candidates.
    pub(super) fn probe_all(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<Vec<TraitCandidate>, I::Error> {
        Self::probe_all_with(
            self.context.item_paths(),
            self.context.lookup_index(),
            self.context.trait_selection(),
            goal,
            table,
        )
    }

    pub(super) fn probe_all_with(
        item_paths: &ItemPathQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<Vec<TraitCandidate>, I::Error> {
        let self_ty = table.resolve_root_var(goal.self_ty());
        let trait_impls = Self::trait_impl_candidates_with(
            item_paths,
            lookup_index,
            session,
            goal.trait_ref(),
            &self_ty,
        )?;
        let mut candidates = Vec::new();
        for trait_impl in trait_impls {
            if let Some(candidate) =
                Self::probe_trait_impl_with(item_paths, session, goal, table, trait_impl)?
            {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    /// Match one impl already chosen by a visible candidate index.
    fn probe_trait_impl_with(
        item_paths: &ItemPathQuery<'query, D, I>,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
    ) -> Result<Option<TraitCandidate>, I::Error> {
        let Some(header) = session.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
        else {
            return Ok(None);
        };
        Self::probe_visible_trait_impl(item_paths, goal, table, trait_impl, &header, session)
    }

    /// Match a canonical header already loaded by another resolution query.
    fn probe_visible_trait_impl(
        item_paths: &ItemPathQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
        header: &crate::ImplHeader,
        session: &TraitSelectionSession,
    ) -> Result<Option<TraitCandidate>, I::Error> {
        let Some(impl_data) = item_paths.items().impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_trait_ref.is(&goal.trait_ref()) {
            return Ok(None);
        }

        session.remember_impl_header(trait_impl.impl_ref, Some(header.clone()));
        let mut table = table.clone();
        let mut subst = InferenceSubstitution::new();
        let Some(applicability) =
            CandidateMatcher.match_goal(goal, trait_impl, header, &mut table, &mut subst)
        else {
            return Ok(None);
        };

        Ok(applicability.is_applicable().then_some(TraitCandidate {
            trait_impl,
            subst,
            applicability,
            table,
        }))
    }

    fn trait_impl_candidates_with(
        item_paths: &ItemPathQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        trait_ref: TraitDefRef,
        self_ty: &Ty,
    ) -> Result<UniqueVec<TraitImplRef>, I::Error> {
        let Some(visible_impls) = lookup_index.trait_impls_for_trait(trait_ref) else {
            return Ok(UniqueVec::new());
        };
        let Some(self_head) = TraitSelfHead::from_ty(self_ty) else {
            return Ok(visible_impls.clone());
        };
        session.indexed_trait_impl_candidates(item_paths, trait_ref, visible_impls, self_head)
    }
}
