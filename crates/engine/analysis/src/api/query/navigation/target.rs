//! Concrete navigation target projection.

use rg_body_ir::{
    BodyAutoderef, BodyEnumVariantRef, BodyImplId, BodyItemRef, BodyRef, BodyTy, BodyValueItemRef,
    ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::{DefId, LocalDefRef, ModuleOrigin, ModuleRef};
use rg_semantic_ir::{EnumVariantRef, FieldRef, FunctionRef, ImplRef, TraitRef, TypeDefRef};

use crate::{
    api::{
        Analysis,
        query::symbols::shared,
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
        // Root modules have no declaration name to jump to, so they navigate to the owning file.
        // Named modules navigate to the `mod` declaration that introduced them.
        let (file_id, span) = match module.origin {
            ModuleOrigin::Root { file_id } => (file_id, None),
            ModuleOrigin::Inline {
                declaration_file,
                declaration_span,
            }
            | ModuleOrigin::OutOfLine {
                declaration_file,
                declaration_span,
                ..
            } => (declaration_file, Some(declaration_span)),
        };

        Ok(Some(NavigationTarget {
            target: module_ref.target,
            kind: NavigationTargetKind::Module,
            name: module
                .name
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "crate".to_string()),
            file_id,
            span,
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
        Ok(self.declaration(item_ref)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_body_value_item(
        &self,
        item_ref: BodyValueItemRef,
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
        Ok(self.declaration(field_ref)?.map(NavigationTarget::from))
    }

    pub(crate) fn navigation_target_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        self.navigation_target_for_resolved_function(ResolvedFunctionRef::Semantic(function_ref))
    }

    pub(crate) fn navigation_target_for_impl(
        &self,
        impl_ref: ImplRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(data) = self.0.semantic_ir.impl_data(impl_ref)? else {
            return Ok(None);
        };
        let Some(local_impl) = self.0.def_map.local_impl(data.local_impl)? else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: impl_ref.target,
            kind: NavigationTargetKind::Impl,
            name: shared::impl_label(data),
            file_id: local_impl.file_id,
            span: Some(local_impl.span),
        }))
    }

    pub(crate) fn navigation_target_for_body_impl(
        &self,
        body_ref: BodyRef,
        impl_id: BodyImplId,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(None);
        };
        let Some(data) = body.local_impl(impl_id) else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: body_ref.target,
            kind: NavigationTargetKind::Impl,
            name: shared::body_impl_label(data),
            file_id: data.source.file_id,
            span: Some(data.source.span),
        }))
    }

    pub(crate) fn navigation_target_for_resolved_function(
        &self,
        function_ref: ResolvedFunctionRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        Ok(self.declaration(function_ref)?.map(NavigationTarget::from))
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
        let Some(data) = self.0.semantic_ir.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: variant_ref.target,
            kind: NavigationTargetKind::EnumVariant,
            name: data.variant.name.to_string(),
            file_id: data.file_id,
            span: Some(data.variant.name_span),
        }))
    }

    pub(crate) fn navigation_target_for_resolved_enum_variant(
        &self,
        variant_ref: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match variant_ref {
            ResolvedEnumVariantRef::Semantic(variant) => {
                self.navigation_target_for_enum_variant(variant)
            }
            ResolvedEnumVariantRef::BodyLocal(variant) => {
                self.navigation_target_for_body_enum_variant(variant)
            }
        }
    }

    fn navigation_target_for_body_enum_variant(
        &self,
        variant_ref: BodyEnumVariantRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(data) = self.0.body_ir.local_enum_variant_data(variant_ref)? else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: variant_ref.item.body.target,
            kind: NavigationTargetKind::EnumVariant,
            name: data.variant.name.to_string(),
            file_id: data.item.source.file_id,
            span: Some(data.variant.name_span),
        }))
    }

    pub(crate) fn navigation_target_for_trait(
        &self,
        trait_ref: TraitRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(local_def) = self
            .0
            .semantic_ir
            .trait_data(trait_ref)?
            .map(|data| data.local_def)
        else {
            return Ok(None);
        };

        self.navigation_target_for_local_def(local_def)
    }

    pub(crate) fn navigation_target_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(target_ir) = self.0.semantic_ir.target_ir(ty.target)? else {
            return Ok(None);
        };
        let local_def = match ty.id {
            rg_semantic_ir::TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                data.local_def
            }
            rg_semantic_ir::TypeDefId::Enum(id) => {
                let Some(data) = target_ir.items().enum_data(id) else {
                    return Ok(None);
                };
                data.local_def
            }
            rg_semantic_ir::TypeDefId::Union(id) => {
                let Some(data) = target_ir.items().union_data(id) else {
                    return Ok(None);
                };
                data.local_def
            }
        };

        self.navigation_target_for_local_def(local_def)
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
