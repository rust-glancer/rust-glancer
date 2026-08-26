//! Current-body and saved-project impl matching for one receiver.
//!
//! Body queries should not each assemble nominal indexes, structural fallbacks, and blanket impl
//! lists. This query adds current-body candidates once, asks `ImplMatcher` for saved-project
//! candidates, and preserves their matching evidence in lookup order.

use rg_def_map::DefMapSource;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_text::Name;
use rg_ty::{ReceiverFunctionCandidate, ReceiverImplMatches, Ty, inference::InferenceTable};

use crate::resolution::BodyResolutionContext;

/// Receiver matches visible while resolving the current body.
pub(crate) struct BodyReceiverImplMatches {
    receiver_ty: Ty,
    matches: ReceiverImplMatches,
    local_inherent_function_names: UniqueVec<Name>,
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
        name: &Name,
    ) -> bool {
        candidate.inherent_match().is_some()
            && self.saved_function_name_is_shadowed(candidate.function().origin, name)
    }

    /// Completion expands functions from impl items before constructing function candidates.
    pub(crate) fn saved_function_name_is_shadowed(
        &self,
        function_origin: rg_ir_model::DefMapRef,
        name: &Name,
    ) -> bool {
        function_origin.as_crate_ref().is_some()
            && self.local_inherent_function_names.contains(name)
    }
}

/// Assembles every impl universe visible to the current body.
pub(crate) struct BodyImplQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyImplQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Match current-body overlays first, then append matching saved-project impls.
    pub(crate) fn matches_for_receiver(
        &self,
        receiver_ty: &Ty,
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
        let mut trait_impls = UniqueVec::new();
        for receiver in receiver_ty.as_adts() {
            inherent_impls.extend(body_items.inherent_impls_for_type(receiver.def)?);
            trait_impls.extend(body_items.trait_impls_for_type(receiver.def)?);
        }
        trait_impls.extend(body_items.trait_impls_without_type_key()?);

        let matcher = self.context.impl_matcher();
        let mut matches = matcher.matches_for_receiver_from_impls(
            &receiver_ty,
            inherent_impls,
            trait_impls,
            table,
        )?;

        // Local inherent functions replace saved inherent functions with the same name. Record the
        // policy beside candidate assembly so methods, associated calls, and completion cannot
        // accidentally implement different overlay rules.
        let item_query = self.context.item_query();
        let mut local_inherent_function_names = UniqueVec::new();
        for impl_match in matches.inherent() {
            let Some(impl_data) = item_query.impl_data(impl_match.impl_ref())? else {
                continue;
            };
            for function in impl_data.functions() {
                if let Some(function_data) = item_query.function_data(function)? {
                    local_inherent_function_names.push(function_data.name.clone());
                }
            }
        }

        matches.extend(matcher.matches_for_receiver(&receiver_ty, table)?);
        Ok(BodyReceiverImplMatches {
            receiver_ty,
            matches,
            local_inherent_function_names,
        })
    }
}
