//! Strict canonical impl matching for type-changing operations.

use rg_def_map::DefMapSource;
use rg_ir_model::{ImplRef, TraitApplicability, TraitImplRef};
use rg_semantic_ir::{ImplData, ItemStoreSource};

use crate::{
    AdtTy, Substitution, TraitGoal, TraitSelectionOptions, TraitSelectionQuery, Ty,
    TypePathResolver,
};

use super::ImplMatcher;

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
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

    /// Match and prove the selected trait impl before using an associated type as a real type.
    pub fn trait_impl_structural_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
    ) -> Result<Option<Substitution>, D::Error> {
        // Inference and closure IDs are local to one body, so only stable receiver identities are
        // safe to reuse across the bodies that share this crate-scoped cache.
        let cacheable = receiver_ty
            .args
            .iter()
            .all(|arg| !arg.has_var() && !arg.has_unknown() && !arg.has_closure());
        if cacheable
            && let Some(subst) = self
                .trait_selection_cache
                .structural_trait_match(trait_impl, receiver_ty)
        {
            return Ok(subst);
        }

        let subst = self.uncached_trait_impl_structural_match(trait_impl, receiver_ty)?;
        if cacheable {
            self.trait_selection_cache.remember_structural_trait_match(
                trait_impl,
                receiver_ty.clone(),
                subst.clone(),
            );
        }
        Ok(subst)
    }

    fn uncached_trait_impl_structural_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
    ) -> Result<Option<Substitution>, D::Error> {
        let Some(header) = self.impl_header(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        let Some(trait_ref) = header.trait_ref.clone() else {
            return Ok(None);
        };
        if trait_ref.application.def != trait_impl.trait_ref {
            return Ok(None);
        }
        let Some((subst, applicability)) =
            Self::impl_self_subst(&header, &Ty::adt(receiver_ty.clone()))
        else {
            return Ok(None);
        };
        if applicability != TraitApplicability::Yes {
            return Ok(None);
        }

        let mut application = trait_ref.application;
        application.args = application
            .args
            .iter()
            .map(|arg| subst.apply_arg(arg))
            .collect();
        let goal = TraitGoal {
            application,
            associated_types: trait_ref
                .associated_types
                .into_iter()
                .map(|binding| crate::AssocTypeBinding {
                    associated_ty: binding.associated_ty,
                    ty: subst.apply(&binding.ty),
                })
                .collect(),
        };
        let table = crate::inference::InferenceTable::new();
        let Some(selection) = TraitSelectionQuery::probe_visible_trait_impl(
            &self.item_paths,
            &self.crate_items,
            &goal,
            &table,
            trait_impl,
            &header,
            TraitSelectionOptions::new(),
            &self.trait_selection_cache,
        )?
        else {
            return Ok(None);
        };
        Ok((selection.applicability == TraitApplicability::Yes).then_some(subst))
    }
}
