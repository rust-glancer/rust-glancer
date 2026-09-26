//! Best-effort trait impl selection.
//!
//! The declaration index provides a cheap rejection before the shared solver query matches the
//! header and proves its predicates. A definite rejection removes the impl; ambiguity or
//! unsupported current-body evidence remains a useful editor-facing `Maybe` match.

use rg_def_map::DefMapSource;
use rg_ir_model::TraitImplRef;
use rg_semantic_ir::ItemStoreSource;

use super::ImplQuery;
use crate::{
    Ty,
    solver::SolverScope,
    trait_selection::{TraitSelection, TraitSelectionQuery},
};

impl<'query, D, I, R> ImplQuery<'query, D, I, R>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
    R: SolverScope<Error = D::Error>,
{
    /// Match one trait impl against any canonical receiver shape.
    ///
    /// Nominal, primitive, and structural associated-item queries use the same selection result.
    /// For `impl<T> Trait for [T]` and receiver `[User]`, header matching first binds `T -> User`;
    /// predicate proof then decides whether the impl can expose its trait declarations.
    pub(super) fn trait_impl_selection_for_ty(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &Ty,
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

        let Some(selected) =
            TraitSelectionQuery::with_resolver(self.context.clone(), &self.resolver)
                .select_impl(trait_impl.impl_ref, receiver_ty)?
        else {
            return Ok(None);
        };
        let Some(application) = selected.application else {
            return Ok(None);
        };
        Ok(Some(TraitSelection {
            trait_impl,
            application,
            subst: selected.subst,
            applicability: selected.applicability,
        }))
    }
}
