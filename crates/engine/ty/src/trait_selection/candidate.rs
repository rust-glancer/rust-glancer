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
use rg_semantic_ir::{ItemLookupQuery, ItemStoreSource};
use rg_std::UniqueVec;

use super::matcher::CandidateMatcher;
use super::{TraitGoal, TraitSelectionSession};
use crate::ItemPathQuery;
use crate::inference::{InferenceSubstitution, InferenceTable};

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
    /// For a `Vec<u8>: Marker` goal, this admits direct `impl Marker for Vec<u8>`-shaped headers and
    /// conservative declarations such as `impl<T> Marker for T`, but skips a direct `u32` impl.
    /// [`Self::probe_impl`] still compares the complete canonical header afterwards.
    ///
    /// Matching is deliberately separate: every match owns a cloned trial table, so callers must
    /// consume one candidate before constructing the next instead of retaining one full table per
    /// visible impl. `None` reports inference-scope work exhaustion; an ordinary search with no
    /// candidates returns `Some(empty)`.
    pub(super) fn plausible_impls(
        item_lookup: &ItemLookupQuery<'_>,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Option<UniqueVec<TraitImplRef>> {
        // Narrow the visible impl set by the resolved outer shape of `Self`. The session owns the
        // policy for receivers that have no useful head, so ordinary trait selection and item
        // lookup cannot drift into different candidate universes.
        let self_ty = table.resolve_root_var(goal.self_ty());
        let trait_ref = goal.trait_ref();
        let candidates = session.trait_impl_candidates_for_ty(item_lookup, trait_ref, &self_ty)?;
        let Some(possible_origins) = goal.possible_impl_origins(table) else {
            return Some(candidates);
        };

        // An unresolved impl header still belongs to one known crate. Coherence can reject that
        // identity before we restore and lower its declaration when the crate owns neither the
        // trait nor any nominal type participating in this concrete application.
        let candidate_count = candidates.len();
        let candidates = candidates
            .into_iter()
            .filter(|candidate| {
                possible_origins.contains(&candidate.impl_ref.origin.origin_crate())
            })
            .collect::<UniqueVec<_>>();
        let skipped = candidate_count - candidates.len();
        if skipped > 0 {
            crate::profile::metric::NATIVE_CANDIDATE_COHERENCE_SKIPS.add(skipped as u64);
        }
        Some(candidates)
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
        // Declaration lookup identifies plausible self-type heads. Load the canonical header,
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
        item_lookup: &ItemLookupQuery<'_>,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<Vec<Self>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut candidates = Vec::new();
        let plausible_impls = Self::plausible_impls(item_lookup, session, goal, table)
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
