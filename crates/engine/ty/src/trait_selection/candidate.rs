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

use super::matcher::{CandidateMatcher, TraitSelfHead};
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
    /// Enumerate every compatible header, including speculative candidates.
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
        // Narrow the visible impl set by the resolved outer shape of `Self`. A parameter, alias,
        // or inference variable has no useful head, so those goals must inspect every visible impl.
        let self_ty = table.resolve_root_var(goal.self_ty());
        let trait_ref = goal.trait_ref();
        let Some(visible_impls) = lookup_index.trait_impls_for_trait(trait_ref) else {
            return Ok(Vec::new());
        };
        let trait_impls = match TraitSelfHead::from_ty(&self_ty) {
            Some(self_head) => session.indexed_trait_impl_candidates(
                item_paths,
                trait_ref,
                visible_impls,
                self_head,
            )?,
            None => visible_impls.collect(),
        };

        let mut candidates = Vec::new();
        for trait_impl in trait_impls {
            // The candidate index identifies plausible self-type heads. Load the canonical
            // header, then confirm that its declaration still names the requested trait before
            // matching the full application and collecting equality evidence.
            let Some(header) =
                session.impl_header_with(item_paths, item_paths, trait_impl.impl_ref)?
            else {
                continue;
            };
            let Some(impl_data) = item_paths.items().impl_data(trait_impl.impl_ref)? else {
                continue;
            };
            if !impl_data.resolved_trait_ref.is(&goal.trait_ref()) {
                continue;
            }

            let mut trial_table = table.clone();
            let mut subst = InferenceSubstitution::new();
            let Some(applicability) = CandidateMatcher.match_goal(
                goal,
                trait_impl,
                &header,
                &mut trial_table,
                &mut subst,
            ) else {
                continue;
            };
            if applicability.is_applicable() {
                candidates.push(Self {
                    trait_impl,
                    subst,
                    applicability,
                    table: trial_table,
                });
            }
        }
        Ok(candidates)
    }
}
