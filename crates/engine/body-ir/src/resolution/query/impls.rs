//! Current-body and saved-project impl matching for one receiver.
//!
//! Associated-item queries describe the declarations they need: a named function, a named const,
//! or all associated items. Lexical trait discovery narrows the search, then this query merges
//! current-body and saved impls of those traits. Inherent impls remain receiver-first.
//!
//! These queries expose concrete impls to associated-path lookup. Calls and dot completion can
//! also use methods supplied by a caller bound, so they discover declarations and prove their
//! applicability in a live inference table instead of requiring a concrete impl here.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, ScopeId, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::{
    Ty,
    lookup::{ReceiverFunctionCandidate, ReceiverImplMatches},
};

use crate::resolution::{
    BodyResolutionContext,
    cache::{BodyLocalInherentItemNames, BodyTraitSurface},
};

/// Receiver matches visible while resolving the current body.
///
/// `receiver_ty` is the canonical receiver used for every match, including completed nominal
/// generic arguments. `matches` retains substitutions and trait-selection evidence. The local name
/// set is carried beside them so an active `impl Widget { fn render(...) }` can suppress the stale
/// saved `render` declaration when consumers expand matched impls into associated items.
pub(crate) struct BodyReceiverImplMatches {
    receiver_ty: Ty,
    matches: ReceiverImplMatches,
    local_inherent_item_names: BodyLocalInherentItemNames,
}

impl BodyReceiverImplMatches {
    pub(crate) fn receiver_ty(&self) -> &Ty {
        &self.receiver_ty
    }

    pub(crate) fn matches(&self) -> &ReceiverImplMatches {
        &self.matches
    }

    /// Return whether a current-body inherent function replaces this saved declaration by name.
    pub(crate) fn saved_inherent_function_is_shadowed(
        &self,
        candidate: &ReceiverFunctionCandidate,
        name: &rg_text::Name,
    ) -> bool {
        candidate.inherent_match().is_some()
            && self.saved_function_name_is_shadowed(candidate.function().origin, name.as_str())
    }

    /// Associated-item collection can also check a function before building its candidate.
    pub(crate) fn saved_function_name_is_shadowed(
        &self,
        function_origin: DefMapRef,
        name: &str,
    ) -> bool {
        function_origin.as_crate_ref().is_some()
            && self.local_inherent_item_names.contains_function(name)
    }

    pub(crate) fn saved_const_name_is_shadowed(&self, const_origin: DefMapRef, name: &str) -> bool {
        const_origin.as_crate_ref().is_some() && self.local_inherent_item_names.contains_const(name)
    }

    pub(crate) fn saved_type_alias_name_is_shadowed(
        &self,
        alias_origin: DefMapRef,
        name: &str,
    ) -> bool {
        alias_origin.as_crate_ref().is_some()
            && self.local_inherent_item_names.contains_type_alias(name)
    }
}

/// Adapts body declaration and lexical-scope views to canonical impl matching.
///
/// This query owns the boundary between two sources of declarations: request-local body overlays
/// and persisted project indexes. It chooses a trait surface, applies Rust's lexical trait scope,
/// gathers impl candidates from both sources, and gives the result to
/// `rg_ty::lookup::ImplQuery` for exact header matching.
pub(crate) struct BodyImplQuery<'context, 'query, D, I> {
    context: &'context BodyResolutionContext<'query, D, I>,
}

impl<'context, 'query, D, I> BodyImplQuery<'context, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: &'context BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Match current-body overlays and every trait that can expose an associated item.
    pub(crate) fn matches_for_receiver_with_associated_items(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self
            .context
            .traits()
            .refs_for_surface(scope, BodyTraitSurface::AssociatedItems)?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs.iter().copied())
    }

    /// Match current-body overlays and traits declaring one named function.
    pub(crate) fn matches_for_receiver_with_function_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self
            .context
            .traits()
            .refs_for_surface(scope, BodyTraitSurface::FunctionNamed(name))?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs.iter().copied())
    }

    /// Match current-body overlays and traits declaring one named associated const.
    pub(crate) fn matches_for_receiver_with_const_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self
            .context
            .traits()
            .refs_for_surface(scope, BodyTraitSurface::ConstNamed(name))?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs.iter().copied())
    }

    /// Match current-body inherent impls first, then impls of caller-selected traits.
    fn matches_for_receiver_with_traits(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
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
        let mut local_inherent_item_names = BodyLocalInherentItemNames::default();
        {
            for receiver in receiver_ty.as_adts() {
                inherent_impls.extend(
                    body_items
                        .inherent_impls_for_type(receiver.def)?
                        .iter()
                        .copied(),
                );
                if let Some(names) = body_items.inherent_item_names_for_type(receiver.def)? {
                    local_inherent_item_names.extend(names);
                }
            }
        }
        trait_impls.extend(body_items.trait_impls_for_traits(trait_refs.as_slice())?);

        let impl_query = self.context.impl_query();
        let mut matches = impl_query.matches_for_receiver_from_impls(
            &receiver_ty,
            inherent_impls,
            trait_impls,
        )?;

        // Current impl items replace saved declarations of the same kind and name. The names come
        // from every current impl with the receiver's nominal key, not only impls that match the
        // completed receiver today: an edited header must still hide its stale saved declaration.
        let saved_matches =
            impl_query.matches_for_receiver_with_traits(&receiver_ty, trait_refs)?;
        matches.extend(saved_matches);
        Ok(BodyReceiverImplMatches {
            receiver_ty,
            matches,
            local_inherent_item_names,
        })
    }
}
