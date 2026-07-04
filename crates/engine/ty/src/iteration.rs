//! Trait-backed iteration item lookup.
//!
//! This module recognizes canonical iterator traits from the use site and asks the shared
//! trait-selection projection API for their associated `Item` type.

use rg_ir_model::{Path, PathSegment, TraitImplRef, TraitRef, hir::items::ImplData};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_text::Name;

use crate::{
    ExpectedTyExt, ItemPathQuery, TraitGoal, TraitSelectionCache, TraitSelectionQuery, Ty,
    associated_type::AssociatedTypeResolver,
    inference::{InferTy, InferenceTable},
};

/// Resolves the associated `Item` type for applicable iterator-shaped trait impls.
#[derive(Clone)]
pub struct IterationItemResolver<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: &'query ItemLookupIndex,
    trait_selection_cache: TraitSelectionCache,
}

impl<'query, D, I> IterationItemResolver<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Creates an iteration resolver over a target-scoped item lookup index.
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index,
            trait_selection_cache: TraitSelectionCache::default(),
        }
    }

    /// Reuse solver state across repeated iterator projections in the same visibility context.
    pub fn with_cache(mut self, cache: TraitSelectionCache) -> Self {
        self.trait_selection_cache = cache;
        self
    }

    /// Returns the item yielded by `for pat in value`, i.e. `IntoIterator::Item`.
    pub fn into_iterator_item_for_ty(&self, ty: &Ty) -> Result<Ty, D::Error> {
        self.associated_item_for_trait(ty, CanonicalIteratorTrait::IntoIterator)
    }

    /// Returns the item yielded by a value already known to implement `Iterator`.
    pub fn iterator_item_for_ty(&self, ty: &Ty) -> Result<Ty, D::Error> {
        self.associated_item_for_trait(ty, CanonicalIteratorTrait::Iterator)
    }

    /// Returns true when a selected trait is the canonical `core::iter::Iterator`.
    pub fn is_iterator_trait_ref(&self, trait_ref: TraitRef) -> Result<bool, D::Error> {
        let resolver = AssociatedTypeResolver::new(&self.item_paths, &self.target_items);
        let canonical_traits =
            self.canonical_trait_refs_from_use_site(&resolver, CanonicalIteratorTrait::Iterator)?;
        Ok(canonical_traits.contains(&trait_ref))
    }

    fn associated_item_for_trait(
        &self,
        ty: &Ty,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<Ty, D::Error> {
        if matches!(ty, Ty::Unknown | Ty::Syntax(_)) {
            return Ok(Ty::Unknown);
        }

        let resolver = AssociatedTypeResolver::new(&self.item_paths, &self.target_items);
        let canonical_traits = self.canonical_trait_refs_for_lookup(&resolver, trait_kind)?;
        let mut candidates = ExpectedUnique::new();
        self.push_solver_projected_items(&mut candidates, ty, &canonical_traits)?;
        if !candidates.is_empty() {
            return Ok(candidates.into_ty());
        }

        Ok(Ty::Unknown)
    }

    /// Ask trait selection to project `Item` for every canonical trait ref we found.
    ///
    /// There can be more than one ref in incomplete fixture or multi-store situations. Keep the
    /// result unique so conflicting canonical candidates become `Unknown` instead of a guessed
    /// iterator item.
    fn push_solver_projected_items(
        &self,
        candidates: &mut ExpectedUnique<Ty>,
        ty: &Ty,
        canonical_traits: &UniqueVec<TraitRef>,
    ) -> Result<(), D::Error> {
        let table = InferenceTable::new();
        for trait_ref in canonical_traits {
            let goal = TraitGoal {
                self_ty: InferTy::from_ty(ty),
                trait_ref: *trait_ref,
                args: Vec::new(),
            };
            let Some(projection) = TraitSelectionQuery::with_index(
                self.item_paths.clone(),
                self.target_items.clone(),
                self.lookup_index,
            )
            .with_cache(self.trait_selection_cache.clone())
            .normalize_assoc_type(&goal, "Item", &table)?
            else {
                continue;
            };
            let item_ty = projection.table.finalize(&projection.ty);
            if item_ty.is_projectable() {
                candidates.push(item_ty);
            }
        }

        Ok(())
    }

    /// Find the canonical iterator trait ref visible to this lookup.
    ///
    /// The use-site root is the common path. If it is unavailable, fall back to checking impl
    /// contexts: fake sysroots and core-like packages may be able to resolve `::core::iter::Trait`
    /// from the impl owner even when the target use site cannot.
    fn canonical_trait_refs_for_lookup(
        &self,
        resolver: &AssociatedTypeResolver<'_, 'query, D, I>,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        let mut canonical_traits = self.canonical_trait_refs_from_use_site(resolver, trait_kind)?;
        if !canonical_traits.is_empty() {
            return Ok(canonical_traits);
        }

        let item_query = self.item_paths.items();
        for trait_impl in self.lookup_index.trait_impls() {
            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };
            if !self.is_canonical_trait_impl(resolver, trait_impl, impl_data, trait_kind)? {
                continue;
            }

            canonical_traits.push(trait_impl.trait_ref);
        }

        Ok(canonical_traits)
    }

    /// Checks whether this trait impl resolved to the canonical iterator trait path.
    fn is_canonical_trait_impl(
        &self,
        resolver: &AssociatedTypeResolver<'_, 'query, D, I>,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<bool, D::Error> {
        Ok(self
            .canonical_trait_refs(resolver, impl_data, trait_kind)?
            .contains(&trait_impl.trait_ref))
    }

    fn canonical_trait_refs(
        &self,
        resolver: &AssociatedTypeResolver<'_, 'query, D, I>,
        impl_data: &ImplData,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        resolver
            .trait_refs_for_path_from_impl_and_use_site(impl_data, &trait_kind.absolute_core_path())
    }

    fn canonical_trait_refs_from_use_site(
        &self,
        resolver: &AssociatedTypeResolver<'_, 'query, D, I>,
        trait_kind: CanonicalIteratorTrait,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        resolver.trait_refs_for_path_from_use_site(&trait_kind.absolute_core_path())
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
