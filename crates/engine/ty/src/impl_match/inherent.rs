//! Receiver matching for inherent impls.

use rg_def_map::DefMapSource;
use rg_ir_model::{FunctionRef, ImplRef, ItemOwner, TraitApplicability};
use rg_semantic_ir::{ImplData, ItemStoreSource};

use crate::{AdtTy, Substitution, Ty, TypePathResolver};

use super::ImplMatcher;

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        receiver_ty: &AdtTy,
    ) -> Result<bool, D::Error> {
        let Some(function_data) = self
            .context
            .item_paths()
            .items()
            .function_data(function_ref)?
        else {
            return Ok(false);
        };
        let ItemOwner::Impl(impl_id) = function_data.owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        };
        let Some(impl_data) = self.context.item_paths().items().impl_data(impl_ref)? else {
            return Ok(false);
        };
        self.impl_applies_to_receiver(impl_ref, impl_data, receiver_ty)
    }

    pub fn impl_applies_to_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &AdtTy,
    ) -> Result<bool, D::Error> {
        if !impl_data.resolved_self_ty.is(&receiver_ty.def) {
            return Ok(false);
        }
        Ok(self
            .impl_self_subst_for_impl(impl_ref, &Ty::adt(receiver_ty.clone()))?
            .is_some_and(|(_, applicability)| applicability.is_applicable()))
    }

    /// Match a structural inherent impl without accepting unresolved clauses or uncertain shapes.
    pub fn structural_inherent_impl_subst(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &Ty,
    ) -> Result<Option<Substitution>, D::Error> {
        if impl_data.trait_ref.is_some() {
            return Ok(None);
        }
        let Some(header) = self.impl_header(impl_ref)? else {
            return Ok(None);
        };
        if !header.clauses.is_empty() {
            return Ok(None);
        }
        let Some((subst, applicability)) = Self::impl_self_subst(&header, receiver_ty) else {
            return Ok(None);
        };
        Ok((applicability == TraitApplicability::Yes).then_some(subst))
    }
}
