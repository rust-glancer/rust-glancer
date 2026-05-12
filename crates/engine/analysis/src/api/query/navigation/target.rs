//! Concrete navigation target projection.

use rg_body_ir::{
    BodyFieldRef, BodyImplId, BodyItemRef, BodyRef, BodyTy, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::{DefId, LocalDefRef, ModuleOrigin, ModuleRef};
use rg_semantic_ir::{EnumVariantRef, FieldRef, FunctionRef, ImplRef, TraitRef, TypeDefRef};

use crate::{
    api::{Analysis, query::symbols::shared},
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
        let Some(local_def_data) = self.0.def_map.local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: local_def.target,
            kind: NavigationTargetKind::from_local_def_kind(local_def_data.kind),
            name: local_def_data.name.to_string(),
            file_id: local_def_data.file_id,
            // Goto should land on the declaration name rather than the whole item. The full item
            // span intentionally includes doc comments, which is useful for outline/hover-like
            // features but feels wrong as an editor cursor destination.
            span: Some(local_def_data.name_span.unwrap_or(local_def_data.span)),
        }))
    }

    pub(crate) fn navigation_target_for_body_item(
        &self,
        item_ref: BodyItemRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(body_data) = self.0.body_ir.body_data(item_ref.body)? else {
            return Ok(None);
        };
        let Some(item) = body_data.local_item(item_ref.item) else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: item_ref.body.target,
            kind: NavigationTargetKind::from_body_item_kind(item.kind),
            name: item.name.to_string(),
            file_id: item.source.file_id,
            span: Some(item.name_source.span),
        }))
    }

    pub(crate) fn navigation_target_for_field(
        &self,
        field_ref: FieldRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(field_data) = self.0.semantic_ir.field_data(field_ref)? else {
            return Ok(None);
        };
        let Some(key) = field_data.field.key.as_ref() else {
            return Ok(None);
        };
        Ok(Some(NavigationTarget {
            target: field_ref.owner.target,
            kind: NavigationTargetKind::Field,
            name: key.declaration_label(),
            file_id: field_data.file_id,
            span: Some(field_data.field.span),
        }))
    }

    pub(crate) fn navigation_target_for_resolved_field(
        &self,
        field_ref: ResolvedFieldRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match field_ref {
            ResolvedFieldRef::Semantic(field) => self.navigation_target_for_field(field),
            ResolvedFieldRef::BodyLocal(field) => self.navigation_target_for_local_field(field),
        }
    }

    fn navigation_target_for_local_field(
        &self,
        field_ref: BodyFieldRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(field_data) = self.0.body_ir.local_field_data(field_ref)? else {
            return Ok(None);
        };
        let Some(key) = field_data.field.key.as_ref() else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: field_ref.item.body.target,
            kind: NavigationTargetKind::Field,
            name: key.declaration_label(),
            file_id: field_data.item.source.file_id,
            span: Some(field_data.field.span),
        }))
    }

    pub(crate) fn navigation_target_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(function_data) = self.0.semantic_ir.function_data(function_ref)? else {
            return Ok(None);
        };

        Ok(Some(NavigationTarget {
            target: function_ref.target,
            kind: NavigationTargetKind::Function,
            name: function_data.name.to_string(),
            file_id: function_data.source.file_id,
            span: Some(function_data.name_span.unwrap_or(function_data.span)),
        }))
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
        match function_ref {
            ResolvedFunctionRef::Semantic(function) => {
                self.navigation_target_for_function(function)
            }
            ResolvedFunctionRef::BodyLocal(function) => {
                let Some(data) = self.0.body_ir.local_function_data(function)? else {
                    return Ok(None);
                };
                Ok(Some(NavigationTarget {
                    target: function.body.target,
                    kind: NavigationTargetKind::Function,
                    name: data.name.to_string(),
                    file_id: data.source.file_id,
                    span: Some(data.name_source.span),
                }))
            }
        }
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
        for ty in ty.local_nominals() {
            if let Some(target) = self.navigation_target_for_body_item(ty.item)? {
                local_targets.push(target);
            }
        }
        if !local_targets.is_empty() {
            return Ok(local_targets);
        }

        let mut targets = Vec::new();
        for ty in ty.nominal_tys() {
            if let Some(target) = self.navigation_target_for_type_def(ty.def)? {
                targets.push(target);
            }
        }
        Ok(targets)
    }
}
