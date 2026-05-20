//! Symbol-to-navigation resolution.

use rg_body_ir::{BodyRef, BodyResolution, BodyTypePathResolution, ScopeId};
use rg_def_map::{ModuleRef, Path};
use rg_semantic_ir::{SemanticTypePathResolution, TypePathContext};

use super::target;
use crate::{
    api::Analysis,
    model::{NavigationTarget, SymbolAt},
};

/// Resolves an already-selected analysis symbol into navigation destinations.
///
/// `SymbolAt` is cursor vocabulary, not a declaration identity. This resolver performs the
/// cross-IR lookups, path fallbacks, and body-resolution handling needed to turn one cursor symbol
/// into zero or more concrete targets.
pub(crate) struct SymbolResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> SymbolResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn resolve_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<NavigationTarget>> {
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
            SymbolAt::LocalEnumVariant { variant, .. } => Ok(self
                .targets()
                .navigation_target_for_resolved_enum_variant(
                    rg_body_ir::ResolvedEnumVariantRef::BodyLocal(variant),
                )?
                .into_iter()
                .collect()),
            SymbolAt::LocalItem { item, .. } => Ok(self
                .targets()
                .navigation_target_for_body_item(item)?
                .into_iter()
                .collect()),
            SymbolAt::LocalValueItem { item, .. } => Ok(self
                .targets()
                .navigation_target_for_body_value_item(item)?
                .into_iter()
                .collect()),
            SymbolAt::LocalField { field, .. } => Ok(self
                .targets()
                .navigation_target_for_resolved_field(rg_body_ir::ResolvedFieldRef::BodyLocal(
                    field,
                ))?
                .into_iter()
                .collect()),
            SymbolAt::LocalFunction { function, .. } => Ok(self
                .targets()
                .navigation_target_for_resolved_function(
                    rg_body_ir::ResolvedFunctionRef::BodyLocal(function),
                )?
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

    fn targets(&self) -> target::NavigationTargetResolver<'_, 'db> {
        target::NavigationTargetResolver::new(self.0)
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
            BodyResolution::LocalValueItem(item) => Ok(self
                .targets()
                .navigation_target_for_body_value_item(*item)?
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
                        .navigation_target_for_resolved_enum_variant(*variant)?
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
