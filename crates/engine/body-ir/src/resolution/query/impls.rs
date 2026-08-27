//! Current-body and saved-project impl matching for one receiver.
//!
//! Body queries describe the trait declaration surface they need: a named function, a named const,
//! or a broader completion surface. This query merges current-body and saved declarations, then
//! asks `ImplMatcher` to consider impls only for those traits. Inherent impls remain receiver-first.
//!
//! For `value.render()`, the flow is: discover traits declaring `render`, keep only traits in the
//! expression's lexical scope, gather current-body and saved impls of those traits, then prove their
//! full headers against `value`'s type. This prevents a receiver query from opening every trait impl
//! in the crate just because its outer `Self` shape happens to match.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, ScopeId, TraitDefRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::{ReceiverFunctionCandidate, ReceiverImplMatches, Ty, inference::InferenceTable};

use crate::resolution::BodyResolutionContext;

use super::{body_items::BodyLocalInherentItemNames, trait_cache::BodyTraitSurface};

/// Selects which declaration families still need matching for one receiver probe.
///
/// Named method lookup probes inherent methods first. If none expose the requested name, its second
/// probe uses `TraitsOnly`; reopening inherent impls there would repeat work and duplicate the same
/// candidates. Completion and general associated-item lookup need both families together and use
/// `InherentAndTraits`.
#[derive(Clone, Copy)]
enum ReceiverImplLanes {
    /// Current-body and saved inherent impls, followed by selected trait impls.
    InherentAndTraits,
    /// Only selected trait impls, after an earlier inherent probe has settled its lane.
    TraitsOnly,
}

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

/// Adapts body declaration and lexical-scope views to canonical impl matching.
///
/// This query owns the boundary between two sources of declarations: request-local body overlays
/// and persisted project indexes. It chooses a trait surface, applies Rust's lexical trait scope,
/// gathers impl candidates from both sources, and gives the result to `rg_ty::ImplMatcher` for
/// exact header matching.
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
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self.trait_refs_for_surface(scope, BodyTraitSurface::AssociatedItems)?;
        self.matches_for_receiver_with_traits(
            receiver_ty,
            trait_refs.iter().copied(),
            table,
            ReceiverImplLanes::InherentAndTraits,
        )
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
            ReceiverImplLanes::InherentAndTraits,
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
            self.trait_refs_for_surface(scope, BodyTraitSurface::FunctionNamed(name))?;
        self.matches_for_receiver_with_traits(
            receiver_ty,
            trait_refs.iter().copied(),
            table,
            ReceiverImplLanes::InherentAndTraits,
        )
    }

    /// Match trait extension methods after the inherent lane found no named method.
    pub(crate) fn trait_matches_for_receiver_with_function_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs =
            self.trait_refs_for_surface(scope, BodyTraitSurface::FunctionNamed(name))?;
        if trait_refs.is_empty() {
            self.context.trait_cache().record_empty_extension_probe();
            return Ok(BodyReceiverImplMatches {
                receiver_ty: receiver_ty.clone(),
                matches: ReceiverImplMatches::default(),
                local_inherent_item_names: BodyLocalInherentItemNames::default(),
            });
        }
        self.matches_for_receiver_with_traits(
            receiver_ty,
            trait_refs.iter().copied(),
            table,
            ReceiverImplLanes::TraitsOnly,
        )
    }

    /// Match current-body overlays and traits declaring one named associated const.
    pub(crate) fn matches_for_receiver_with_const_name(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        name: &str,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self.trait_refs_for_surface(scope, BodyTraitSurface::ConstNamed(name))?;
        self.matches_for_receiver_with_traits(
            receiver_ty,
            trait_refs.iter().copied(),
            table,
            ReceiverImplLanes::InherentAndTraits,
        )
    }

    /// Match current-body overlays and every trait that can expose a function.
    pub(crate) fn matches_for_receiver_with_functions(
        &self,
        scope: ScopeId,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<BodyReceiverImplMatches, PackageStoreError> {
        let trait_refs = self.trait_refs_for_surface(scope, BodyTraitSurface::Functions)?;
        self.matches_for_receiver_with_traits(
            receiver_ty,
            trait_refs.iter().copied(),
            table,
            ReceiverImplLanes::InherentAndTraits,
        )
    }

    /// Merge body-origin declarations before saved-project declaration indexes.
    ///
    /// For `value.render()`, body-local and saved reverse-name indexes first answer which traits
    /// declare `render`. The combined declaration list is then intersected with the traits visible
    /// at this `scope`. The result still says nothing about `value`'s type; receiver matching starts
    /// only after this name-and-scope filter has selected a small trait universe.
    fn trait_refs_for_surface(
        &self,
        scope: ScopeId,
        surface: BodyTraitSurface<'_>,
    ) -> Result<std::sync::Arc<UniqueVec<TraitDefRef>>, PackageStoreError> {
        self.context
            .trait_cache()
            .surface_or_try_init(scope, surface, || {
                let body_items = self.context.body_local_items();
                let item_lookup = self.context.item_lookup_query();
                let (body_traits, saved_traits) = match surface {
                    BodyTraitSurface::AssociatedItems => (
                        body_items.traits_with_associated_items()?,
                        item_lookup.traits_with_associated_items(),
                    ),
                    BodyTraitSurface::Functions => (
                        body_items.traits_with_functions()?,
                        item_lookup.traits_with_functions(),
                    ),
                    BodyTraitSurface::FunctionNamed(name) => (
                        body_items.traits_with_function_name(name)?,
                        item_lookup.traits_with_function_name(name),
                    ),
                    BodyTraitSurface::ConstNamed(name) => (
                        body_items.traits_with_const_name(name)?,
                        item_lookup.traits_with_const_name(name),
                    ),
                };
                let mut traits = body_traits.iter().copied().collect::<UniqueVec<_>>();
                traits.extend(saved_traits);

                // Declaration indexes answer which traits *could* provide this item. Rust's
                // implicit lookup then asks the independent lexical question: which of those
                // traits are in method scope at this use site? Both facts are stable for one
                // immutable body, so fixed-point retries reuse this filtered result.
                let traits_in_scope = self.context.traits().traits_in_scope(scope)?;
                Ok(traits
                    .into_iter()
                    .filter(|trait_ref| traits_in_scope.contains(trait_ref))
                    .collect())
            })
    }

    /// Match current-body inherent impls first, then impls of caller-selected traits.
    fn matches_for_receiver_with_traits(
        &self,
        receiver_ty: &Ty,
        trait_refs: impl IntoIterator<Item = TraitDefRef>,
        table: &InferenceTable,
        lanes: ReceiverImplLanes,
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
        if matches!(lanes, ReceiverImplLanes::InherentAndTraits) {
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
        let saved_matches = match lanes {
            ReceiverImplLanes::InherentAndTraits => {
                matcher.matches_for_receiver_with_traits(&receiver_ty, trait_refs, table)?
            }
            ReceiverImplLanes::TraitsOnly => {
                matcher.trait_matches_for_receiver(&receiver_ty, trait_refs, table)?
            }
        };
        matches.extend(saved_matches);
        Ok(BodyReceiverImplMatches {
            receiver_ty,
            matches,
            local_inherent_item_names,
        })
    }
}
