//! Concrete navigation target projection.

use rg_body_ir::{
    BodyAutoderef, BodyDeclarationRef, BodyImplId, BodyImplRef, BodyItemRef, BodyRef, BodyTy,
    ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::{DefId, LocalDefRef, ModuleOrigin, ModuleRef};
use rg_semantic_ir::{
    EnumVariantRef, FieldRef, FunctionRef, ImplRef, SemanticItemRef, TraitRef, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        view::declaration::{DeclarationLookup, DeclarationRef},
    },
    model::{Declaration, NavigationTarget, NavigationTargetKind},
};

/// Converts stable IR identities into concrete editor navigation targets.
///
/// This resolver does not decide what the cursor means. It receives already-resolved def-map,
/// semantic IR, or body IR IDs and projects them into the public `NavigationTarget` shape.
pub(crate) struct NavigationTargetResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> NavigationTargetResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn navigation_target_for_def(
        &self,
        def: DefId,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match def {
            DefId::Module(module_ref) => self.navigation_target_for_module(module_ref),
            DefId::Local(local_def) => self.navigation_target_for_local_def(local_def),
        }
    }

    fn navigation_target_for_module(
        &self,
        module_ref: ModuleRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(module) = self.0.def_map.module(module_ref)? else {
            return Ok(None);
        };
        if let ModuleOrigin::Root { file_id } = module.origin {
            // Root modules have no declaration name to jump to, so they navigate to the owning
            // file. Named modules are ordinary declarations.
            return Ok(Some(NavigationTarget {
                target: module_ref.target,
                kind: NavigationTargetKind::Module,
                name: "crate".to_string(),
                file_id,
                span: None,
            }));
        };

        Ok(self.declaration(module_ref)?.map(|declaration| {
            let span = declaration.span;
            NavigationTarget {
                span: Some(span),
                ..NavigationTarget::from(declaration)
            }
        }))
    }

    fn navigation_target_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(local_def)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_body_item(
        &self,
        item_ref: BodyItemRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_body_declaration(item_ref.into())
    }

    pub(crate) fn navigation_target_for_body_declaration(
        &self,
        declaration: BodyDeclarationRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(declaration)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_semantic_item(
        &self,
        item_ref: SemanticItemRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(item_ref)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_field(
        &self,
        field_ref: FieldRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_resolved_field(ResolvedFieldRef::Semantic(field_ref))
    }

    pub(crate) fn navigation_target_for_resolved_field(
        &self,
        field_ref: ResolvedFieldRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match field_ref {
            ResolvedFieldRef::Semantic(_) => {
                Ok(self.declaration(field_ref)?.map(NavigationTarget::from))
            }
            ResolvedFieldRef::BodyLocal(field_ref) => {
                self.navigation_target_for_body_declaration(field_ref.into())
            }
        }
    }

    pub(crate) fn navigation_target_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(function_ref)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_impl(
        &self,
        impl_ref: ImplRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(impl_ref)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_body_impl(
        &self,
        body_ref: BodyRef,
        impl_id: BodyImplId,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_body_declaration(
            BodyImplRef {
                body: body_ref,
                impl_id,
            }
            .into(),
        )
    }

    pub(crate) fn navigation_target_for_resolved_function(
        &self,
        function_ref: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match function_ref {
            ResolvedFunctionRef::Semantic(_) => {
                Ok(self.declaration(function_ref)?.map(NavigationTarget::from))
            }
            ResolvedFunctionRef::BodyLocal(function_ref) => {
                self.navigation_target_for_body_declaration(function_ref.into())
            }
        }
    }

    fn declaration(
        &self,
        declaration: impl Into<DeclarationRef>,
    ) -> anyhow::Result<Option<Declaration>> {
        DeclarationLookup::new(self.0).declaration(declaration.into())
    }

    pub(crate) fn navigation_target_for_enum_variant(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_resolved_enum_variant(ResolvedEnumVariantRef::Semantic(
            variant_ref,
        ))
    }

    pub(crate) fn navigation_target_for_resolved_enum_variant(
        &self,
        variant_ref: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match variant_ref {
            ResolvedEnumVariantRef::Semantic(_) => {
                Ok(self.declaration(variant_ref)?.map(NavigationTarget::from))
            }
            ResolvedEnumVariantRef::BodyLocal(variant_ref) => {
                self.navigation_target_for_body_declaration(variant_ref.into())
            }
        }
    }

    pub(crate) fn navigation_target_for_trait(
        &self,
        trait_ref: TraitRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(trait_ref)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(ty)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_targets_for_body_ty(
        &self,
        ty: &BodyTy,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let mut local_targets = Vec::new();
        for candidate in BodyAutoderef::peel_references(ty) {
            for ty in candidate.ty().as_local_nominals() {
                if let Some(target) = self.navigation_target_for_body_item(ty.item)? {
                    local_targets.push(target);
                }
            }
        }
        if !local_targets.is_empty() {
            return Ok(local_targets);
        }

        let mut targets = Vec::new();
        for candidate in BodyAutoderef::peel_references(ty) {
            for ty in candidate.ty().as_nominals() {
                if let Some(target) = self.navigation_target_for_type_def(ty.def)? {
                    targets.push(target);
                }
            }
        }
        Ok(targets)
    }
}
