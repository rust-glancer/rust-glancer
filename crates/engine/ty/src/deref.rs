//! Trait-backed `Deref` target lookup for autoderef.
//!
//! This module deliberately stays narrow: it recognizes `core::ops::Deref` impls for a known
//! nominal receiver and resolves the impl's associated `Target` type with the receiver substitution.

use rg_ir_model::{Path, PathSegment, TraitImplRef, hir::items::ImplData};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery, TypePathContext,
};
use rg_std::UniqueVec;
use rg_text::Name;

use crate::{
    ImplMatcher, ItemPathQuery, NominalTy, Ty, TypeSubst, associated_type::AssociatedTypeProjector,
};

/// Resolves the associated `Target` type for applicable `core::ops::Deref` impls.
#[derive(Clone)]
pub(crate) struct DerefResolver<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: Option<&'query ItemLookupIndex>,
}

impl<'query, D, I> DerefResolver<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub(crate) fn new(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: Option<&'query ItemLookupIndex>,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index,
        }
    }

    /// Returns all one-step `Deref::Target` types for a known type.
    pub(crate) fn targets_for_ty(&self, ty: &Ty) -> Result<Vec<Ty>, D::Error> {
        // TODO: Add `DerefMut` once receiver contexts carry enough mutability information to
        // distinguish mutable adjustment from shared `Deref`.
        let mut targets = UniqueVec::new();
        for receiver_ty in ty.as_nominals() {
            for target in self.targets_for_nominal(receiver_ty)? {
                targets.push(target);
            }
        }
        Ok(targets.into_vec())
    }

    /// Returns one-step `Deref::Target` types for a nominal receiver.
    ///
    /// For `impl<T> core::ops::Deref for Wrapper<T> { type Target = T; }` and receiver
    /// `Wrapper<User>`, this resolves the target as `User`.
    fn targets_for_nominal(&self, receiver_ty: &NominalTy) -> Result<Vec<Ty>, D::Error> {
        let matcher = ImplMatcher::new(self.item_paths.clone(), self.target_items.clone());
        let item_query = self.item_paths.items();
        let mut targets = UniqueVec::new();
        let trait_impls = match self.lookup_index {
            Some(index) => index.trait_impls_for_type(receiver_ty.def).to_vec(),
            None => self.target_items.trait_impls_for_type(receiver_ty.def)?,
        };

        for trait_impl in trait_impls {
            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };
            if !self.is_core_ops_deref_impl(trait_impl, impl_data)? {
                continue;
            }

            // `Deref` is a real type adjustment, not just an optimistic editor candidate.
            // Require a structural impl-self match so uncertain trait impls cannot change
            // field/method lookup receiver types.
            let Some(subst) = matcher.trait_impl_structural_match(trait_impl, receiver_ty)? else {
                continue;
            };

            let Some(target) = self.target_from_impl(trait_impl, impl_data, &subst)? else {
                continue;
            };
            targets.push(target);
        }

        Ok(targets.into_vec())
    }

    /// Checks whether this trait impl resolved to the canonical `core::ops::Deref` path.
    fn is_core_ops_deref_impl(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
    ) -> Result<bool, D::Error> {
        let path = Path {
            absolute: true,
            segments: vec![
                PathSegment::Name(Name::new("core")),
                PathSegment::Name(Name::new("ops")),
                PathSegment::Name(Name::new("Deref")),
            ],
        };
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(trait_impl.impl_ref),
        };

        AssociatedTypeProjector::new(&self.item_paths, &self.target_items)
            .trait_impl_resolves_to_path(trait_impl, context, &path)
    }

    /// Resolves the `type Target = ...` item declared in a matching `Deref` impl.
    fn target_from_impl(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        subst: &TypeSubst,
    ) -> Result<Option<Ty>, D::Error> {
        AssociatedTypeProjector::new(&self.item_paths, &self.target_items)
            .associated_type_from_impl(trait_impl, impl_data, "Target", subst)
    }
}
