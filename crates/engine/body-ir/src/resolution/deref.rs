//! Trait-backed `Deref` target lookup for Body IR autoderef.
//!
//! This module deliberately stays narrow: it recognizes `core::ops::Deref` impls for a known
//! nominal receiver and resolves the impl's associated `Target` type with the receiver substitution.

use rg_def_map::{DefMapReadTxn, Path, PathSegment};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{
    AssocItemId, SemanticIrReadTxn, SemanticTypePathResolution, TraitImplRef, TypeAliasRef,
    TypePathContext,
};
use rg_text::Name;

use crate::ir::ty::{BodyNominalTy, BodyTy};

use super::{
    impl_match::BodyImplMatcher,
    index::SemanticResolutionIndex,
    push_unique,
    ty::{TypeSubst, ty_from_type_ref_in_context},
};

/// Resolves the associated `Target` type for applicable `core::ops::Deref` impls.
#[derive(Clone, Copy)]
pub(super) struct BodyDerefResolver<'query, 'db> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    semantic_index: Option<&'query SemanticResolutionIndex>,
}

impl<'query, 'db> BodyDerefResolver<'query, 'db> {
    pub(super) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        semantic_index: Option<&'query SemanticResolutionIndex>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            semantic_index,
        }
    }

    /// Returns all one-step `Deref::Target` types for a Body IR type.
    ///
    /// Only module-level nominal types participate. Body-local trait impls remain outside this
    /// lookup model, matching method resolution's current boundary.
    pub(super) fn targets_for_ty(&self, ty: &BodyTy) -> Result<Vec<BodyTy>, PackageStoreError> {
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

    /// Returns one-step `Deref::Target` types for a semantic nominal receiver.
    ///
    /// For `impl<T> core::ops::Deref for Wrapper<T> { type Target = T; }` and receiver
    /// `Wrapper<User>`, this resolves the target as `User`.
    fn targets_for_nominal(
        &self,
        receiver_ty: &BodyNominalTy,
    ) -> Result<Vec<BodyTy>, PackageStoreError> {
        let matcher = BodyImplMatcher::new(self.def_map, self.semantic_ir);
        let mut targets = Vec::new();
        let trait_impls = match self.semantic_index {
            Some(index) => index.trait_impls_for_type(receiver_ty.def).to_vec(),
            None => self.semantic_ir.trait_impls_for_type(receiver_ty.def)?,
        };

        for trait_impl in trait_impls {
            let Some(impl_data) = self.semantic_ir.impl_data(trait_impl.impl_ref)? else {
                continue;
            };
            if !self.is_core_ops_deref_impl(trait_impl, impl_data)? {
                continue;
            }

            // `Deref` is a real type adjustment, not just an optimistic editor candidate.
            // Require a structural impl-self match so uncertain trait impls cannot change
            // field/method lookup receiver types.
            let Some(subst) =
                matcher.semantic_trait_impl_structural_match(trait_impl, receiver_ty)?
            else {
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
        impl_data: &rg_semantic_ir::ImplData,
    ) -> Result<bool, PackageStoreError> {
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

        Ok(
            match self
                .semantic_ir
                .resolve_type_path(self.def_map, context, &path)?
            {
                SemanticTypePathResolution::Traits(traits) => {
                    traits.contains(&trait_impl.trait_ref)
                }
                SemanticTypePathResolution::SelfType(_)
                | SemanticTypePathResolution::TypeDefs(_)
                | SemanticTypePathResolution::Unknown => false,
            },
        )
    }

    /// Resolves the `type Target = ...` item declared in a matching `Deref` impl.
    fn target_from_impl(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &rg_semantic_ir::ImplData,
        subst: &TypeSubst,
    ) -> Result<Option<BodyTy>, PackageStoreError> {
        for item in &impl_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                target: trait_impl.impl_ref.target,
                id: *type_alias_id,
            };
            let Some(type_alias_data) = self.semantic_ir.type_alias_data(type_alias_ref)? else {
                continue;
            };
            if type_alias_data.name.as_str() != "Target" {
                continue;
            }
            let Some(target_ty) = type_alias_data.signature.aliased_ty() else {
                continue;
            };

            let resolved = self.ty_from_target_type_ref(trait_impl, impl_data, target_ty, subst)?;
            if matches!(resolved, BodyTy::Unknown | BodyTy::Syntax(_)) {
                return Ok(None);
            }
            return Ok(Some(resolved));
        }

        Ok(None)
    }

    /// Converts the associated target type into Body IR after applying impl substitutions.
    fn ty_from_target_type_ref(
        &self,
        trait_impl: TraitImplRef,
        impl_data: &rg_semantic_ir::ImplData,
        target_ty: &TypeRef,
        subst: &TypeSubst,
    ) -> Result<BodyTy, PackageStoreError> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(trait_impl.impl_ref),
        };
        ty_from_type_ref_in_context(
            self.def_map,
            self.semantic_ir,
            target_ty,
            context,
            BodyTy::Syntax(target_ty.clone()),
            subst,
        )
    }
}
