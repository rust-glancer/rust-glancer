//! Best-effort trait impl selection.
//!
//! Header compatibility goes through native candidate discovery, then the shared selection query
//! proves the exact candidate's predicates. A definite rejection removes the impl; ambiguity or
//! unsupported current-body evidence remains a useful editor-facing `Maybe` match.

use crate::{
    AdtTy, TraitSelection, TraitSelectionQuery, Ty, TypePathResolver, inference::InferenceTable,
};
use rg_def_map::DefMapSource;
use rg_ir_model::{TraitApplicability, TraitImplRef};
use rg_semantic_ir::ItemStoreSource;

use super::ImplMatcher;

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Return only the yes/maybe/no part of exact trait impl selection.
    pub(crate) fn trait_impl_applicability(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
        table: &InferenceTable,
    ) -> Result<TraitApplicability, D::Error> {
        Ok(self
            .trait_impl_selection_for_ty(trait_impl, &Ty::adt(receiver_ty.clone()), table)?
            .map(|selection| selection.applicability)
            .unwrap_or(TraitApplicability::No))
    }

    /// Match one trait impl against any canonical receiver shape.
    ///
    /// Nominal, primitive, and structural associated-item queries use the same selection result.
    /// For `impl<T> Trait for [T]` and receiver `[User]`, header matching first binds `T -> User`;
    /// predicate proof then decides whether the impl can expose its trait declarations.
    pub(super) fn trait_impl_selection_for_ty(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &Ty,
        table: &InferenceTable,
    ) -> Result<Option<TraitSelection>, D::Error> {
        let item_query = self.context.item_paths().items();
        let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_trait_ref.is(&trait_impl.trait_ref) {
            return Ok(None);
        }

        // A nominal key is a cheap rejection before canonical header matching. Unkeyed impls are
        // structural or blanket candidates and therefore proceed for every receiver shape.
        if let Some(indexed_self_ty) = impl_data.resolved_self_ty.as_option()
            && !receiver_ty
                .as_adts()
                .iter()
                .any(|receiver| receiver.def == *indexed_self_ty)
        {
            return Ok(None);
        }

        // Fixed-point retries see the same receiver representation many times. The conservative
        // fallback lane also presents impls such as `impl<T> Trait for T` to many receiver shapes.
        // Cache this table-independent header comparison, including a negative result. Its
        // inference-scope owner keeps live variables and closure identities request-local.
        let Some(header_match) = self.impl_self_match_for_impl(trait_impl.impl_ref, receiver_ty)?
        else {
            return Ok(None);
        };
        let Some(mut selection) = TraitSelectionQuery::new(self.context.clone())
            .probe_instantiated_impl(trait_impl, &header_match.header, header_match.subst, table)?
        else {
            return Ok(None);
        };

        selection.applicability = header_match.applicability.and(selection.applicability);
        Ok(selection.applicability.is_applicable().then_some(selection))
    }
}
