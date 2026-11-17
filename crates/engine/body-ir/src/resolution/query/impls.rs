//! Body-local and crate-level impl matching for one receiver.
//!
//! Associated-item queries ask for a named const or all associated items. Lexical trait discovery
//! narrows the search, then this query merges body-local and crate-level impls of those traits.
//! Inherent impls remain receiver-first.
//!
//! These queries expose concrete impls to associated-path lookup. Function references, calls,
//! and dot completion also use methods supplied by bounds, so they discover declarations and
//! prove their applicability in a live inference table instead of requiring a concrete impl here.

use rg_def_map::DefMapSource;
use rg_ir_model::{ScopeId, TraitDefRef};
use rg_semantic_ir::{AssociatedItemFilter, SemanticItemSource};
use rg_source::ReadError;
use rg_std::UniqueVec;
use rg_ty::{Ty, lookup::ReceiverImplMatches};

use crate::resolution::{BodyResolutionContext, cache::BodyTraitSurface};

/// Receiver matches visible while resolving the current body.
///
/// `receiver_ty` is the canonical receiver used for every match, including completed nominal
/// generic arguments. `matches` retains substitutions and trait-selection evidence for both
/// body-local and crate-level impls.
pub(crate) struct BodyReceiverImplMatches {
    receiver_ty: Ty,
    matches: ReceiverImplMatches,
}

impl BodyReceiverImplMatches {
    pub(crate) fn receiver_ty(&self) -> &Ty {
        &self.receiver_ty
    }

    pub(crate) fn matches(&self) -> &ReceiverImplMatches {
        &self.matches
    }
}

/// Adapts body declaration and lexical-scope views to canonical impl matching.
///
/// This query owns the boundary between two sources of declarations: body-local declarations
/// and crate-level declarations. It chooses a trait surface, applies Rust's lexical trait scope,
/// gathers impl candidates from both sources, and gives the result to
/// `rg_ty::lookup::ImplQuery` for exact header matching.
pub(crate) struct BodyImplQuery<'context, 'query, D, I> {
    context: &'context BodyResolutionContext<'query, D, I>,
}

impl<'context, 'query, D, I> BodyImplQuery<'context, 'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    pub(crate) fn new(context: &'context BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Match body-local impls and every trait that can expose an associated item.
    pub(crate) fn matches_for_receiver_with_associated_items(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
    ) -> Result<BodyReceiverImplMatches, ReadError> {
        let trait_refs = self
            .context
            .traits()
            .refs_for_surface(scope, BodyTraitSurface::AssociatedItems)?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs.iter().copied())
    }

    /// Match body-local impls and traits declaring one named associated const.
    pub(crate) fn matches_for_receiver_with_const_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
    ) -> Result<BodyReceiverImplMatches, ReadError> {
        let trait_refs = self
            .context
            .traits()
            .refs_for_surface(scope, BodyTraitSurface::ConstNamed(name))?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs.iter().copied())
    }

    /// Match body-local inherent impls first, then impls of caller-selected traits.
    fn matches_for_receiver_with_traits(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<BodyReceiverImplMatches, ReadError> {
        // Type-only paths can omit nominal arguments. Complete them before matching so every item
        // adapter sees the same canonical receiver and the same impl substitution.
        let receiver_ty = match receiver_ty {
            Ty::Adt(receiver) => Ty::adt(
                self.context
                    .generics()
                    .complete_omitted_nominal_args(receiver)?,
            ),
            _ => receiver_ty.clone(),
        };

        let body_items = self.context.body_local_items();
        let mut inherent_impls = UniqueVec::new();
        let trait_refs = trait_refs.into_iter().collect::<UniqueVec<_>>();
        let mut trait_impls = UniqueVec::new();
        for receiver in receiver_ty.as_adts() {
            inherent_impls.extend(
                body_items
                    .inherent_impls_for_type(receiver.def)?
                    .iter()
                    .copied(),
            );
        }
        trait_impls.extend(body_items.trait_impls_for_traits(trait_refs.as_slice())?);

        let impl_query = self.context.impl_query();
        let mut matches = impl_query.matches_for_receiver_from_impls(
            &receiver_ty,
            inherent_impls,
            trait_impls,
        )?;

        // Local impls supplement the crate index.
        matches.extend(impl_query.matches_for_receiver_with_traits(
            &receiver_ty,
            trait_refs,
            AssociatedItemFilter::All,
        )?);
        Ok(BodyReceiverImplMatches {
            receiver_ty,
            matches,
        })
    }
}
