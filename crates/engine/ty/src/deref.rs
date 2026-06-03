//! Trait-backed `Deref` target lookup for autoderef.
//!
//! This module deliberately stays narrow: it recognizes `core::ops::Deref` impls for a known
//! nominal receiver and resolves the impl's associated `Target` type with the receiver substitution.

use rg_ir_model::{
    AssocItemId, TraitImplRef, TypeAliasRef, TypePathResolution, hir::items::ImplData,
};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreSource, Path, PathSegment, TypePathContext,
};
use rg_item_tree::TypeRef;
use rg_text::Name;

use crate::{ImplMatcher, ItemPathQuery, NominalTy, Ty, TypeSubst};

/// Resolves the associated `Target` type for applicable `core::ops::Deref` impls.
#[derive(Clone)]
pub(crate) struct DerefResolver<'query, D, I> {
    item_paths: ItemPathQuery<'query, D, I>,
    lookup_index: Option<&'query ItemLookupIndex>,
}

impl<'query, D, I> DerefResolver<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub(crate) fn new(
        item_paths: ItemPathQuery<'query, D, I>,
        lookup_index: Option<&'query ItemLookupIndex>,
    ) -> Self {
        Self {
            item_paths,
            lookup_index,
        }
    }

    /// Returns all one-step `Deref::Target` types for a known type.
    pub(crate) fn targets_for_ty(&self, ty: &Ty) -> Result<Vec<Ty>, D::Error> {
        // TODO: Add `DerefMut` once receiver contexts carry enough mutability information to
        // distinguish mutable adjustment from shared `Deref`.
        let mut targets = Vec::new();
        for receiver_ty in ty.as_nominals() {
            for target in self.targets_for_nominal(receiver_ty)? {
                push_unique(&mut targets, target);
            }
        }
        Ok(targets)
    }

    /// Returns one-step `Deref::Target` types for a nominal receiver.
    ///
    /// For `impl<T> core::ops::Deref for Wrapper<T> { type Target = T; }` and receiver
    /// `Wrapper<User>`, this resolves the target as `User`.
    fn targets_for_nominal(&self, receiver_ty: &NominalTy) -> Result<Vec<Ty>, D::Error> {
        let matcher = ImplMatcher::new(self.item_paths.clone());
        let item_query = self.item_paths.items();
        let mut targets = Vec::new();
        let trait_impls = match self.lookup_index {
            Some(index) => index.trait_impls_for_type(receiver_ty.def).to_vec(),
            None => item_query.trait_impls_for_type(receiver_ty.def)?,
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
            push_unique(&mut targets, target);
        }

        Ok(targets)
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

        Ok(match self.item_paths.resolve_type_path(context, &path)? {
            TypePathResolution::Traits(traits) => traits.contains(&trait_impl.trait_ref),
            TypePathResolution::SelfType(_)
            | TypePathResolution::TypeDefs(_)
            | TypePathResolution::TypeAliases(_)
            | TypePathResolution::Unknown => false,
        })
    }

    /// Resolves the `type Target = ...` item declared in a matching `Deref` impl.
    fn target_from_impl(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
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
            if type_alias_data.name.as_str() != "Target" {
                continue;
            }
            let Some(target_ty) = type_alias_data.signature.aliased_ty() else {
                continue;
            };

            let resolved = self.ty_from_target_type_ref(trait_impl, impl_data, target_ty, subst)?;
            if matches!(resolved, Ty::Unknown | Ty::Syntax(_)) {
                return Ok(None);
            }
            return Ok(Some(resolved));
        }

        Ok(None)
    }

    /// Converts the associated target type after applying impl substitutions.
    fn ty_from_target_type_ref(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        target_ty: &TypeRef,
        subst: &TypeSubst,
    ) -> Result<Ty, D::Error> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(trait_impl.impl_ref),
        };
        self.item_paths
            .resolve_type_ref(target_ty, context, Ty::syntax(target_ty.clone()), subst)
    }
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
