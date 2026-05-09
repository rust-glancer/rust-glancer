//! Converts found symbols and inferred types into editor navigation targets.
//!
//! Analysis can receive identities from def-map, semantic IR, or body IR. This adapter keeps the
//! public navigation shape uniform while preserving the source span each layer considers primary.

use rg_body_ir::{
    BodyFieldRef, BodyItemRef, BodyRef, BodyResolution, BodyTy, BodyTypePathResolution,
    ResolvedFieldRef, ResolvedFunctionRef, ScopeId,
};
use rg_def_map::{DefId, LocalDefRef, ModuleOrigin, ModuleRef, Path, TargetRef};
use rg_parse::FileId;
use rg_semantic_ir::{
    EnumVariantRef, FieldRef, FunctionRef, SemanticTypePathResolution, TraitRef, TypeDefRef,
    TypePathContext,
};

use super::{
    Analysis,
    data::{NavigationTarget, NavigationTargetKind, SymbolAt},
};

pub(super) struct NavigationTargetResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> NavigationTargetResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    fn navigation_target_for_def(&self, def: DefId) -> anyhow::Result<Option<NavigationTarget>> {
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

    fn navigation_target_for_body_item(
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

    fn navigation_target_for_field(
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

    fn navigation_target_for_resolved_field(
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

    fn navigation_target_for_function(
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

    fn navigation_target_for_resolved_function(
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

    fn navigation_target_for_enum_variant(
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

    fn navigation_target_for_trait(
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

    fn navigation_target_for_type_def(
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

    fn navigation_targets_for_body_ty(&self, ty: &BodyTy) -> anyhow::Result<Vec<NavigationTarget>> {
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

pub(super) struct SymbolResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
        match symbol {
            SymbolAt::Binding { body, binding } => Ok(self
                .0
                .body_ir
                .body_data(body)?
                .and_then(|body_data| body_data.binding(binding))
                .map(|binding_data| vec![NavigationTarget::from_binding(body.target, binding_data)])
                .unwrap_or_default()),
            SymbolAt::BodyPath {
                body, scope, path, ..
            } => self.navigation_targets_for_body_type_path(body, scope, &path),
            SymbolAt::BodyValuePath {
                body, scope, path, ..
            } => self.navigation_targets_for_body_value_path(body, scope, &path),
            SymbolAt::Def { def, .. } => Ok(self
                .targets()
                .navigation_target_for_def(def)?
                .into_iter()
                .collect()),
            SymbolAt::Expr { body, expr } => {
                let targets = self
                    .0
                    .body_ir
                    .body_data(body)?
                    .and_then(|body_data| {
                        body_data.expr(expr).map(|expr_data| (body_data, expr_data))
                    })
                    .map(|(body_data, expr_data)| {
                        self.navigation_targets_for_resolution(body_data, &expr_data.resolution)
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(targets)
            }
            SymbolAt::Field { field, .. } => Ok(self
                .targets()
                .navigation_target_for_field(field)?
                .into_iter()
                .collect()),
            SymbolAt::Function { function, .. } => Ok(self
                .targets()
                .navigation_target_for_function(function)?
                .into_iter()
                .collect()),
            SymbolAt::EnumVariant { variant, .. } => Ok(self
                .targets()
                .navigation_target_for_enum_variant(variant)?
                .into_iter()
                .collect()),
            SymbolAt::LocalItem { item, .. } => Ok(self
                .targets()
                .navigation_target_for_body_item(item)?
                .into_iter()
                .collect()),
            SymbolAt::TypePath { context, path, .. } => {
                self.navigation_targets_for_type_path(context, &path)
            }
            SymbolAt::UsePath { module, path, .. } => {
                self.navigation_targets_for_use_path(module, &path)
            }
            SymbolAt::Body { .. } => Ok(Vec::new()),
        }
    }

    fn targets(&self) -> NavigationTargetResolver<'_, 'db> {
        NavigationTargetResolver::new(self.0)
    }

    fn navigation_targets_for_resolution(
        &self,
        body: &rg_body_ir::BodyData,
        resolution: &BodyResolution,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        // Body resolution can point at lexical bindings, body-local items, or semantic items.
        // Normalize each source of identity into the same navigation payload.
        match resolution {
            BodyResolution::Local(binding) => Ok(body
                .binding(*binding)
                .map(|binding_data| {
                    NavigationTarget::from_binding(body.owner().target, binding_data)
                })
                .into_iter()
                .collect()),
            BodyResolution::LocalItem(item) => Ok(self
                .targets()
                .navigation_target_for_body_item(*item)?
                .into_iter()
                .collect()),
            BodyResolution::Item(defs) => {
                let mut targets = Vec::new();
                for def in defs {
                    if let Some(target) = self.targets().navigation_target_for_def(*def)? {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            BodyResolution::Field(fields) => {
                let mut targets = Vec::new();
                for field in fields {
                    if let Some(target) = self
                        .targets()
                        .navigation_target_for_resolved_field(*field)?
                    {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            BodyResolution::Function(functions) | BodyResolution::Method(functions) => {
                let mut targets = Vec::new();
                for function in functions {
                    if let Some(target) = self
                        .targets()
                        .navigation_target_for_resolved_function(*function)?
                    {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            BodyResolution::EnumVariant(variants) => {
                let mut targets = Vec::new();
                for variant in variants {
                    if let Some(target) = self
                        .targets()
                        .navigation_target_for_enum_variant(*variant)?
                    {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            BodyResolution::Unknown => Ok(Vec::new()),
        }
    }

    fn navigation_targets_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let resolution = self
            .0
            .semantic_ir
            .resolve_type_path(&self.0.def_map, context, path)?;

        let targets = self.navigation_targets_for_semantic_type_path_resolution(resolution)?;
        if targets.is_empty() {
            // A cursor can sit on a non-type prefix inside a type path, for example `helper` in
            // `helper::Tool`. Semantic type resolution correctly says "not a type", but editor
            // navigation should still use DefMap to jump to the module/crate prefix.
            self.navigation_targets_for_use_path(context.module, path)
        } else {
            Ok(targets)
        }
    }

    fn navigation_targets_for_use_path(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let mut targets = Vec::new();
        for def in self.0.def_map.resolve_path(module, path)?.resolved {
            if let Some(target) = self.targets().navigation_target_for_def(def)? {
                targets.push(target);
            }
        }
        Ok(targets)
    }

    fn navigation_targets_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let resolution = self.0.body_ir.resolve_type_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            body_ref,
            scope,
            path,
        )?;

        let targets = self.navigation_targets_for_body_type_path_resolution(resolution)?;
        if targets.is_empty() {
            // Body-local type resolution owns `Self` and local items. If that fails, the path may
            // still be a module/crate prefix selected by the cursor, so fall back to the owning
            // module's DefMap lookup.
            let Some(body) = self.0.body_ir.body_data(body_ref)? else {
                return Ok(Vec::new());
            };
            self.navigation_targets_for_use_path(body.owner_module(), path)
        } else {
            Ok(targets)
        }
    }

    fn navigation_targets_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let (resolution, _) = self.0.body_ir.resolve_value_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            body_ref,
            scope,
            path,
        )?;

        let Some(body_data) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        self.navigation_targets_for_resolution(body_data, &resolution)
    }

    fn navigation_targets_for_semantic_type_path_resolution(
        &self,
        resolution: SemanticTypePathResolution,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        // Type paths can legally resolve to traits in bound positions, so goto-definition should
        // navigate to those traits instead of treating them as unknown.
        match resolution {
            SemanticTypePathResolution::SelfType(types)
            | SemanticTypePathResolution::TypeDefs(types) => {
                let mut targets = Vec::new();
                for ty in types {
                    if let Some(target) = self.targets().navigation_target_for_type_def(ty)? {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            SemanticTypePathResolution::Traits(traits) => {
                let mut targets = Vec::new();
                for trait_ref in traits {
                    if let Some(target) = self.targets().navigation_target_for_trait(trait_ref)? {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            SemanticTypePathResolution::Unknown => Ok(Vec::new()),
        }
    }

    fn navigation_targets_for_body_type_path_resolution(
        &self,
        resolution: BodyTypePathResolution,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => Ok(self
                .targets()
                .navigation_target_for_body_item(item)?
                .into_iter()
                .collect()),
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                let mut targets = Vec::new();
                for ty in types {
                    if let Some(target) = self.targets().navigation_target_for_type_def(ty)? {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            BodyTypePathResolution::Traits(traits) => {
                let mut targets = Vec::new();
                for trait_ref in traits {
                    if let Some(target) = self.targets().navigation_target_for_trait(trait_ref)? {
                        targets.push(target);
                    }
                }
                Ok(targets)
            }
            BodyTypePathResolution::Unknown => Ok(Vec::new()),
        }
    }
}

pub(super) struct GotoResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> GotoResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn goto_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        SymbolResolver::new(self.0).resolve_symbol(symbol)
    }
}

pub(super) struct TypeDefinitionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> TypeDefinitionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn goto_type_definition(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(ty) = super::ty::TypeResolver::new(self.0).type_at(target, file_id, offset)?
        else {
            return Ok(Vec::new());
        };

        NavigationTargetResolver::new(self.0).navigation_targets_for_body_ty(&ty)
    }
}
