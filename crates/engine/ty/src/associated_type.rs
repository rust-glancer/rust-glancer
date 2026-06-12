//! Shared projection for associated types declared by trait impls.
//!
//! This is intentionally much smaller than a trait solver: callers first decide that an impl is a
//! relevant candidate, then this helper reads a named associated type from that impl and resolves it
//! with the substitutions supplied by `ImplMatcher`.

use rg_ir_model::{
    AssocItemId, Path, TraitImplRef, TraitRef, TypeAliasRef, TypePathResolution,
    hir::items::ImplData,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TargetItemQuery, TypePathContext};
use rg_std::{ExpectedUnique, UniqueVec};

use crate::{GenericArg, ItemPathQuery, OpaqueTraitBound, Ty, TypeSubst};

pub(crate) struct AssociatedTypeProjector<'a, 'query, D, I> {
    item_paths: &'a ItemPathQuery<'query, D, I>,
    target_items: &'a TargetItemQuery<'query, D, I>,
}

impl<'a, 'query, D, I> AssociatedTypeProjector<'a, 'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    pub(crate) fn new(
        item_paths: &'a ItemPathQuery<'query, D, I>,
        target_items: &'a TargetItemQuery<'query, D, I>,
    ) -> Self {
        Self {
            item_paths,
            target_items,
        }
    }

    pub(crate) fn trait_impl_resolves_to_path(
        &self,
        trait_impl: TraitImplRef,
        context: TypePathContext,
        trait_path: &Path,
    ) -> Result<bool, D::Error> {
        Ok(self
            .trait_refs_for_path(context, trait_path)?
            .contains(&trait_impl.trait_ref))
    }

    pub(crate) fn trait_refs_for_path(
        &self,
        context: TypePathContext,
        trait_path: &Path,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        let TypePathResolution::Trait(trait_ref) =
            self.item_paths.resolve_type_path(context, trait_path)?
        else {
            return Ok(UniqueVec::new());
        };
        let mut traits = UniqueVec::new();
        traits.push(trait_ref);
        Ok(traits)
    }

    pub(crate) fn trait_refs_for_path_from_impl_and_use_site(
        &self,
        impl_data: &ImplData,
        trait_path: &Path,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        let mut traits = UniqueVec::new();

        // Impls written outside `core` can resolve `::core::path::Trait` from their own module.
        // Impls written inside the core crate itself need the lookup target's extern-root view
        // instead, because a fixture package may not name itself `core` internally.
        let impl_context = TypePathContext {
            module: impl_data.owner,
            impl_ref: None,
        };
        self.push_trait_refs_for_path(impl_context, trait_path, &mut traits)?;

        if let Some(use_site_root) = self.target_items.use_site_root_module()? {
            self.push_trait_refs_for_path(
                TypePathContext {
                    module: use_site_root,
                    impl_ref: None,
                },
                trait_path,
                &mut traits,
            )?;
        }

        Ok(traits)
    }

    pub(crate) fn trait_refs_for_path_from_use_site(
        &self,
        trait_path: &Path,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        let mut traits = UniqueVec::new();
        let Some(use_site_root) = self.target_items.use_site_root_module()? else {
            return Ok(traits);
        };

        self.push_trait_refs_for_path(
            TypePathContext {
                module: use_site_root,
                impl_ref: None,
            },
            trait_path,
            &mut traits,
        )?;
        Ok(traits)
    }

    fn push_trait_refs_for_path(
        &self,
        context: TypePathContext,
        trait_path: &Path,
        traits: &mut UniqueVec<TraitRef>,
    ) -> Result<(), D::Error> {
        for trait_ref in self.trait_refs_for_path(context, trait_path)? {
            traits.push(trait_ref);
        }
        Ok(())
    }

    pub(crate) fn associated_type_from_impl(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        assoc_name: &str,
        subst: &TypeSubst,
    ) -> Result<Option<Ty>, D::Error> {
        let item_query = self.item_paths.items();
        for item in &impl_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: trait_impl.impl_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) = item_query.type_alias_data(type_alias_ref)? else {
                continue;
            };
            if type_alias_data.name.as_str() != assoc_name {
                continue;
            }
            let Some(aliased_ty) = type_alias_data.signature.aliased_ty() else {
                continue;
            };

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(trait_impl.impl_ref),
            };
            let ty = self.item_paths.resolve_type_ref(
                aliased_ty,
                context,
                Ty::syntax(aliased_ty.clone()),
                subst,
            )?;
            return Ok(ty.is_projectable().then_some(ty));
        }

        Ok(None)
    }

    pub(crate) fn push_associated_types_from_opaque_bounds(
        &self,
        candidates: &mut ExpectedUnique<Ty>,
        bounds: &UniqueVec<OpaqueTraitBound>,
        canonical_traits: &UniqueVec<TraitRef>,
        assoc_name: &str,
    ) {
        for bound in bounds {
            if !canonical_traits.contains(&bound.trait_ref) {
                continue;
            }

            // Opaque types expose only their declared bounds. Associated type equalities such as
            // `impl Iterator<Item = User>` are precise facts even though the hidden concrete type
            // remains unknown.
            for arg in &bound.args {
                let GenericArg::AssocType { name, ty: Some(ty) } = arg else {
                    continue;
                };
                if name.as_str() == assoc_name && ty.is_projectable() {
                    candidates.push((**ty).clone());
                }
            }
        }
    }
}
