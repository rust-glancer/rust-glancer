//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! This is intentionally smaller than a real trait solver. It starts from a resolved trait goal,
//! enumerates visible impls for that trait, and checks only direct header relationships that can be
//! expressed as inference-table unification. More complex bounds are skipped so callers can keep
//! returning unknown or maybe-applicable facts instead of inventing a proof.

mod header;
mod matcher;

use rg_ir_model::{TraitApplicability, TraitImplRef, TraitRef};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery};
use rg_std::ExpectedUnique;

use self::header::SupportedImplHeader;
use self::matcher::CandidateMatcher;
use crate::ItemPathQuery;
use crate::inference::{InferGenericArg, InferTy, InferTypeSubst, InferenceTable};

/// A shallow trait goal such as `Vec<?T>: FromIterator<User>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitGoal {
    pub self_ty: InferTy,
    pub trait_ref: TraitRef,
    pub args: Vec<InferGenericArg>,
}

/// One visible impl whose header is compatible with a trait goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitSelection {
    pub trait_impl: TraitImplRef,
    pub subst: InferTypeSubst,
    pub applicability: TraitApplicability,
    /// Trial table after applying this candidate's direct equality evidence.
    ///
    /// Probe mode returns the table instead of mutating the caller. A later commit mode can adopt
    /// this table only when exactly one candidate survives.
    pub table: InferenceTable,
}

/// Shared bounded trait-selection query.
pub struct TraitSelectionQuery<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: &'query ItemLookupIndex,
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index,
        }
    }

    /// Return the unique visible impl whose simple header is compatible with the goal.
    ///
    /// This is probe mode: every candidate gets a cloned inference table, and the caller's table
    /// remains unchanged even if a candidate would solve variables. Multiple distinct surviving
    /// candidates become `ExpectedUnique::Ambiguous` rather than being exposed as a ranking list.
    pub fn probe(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitSelection>, I::Error> {
        let mut selections = ExpectedUnique::new();
        for trait_impl in self.trait_impl_candidates(goal.trait_ref)? {
            let Some(selection) = self.probe_trait_impl(goal, table, trait_impl)? else {
                continue;
            };
            selections.push(selection);
        }
        Ok(selections)
    }

    /// Probe one already-visible impl against a trait goal.
    ///
    /// Method lookup and completion often start from an impl list that was already filtered by
    /// visibility, receiver indexes, or body-local overlay rules. This entry point lets those
    /// callers reuse the same bounded header matcher without asking trait selection to enumerate
    /// candidates again.
    pub fn probe_trait_impl(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
    ) -> Result<Option<TraitSelection>, I::Error> {
        Self::probe_visible_trait_impl(
            &self.item_paths,
            &self.target_items,
            goal,
            table,
            trait_impl,
        )
    }

    /// Probe one already-visible impl without constructing an owning query wrapper.
    ///
    /// Some callers, such as method lookup, already own borrowed query state and want to reuse the
    /// same candidate matcher for a single impl. This keeps that integration read-only and avoids
    /// adding clone requirements to callers that only need a local proof.
    pub fn probe_visible_trait_impl(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
    ) -> Result<Option<TraitSelection>, I::Error> {
        Self::probe_impl(item_paths, target_items, goal, table, trait_impl)
    }

    fn trait_impl_candidates(&self, trait_ref: TraitRef) -> Result<Vec<TraitImplRef>, I::Error> {
        Ok(self
            .lookup_index
            .trait_impls_for_trait(trait_ref)
            .map(|candidates| candidates.iter().copied().collect())
            .unwrap_or_default())
    }

    fn probe_impl(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
        goal: &TraitGoal,
        table: &InferenceTable,
        trait_impl: TraitImplRef,
    ) -> Result<Option<TraitSelection>, I::Error> {
        let Some(impl_data) = target_items.items().impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_trait_ref.is(&goal.trait_ref)
            || !SupportedImplHeader::accepts(impl_data)
        {
            return Ok(None);
        }

        let mut table = table.clone();
        let mut subst = InferTypeSubst::new();
        let matcher = CandidateMatcher::new(item_paths);
        let Some(applicability) =
            matcher.match_goal(goal, trait_impl, impl_data, &mut table, &mut subst)?
        else {
            return Ok(None);
        };

        Ok(applicability.is_applicable().then_some(TraitSelection {
            trait_impl,
            subst,
            applicability,
            table,
        }))
    }
}

#[cfg(test)]
mod tests;
