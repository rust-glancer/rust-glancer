//! Native discovery of trait impls whose canonical headers fit a goal.
//!
//! A candidate is deliberately weaker than a selection. This module may enumerate visible impls,
//! compare their `Self` and positional trait arguments, and record the resulting substitution. It
//! does not prove impl predicates or associated-type equalities. Callers that need a semantic fact
//! must pass candidates through [`TraitSelectionQuery`](super::TraitSelectionQuery). An
//! editor-facing caller may retain a candidate as `Maybe` only when that semantic pass reports
//! genuine ambiguity or an explicit adapter limit, never merely because the header matched.

use rg_def_map::DefMapSource;
use rg_ir_model::{TraitApplicability, TraitImplRef};
use rg_semantic_ir::{ItemLookupIndex, ItemStoreSource};
use rg_std::UniqueVec;

use super::matcher::{CandidateMatcher, TraitSelfHead};
use super::{TraitGoal, TraitSelectionSession, session::TraitWorkKind};
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::{ItemPathQuery, Ty};

/// One visible impl whose canonical header is compatible with a trait goal.
///
/// `applicability` describes only the header match. `Maybe` normally means that an alias or
/// otherwise incomplete semantic type prevented a definite structural comparison. The impl's
/// predicates have not been submitted to Chalk yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TraitCandidate {
    pub(super) trait_impl: TraitImplRef,
    pub(super) subst: InferenceSubstitution,
    pub(super) applicability: TraitApplicability,
    /// Trial table after applying direct equality evidence from the impl header.
    pub(super) table: InferenceTable,
}

impl TraitCandidate {
    /// Enumerate the cheap impl identities that may have a compatible header.
    ///
    /// Matching is deliberately separate: every match owns a cloned trial table, so callers must
    /// consume one candidate before constructing the next instead of retaining one full table per
    /// visible impl.
    pub(super) fn plausible_impls<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // Narrow the visible impl set by the resolved outer shape of `Self`. Parameters and aliases
        // have no useful head, so those established semantic shapes retain every visible impl.
        let self_ty = table.resolve_root_var(goal.self_ty());
        // A trait obligation constrains `Self` after some other inference source gives it a shape;
        // it is not an inverse lookup over today's impl set. Besides being order-dependent Rust
        // semantics, probing a bare slot would clone the whole body table once for every concrete
        // impl before eventually calling the result ambiguous.
        if matches!(self_ty, Ty::InferVar { .. } | Ty::Unknown) {
            return Ok(Some(UniqueVec::new()));
        }
        let trait_ref = goal.trait_ref();
        let Some(visible_impls) = lookup_index.trait_impls_for_trait(trait_ref) else {
            return Ok(Some(UniqueVec::new()));
        };
        match TraitSelfHead::from_ty(&self_ty) {
            Some(self_head) => session.indexed_trait_impl_candidates(
                item_paths,
                trait_ref,
                visible_impls,
                self_head,
            ),
            None => {
                let visible_impls = visible_impls.collect::<Vec<_>>();
                if !session.consume_work(TraitWorkKind::CandidateIndex, visible_impls.len()) {
                    return Ok(None);
                }
                Ok(Some(visible_impls.into_iter().collect()))
            }
        }
    }

    /// Match one plausible impl against the goal using an isolated trial table.
    pub(super) fn probe_impl<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
    ) -> Result<Option<Self>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // The candidate index identifies plausible self-type heads. Load the canonical header,
        // then confirm that its declaration still names the requested trait before matching the
        // full application and collecting equality evidence.
        let Some(header) = session.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
        else {
            return Ok(None);
        };
        let Some(impl_data) = item_paths.items().impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_trait_ref.is(&goal.trait_ref()) {
            return Ok(None);
        }

        let mut trial_table = table.clone();
        let mut subst = InferenceSubstitution::new();
        let Some(applicability) =
            CandidateMatcher.match_goal(goal, trait_impl, &header, &mut trial_table, &mut subst)
        else {
            return Ok(None);
        };
        Ok(applicability.is_applicable().then_some(Self {
            trait_impl,
            subst,
            applicability,
            table: trial_table,
        }))
    }

    #[cfg(test)]
    pub(super) fn probe_all<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<Vec<Self>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut candidates = Vec::new();
        let plausible_impls =
            Self::plausible_impls(item_paths, lookup_index, session, goal, table)?
                .expect("unbounded candidate fixture query should not exhaust work");
        for trait_impl in plausible_impls {
            if let Some(candidate) = Self::probe_impl(item_paths, session, goal, table, trait_impl)?
            {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }
}
