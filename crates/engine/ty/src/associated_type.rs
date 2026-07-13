//! Small associated-type helpers that still sit outside general trait selection.
//!
//! The shared solver path owns ordinary trait selection and projection. This helper keeps two
//! narrow jobs that predate that boundary and are still useful to callers:
//!
//! - resolve canonical trait paths from impl and use-site contexts;
//! - read a selected impl associated type for strict adjustment paths such as `Deref`.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, Path, TraitImplRef, TraitRef, TypeAliasRef, TypePathResolution};
use rg_semantic_ir::{CrateItemQuery, ImplData, ItemStoreSource, TypePathContext};
use rg_std::UniqueVec;

use crate::{ItemPathQuery, Ty, TypeSubst};

/// Resolves associated-type-adjacent facts that are not general projection.
///
/// Most associated projection should go through `TraitSelectionQuery::normalize_assoc_type`.
/// This helper remains for stricter callers that already know which impl or canonical path they
/// are asking about.
pub(crate) struct AssociatedTypeResolver<'a, 'query, D, I> {
    item_paths: &'a ItemPathQuery<'query, D, I>,
    crate_items: &'a CrateItemQuery<'query, D, I>,
}

impl<'a, 'query, D, I> AssociatedTypeResolver<'a, 'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    pub(crate) fn new(
        item_paths: &'a ItemPathQuery<'query, D, I>,
        crate_items: &'a CrateItemQuery<'query, D, I>,
    ) -> Self {
        Self {
            item_paths,
            crate_items,
        }
    }

    /// Check whether an already-selected trait impl is for a specific canonical path.
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

    /// Resolve a trait path from one source context.
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

    /// Resolve a canonical trait path from both the impl's module and the lookup use site.
    ///
    /// Iterator and deref helpers often compare against paths like `::core::iter::Iterator`.
    /// Depending on whether the impl is written inside the core-like crate or outside it, one of
    /// these contexts may be the only place where that path resolves.
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

        if let Some(use_site_root) = self.crate_items.use_site_root_module()? {
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

    /// Resolve a canonical trait path from the target's use-site root.
    pub(crate) fn trait_refs_for_path_from_use_site(
        &self,
        trait_path: &Path,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        let mut traits = UniqueVec::new();
        let Some(use_site_root) = self.crate_items.use_site_root_module()? else {
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

    /// Read an associated alias body from an impl that another strict path already selected.
    ///
    /// This is used by adjustment code such as `Deref`, where an uncertain alias value must not
    /// become a real receiver type.
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
}
