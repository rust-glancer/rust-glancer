//! Current-body and saved-project impl matching for one receiver.
//!
//! Body queries describe the trait declaration surface they need: a named function, a named const,
//! or a broader completion surface. This query merges current-body and saved declarations, then
//! asks `ImplMatcher` to consider impls only for those traits. Inherent impls remain receiver-first.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, ScopeId, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::{ReceiverFunctionCandidate, ReceiverImplMatches, Ty, inference::InferenceTable};

use crate::resolution::BodyResolutionContext;

use super::body_items::BodyLocalInherentItemNames;

/// Receiver matches visible while resolving the current body.
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

    /// Completion expands functions from impl items before constructing function candidates.
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

/// Assembles every impl universe visible to the current body.
pub(crate) struct BodyImplQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// Trait declaration surface that may expose an item for one receiver lookup.
enum TraitItemSurface<'name> {
    AssociatedItems,
    Functions,
    FunctionNamed(&'name str),
    ConstNamed(&'name str),
}

impl<'query, D, I> BodyImplQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Match current-body overlays and every trait that can expose an associated item.
    pub(crate) fn matches_for_receiver_with_associated_items(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self.trait_refs_for_surface(scope, TraitItemSurface::AssociatedItems)?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs, table)
    }

    /// Match current-body and saved inherent impls without considering any trait impl.
    pub(crate) fn inherent_matches_for_receiver(
        &self,
        receiver_ty: &Ty,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        self.matches_for_receiver_with_traits(
            receiver_ty,
            UniqueVec::<TraitDefRef>::new(),
            &InferenceTable::new(),
        )
    }

    /// Match current-body overlays and traits declaring one named function.
    pub(crate) fn matches_for_receiver_with_function_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs =
            self.trait_refs_for_surface(scope, TraitItemSurface::FunctionNamed(name))?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs, table)
    }

    /// Match current-body overlays and traits declaring one named associated const.
    pub(crate) fn matches_for_receiver_with_const_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self.trait_refs_for_surface(scope, TraitItemSurface::ConstNamed(name))?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs, table)
    }

    /// Match current-body overlays and every trait that can expose a function.
    pub(crate) fn matches_for_receiver_with_functions(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self.trait_refs_for_surface(scope, TraitItemSurface::Functions)?;
        self.matches_for_receiver_with_traits(receiver_ty, trait_refs, table)
    }

    /// Merge body-origin declarations before saved-project declaration indexes.
    fn trait_refs_for_surface(
        &self,
        scope: ScopeId,
        surface: TraitItemSurface<'_>,
    ) -> Result<UniqueVec<TraitDefRef>, PackageStoreError> {
        let body_items = self.context.body_local_items();
        let item_lookup = self.context.item_lookup_query();
        let (mut body_traits, saved_traits) = match surface {
            TraitItemSurface::AssociatedItems => (
                body_items.traits_with_associated_items()?,
                item_lookup.traits_with_associated_items(),
            ),
            TraitItemSurface::Functions => (
                body_items.traits_with_functions()?,
                item_lookup.traits_with_functions(),
            ),
            TraitItemSurface::FunctionNamed(name) => (
                body_items.traits_with_function_name(name)?,
                item_lookup.traits_with_function_name(name),
            ),
            TraitItemSurface::ConstNamed(name) => (
                body_items.traits_with_const_name(name)?,
                item_lookup.traits_with_const_name(name),
            ),
        };
        body_traits.extend(saved_traits);

        // Declaration indexes answer which traits *could* provide this item. Rust's implicit
        // lookup then asks the independent lexical question: which of those traits are in method
        // scope at this use site? Filtering before impl matching keeps
        // unrelated blanket impls out of both correctness results and completion work.
        let traits_in_scope = self.context.traits().traits_in_scope(scope)?;
        Ok(body_traits
            .into_iter()
            .filter(|trait_ref| traits_in_scope.contains(trait_ref))
            .collect())
    }

    /// Match current-body inherent impls first, then impls of caller-selected traits.
    fn matches_for_receiver_with_traits(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
        table: &InferenceTable,
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
        for receiver in receiver_ty.as_adts() {
            inherent_impls.extend(body_items.inherent_impls_for_type(receiver.def)?);
            local_inherent_item_names
                .extend(body_items.inherent_item_names_for_type(receiver.def)?);
        }
        trait_impls.extend(body_items.trait_impls_for_traits(trait_refs.iter().copied())?);

        let matcher = self.context.impl_matcher();
        let mut matches = matcher.matches_for_receiver_from_impls(
            &receiver_ty,
            inherent_impls,
            trait_impls,
            table,
        )?;

        // Current impl items replace saved declarations of the same kind and name. The names come
        // from every current impl with the receiver's nominal key, not only impls that match the
        // completed receiver today: an edited header must still hide its stale saved declaration.
        matches.extend(matcher.matches_for_receiver_with_traits(
            &receiver_ty,
            trait_refs,
            table,
        )?);
        Ok(BodyReceiverImplMatches {
            receiver_ty,
            matches,
            local_inherent_item_names,
        })
    }
}
