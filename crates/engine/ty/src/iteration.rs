//! Trait-backed iteration item lookup.
//!
//! This module mirrors the narrow shape of `DerefResolver`: it recognizes canonical iterator
//! traits and projects their associated `Item` type for impls whose self type can be matched
//! without a solver.

use rg_ir_model::items::{TypeBound, TypeRef};
use rg_ir_model::{
    AssocItemId, ImplRef, Path, PathSegment, TraitImplRef, TraitRef, TypeAliasRef,
    hir::items::ImplData,
};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery, TypePathContext,
};
use rg_std::UniqueVec;
use rg_text::Name;

use crate::{ImplMatcher, ItemPathQuery, Ty, associated_type::AssociatedTypeProjector};

/// Resolves the associated `Item` type for applicable iterator-shaped trait impls.
#[derive(Clone)]
pub struct IterationItemResolver<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: Option<&'query ItemLookupIndex>,
}

impl<'query, D, I> IterationItemResolver<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub fn new(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index: None,
        }
    }

    /// Creates an iteration resolver that can reuse a target-scoped item lookup index.
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index: Some(lookup_index),
        }
    }

    /// Returns the item yielded by `for pat in value`, i.e. `IntoIterator::Item`.
    pub fn into_iterator_item_for_ty(&self, ty: &Ty) -> Result<Ty, D::Error> {
        self.associated_item_for_trait(ty, CanonicalIteratorTrait::IntoIterator)
    }

    /// Returns the item yielded by a value already known to implement `Iterator`.
    pub fn iterator_item_for_ty(&self, ty: &Ty) -> Result<Ty, D::Error> {
        self.associated_item_for_trait(ty, CanonicalIteratorTrait::Iterator)
    }

    fn associated_item_for_trait(
        &self,
        ty: &Ty,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<Ty, D::Error> {
        if matches!(ty, Ty::Unknown | Ty::Syntax(_)) {
            return Ok(Ty::Unknown);
        }

        let projector = AssociatedTypeProjector::new(&self.item_paths, &self.target_items);
        let matcher = ImplMatcher::new(self.item_paths.clone(), self.target_items.clone());
        let canonical_traits = self.canonical_trait_refs_from_use_site(&projector, trait_kind)?;
        let mut candidates = UniqueVec::new();
        if let Ty::Opaque { bounds } = ty {
            projector.push_associated_types_from_opaque_bounds(
                &mut candidates,
                bounds,
                &canonical_traits,
                "Item",
            );
        }

        let item_query = self.item_paths.items();
        for trait_impl in self.trait_impl_candidates(&projector, &canonical_traits, trait_kind)? {
            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };

            if matches!(trait_kind, CanonicalIteratorTrait::IntoIterator)
                && let Some(item_ty) = self.blanket_into_iterator_item_for_ty(
                    &projector,
                    trait_impl.impl_ref,
                    impl_data,
                    ty,
                )?
            {
                candidates.push(item_ty);
                continue;
            }

            let Some(subst) =
                matcher.trait_impl_projection_subst_for_ty(trait_impl, impl_data, ty)?
            else {
                continue;
            };
            let Some(item_ty) =
                projector.associated_type_from_impl(trait_impl, impl_data, "Item", &subst)?
            else {
                continue;
            };
            candidates.push(item_ty);
        }

        Ok(Ty::one_or_unknown(candidates.into_vec()))
    }

    fn trait_impl_candidates(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        canonical_traits: &[TraitRef],
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<Vec<TraitImplRef>, D::Error> {
        let mut candidates = UniqueVec::new();
        for trait_ref in canonical_traits {
            if let Some(indexed_impls) = self
                .lookup_index
                .and_then(|index| index.trait_impls_for_trait(*trait_ref))
            {
                for trait_impl in indexed_impls {
                    candidates.push(*trait_impl);
                }
                continue;
            }

            for trait_impl in self.target_items.trait_impls_for_trait(*trait_ref)? {
                candidates.push(trait_impl);
            }
        }

        if !canonical_traits.is_empty() {
            return Ok(candidates.into_vec());
        }

        // Some fixture/core-like contexts cannot resolve `::core` from the use-site root. Keep the
        // older impl-context scan as a rare fallback so the fast path does not narrow semantics.
        for store in self.target_items.visible_stores()? {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                if impl_data.trait_ref.is_none() {
                    continue;
                }

                for trait_ref in &impl_data.resolved_trait_refs {
                    let trait_impl = TraitImplRef {
                        impl_ref,
                        trait_ref: *trait_ref,
                    };
                    if !self
                        .is_canonical_trait_impl(projector, trait_impl, impl_data, trait_kind)?
                    {
                        continue;
                    }

                    candidates.push(trait_impl);
                }
            }
        }

        Ok(candidates.into_vec())
    }

    /// Checks whether this trait impl resolved to the canonical iterator trait path.
    fn is_canonical_trait_impl(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<bool, D::Error> {
        Ok(self
            .canonical_trait_refs(projector, impl_data, trait_kind)?
            .contains(&trait_impl.trait_ref))
    }

    fn canonical_trait_refs(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        impl_data: &ImplData,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<Vec<TraitRef>, D::Error> {
        projector
            .trait_refs_for_path_from_impl_and_use_site(impl_data, &trait_kind.absolute_core_path())
    }

    fn canonical_trait_refs_from_use_site(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<Vec<TraitRef>, D::Error> {
        projector.trait_refs_for_path_from_use_site(&trait_kind.absolute_core_path())
    }

    fn blanket_into_iterator_item_for_ty(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &Ty,
    ) -> Result<Option<Ty>, D::Error> {
        let Some(param_name) =
            self.blanket_into_iterator_param_name(projector, impl_ref, impl_data)?
        else {
            return Ok(None);
        };
        if !self.impl_item_aliases_type_param_item(impl_ref, impl_data, &param_name)? {
            return Ok(None);
        }

        // This is the one blanket impl we model: `impl<I: Iterator> IntoIterator for I`.
        // Instead of solving the bound generally, ask the same resolver for `Iterator::Item`
        // on the concrete receiver and reuse that projection when it is unambiguous.
        let item_ty = self.iterator_item_for_ty(receiver_ty)?;
        Ok(item_ty.is_projectable().then_some(item_ty))
    }

    fn blanket_into_iterator_param_name(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        impl_ref: ImplRef,
        impl_data: &ImplData,
    ) -> Result<Option<Name>, D::Error> {
        if !impl_data.generics.lifetimes.is_empty()
            || !impl_data.generics.consts.is_empty()
            || !impl_data.generics.where_predicates.is_empty()
        {
            return Ok(None);
        }

        let [param] = impl_data.generics.types.as_slice() else {
            return Ok(None);
        };
        if param.default.is_some() || param.bounds.len() != 1 {
            return Ok(None);
        }
        if impl_data
            .self_ty
            .type_param_name()
            .is_none_or(|name| name != param.name)
        {
            return Ok(None);
        }
        if !self.type_bound_is_canonical_trait(
            projector,
            impl_ref,
            impl_data,
            &param.bounds[0],
            CanonicalIteratorTrait::Iterator,
        )? {
            return Ok(None);
        }

        Ok(Some(param.name.clone()))
    }

    fn type_bound_is_canonical_trait(
        &self,
        projector: &AssociatedTypeProjector<'_, 'query, D, I>,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        bound: &TypeBound,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<bool, D::Error> {
        let TypeBound::Trait(bound_ty) = bound else {
            return Ok(false);
        };
        let Some(bound_path) = Path::from_type_ref(bound_ty) else {
            return Ok(false);
        };

        let bound_context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(impl_ref),
        };
        let bound_traits = projector.trait_refs_for_path(bound_context, &bound_path)?;

        let canonical_traits = self.canonical_trait_refs(projector, impl_data, trait_kind)?;
        Ok(bound_traits
            .into_iter()
            .any(|trait_ref| canonical_traits.contains(&trait_ref)))
    }

    fn impl_item_aliases_type_param_item(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        param_name: &Name,
    ) -> Result<bool, D::Error> {
        let item_query = self.item_paths.items();
        for item in &impl_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: impl_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) = item_query.type_alias_data(type_alias_ref)? else {
                continue;
            };
            if type_alias_data.name.as_str() != "Item" {
                continue;
            }

            return Ok(type_alias_data
                .signature
                .aliased_ty()
                .is_some_and(|ty| Self::is_type_param_assoc_item(ty, param_name, "Item")));
        }

        Ok(false)
    }

    fn is_type_param_assoc_item(ty: &TypeRef, param_name: &Name, assoc_name: &str) -> bool {
        let TypeRef::Path(path) = ty else {
            return false;
        };
        let [param_segment, assoc_segment] = path.segments.as_slice() else {
            return false;
        };

        !path.absolute
            && param_segment.name == *param_name
            && param_segment.args.is_empty()
            && assoc_segment.name.as_str() == assoc_name
            && assoc_segment.args.is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
enum CanonicalIteratorTrait {
    IntoIterator,
    Iterator,
}

impl CanonicalIteratorTrait {
    fn absolute_core_path(self) -> Path {
        let trait_name = match self {
            Self::IntoIterator => "IntoIterator",
            Self::Iterator => "Iterator",
        };

        Path {
            absolute: true,
            segments: vec![
                PathSegment::Name(Name::new("core")),
                PathSegment::Name(Name::new("iter")),
                PathSegment::Name(Name::new(trait_name)),
            ],
        }
    }
}
