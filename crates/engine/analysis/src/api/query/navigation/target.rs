//! Concrete navigation target projection.

use rg_body_ir::{
    BodyAutoderef, BodyDeclarationRef, BodyImplId, BodyImplRef, BodyItemRef, BodyRef, BodyTy,
    ResolvedDeclarationRef,
};
use rg_def_map::{DefId, LocalDefRef, ModuleOrigin, ModuleRef};
use rg_semantic_ir::{
    EnumVariantRef, FieldRef, FunctionRef, ImplRef, SemanticItemRef, TraitRef, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        view::declaration::{Declaration, DeclarationRef, DeclarationView},
    },
    model::{NavigationTarget, NavigationTargetKind},
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

        Ok(self
            .declaration(module_ref)?
            .map(|declaration| NavigationTarget {
                target: declaration.target(),
                kind: NavigationTargetKind::from(declaration.kind()),
                name: declaration.name().to_string(),
                file_id: declaration.file_id(),
                span: Some(declaration.span()),
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
        self.navigation_target_for_declaration(declaration.into())
    }

    pub(crate) fn navigation_target_for_resolved_declaration(
        &self,
        declaration: ResolvedDeclarationRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match declaration {
            ResolvedDeclarationRef::Def(def) => self.navigation_target_for_def(def),
            ResolvedDeclarationRef::Semantic(declaration) => {
                self.navigation_target_for_declaration(declaration.into())
            }
            ResolvedDeclarationRef::Body(declaration) => {
                self.navigation_target_for_declaration(declaration.into())
            }
        }
    }

    pub(crate) fn navigation_target_for_semantic_item(
        &self,
        item_ref: SemanticItemRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(item_ref.into())
    }

    pub(crate) fn navigation_target_for_field(
        &self,
        field_ref: FieldRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(field_ref.into())
    }

    pub(crate) fn navigation_target_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(function_ref.into())
    }

    pub(crate) fn navigation_target_for_impl(
        &self,
        impl_ref: ImplRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(impl_ref.into())
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

    fn declaration(
        &self,
        declaration: impl Into<DeclarationRef>,
    ) -> anyhow::Result<Option<Declaration>> {
        DeclarationView::new(self.0).declaration(declaration.into())
    }

    pub(crate) fn navigation_target_for_enum_variant(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(variant_ref.into())
    }

    pub(crate) fn navigation_target_for_trait(
        &self,
        trait_ref: TraitRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(trait_ref.into())
    }

    pub(crate) fn navigation_target_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_declaration(ty.into())
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

    pub(crate) fn navigation_target_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match declaration {
            DeclarationRef::Module(module) => self.navigation_target_for_module(module),
            DeclarationRef::LocalDef(local_def) => self.navigation_target_for_local_def(local_def),
            DeclarationRef::Semantic(_) | DeclarationRef::Body(_) => {
                Ok(self.declaration(declaration)?.map(NavigationTarget::from))
            }
        }
    }
}
