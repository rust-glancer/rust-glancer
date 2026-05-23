//! Resolves analysis cursor symbols into semantic/body identities.
//!
//! Navigation and hover need different presentation payloads, but they start from the same core
//! question: "what declaration-like entity does this cursor symbol denote?"

use rg_body_ir::{
    BodyBindingRef, BodyDeclarationRef, BodyRef, BodyResolution, BodyTypePathResolution,
    ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef, ScopeId,
};
use rg_def_map::{DefId, LocalDefRef, ModuleRef, Path};
use rg_semantic_ir::SemanticItemRef;

use crate::{api::Analysis, model::SymbolAt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedEntity {
    Module {
        module: ModuleRef,
        display_name: Option<String>,
    },
    SemanticItem(SemanticItemRef),
    BodyDeclaration(BodyDeclarationRef),
    Field(ResolvedFieldRef),
    EnumVariant(ResolvedEnumVariantRef),
    LocalDef(LocalDefRef),
}

pub(crate) struct EntityResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> EntityResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn entities_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        match symbol {
            SymbolAt::Body { .. } => Ok(Vec::new()),
            SymbolAt::Binding { body, binding } => Ok(vec![ResolvedEntity::BodyDeclaration(
                BodyBindingRef { body, binding }.into(),
            )]),
            SymbolAt::BodyPath {
                body, scope, path, ..
            } => self.entities_for_body_type_path(body, scope, &path),
            SymbolAt::BodyValuePath {
                body, scope, path, ..
            } => self.entities_for_body_value_path(body, scope, &path),
            SymbolAt::Def { def, .. } => self.entities_for_def(def),
            SymbolAt::Expr { body, expr } => {
                let Some(body_data) = self.0.body_ir.body_data(body)? else {
                    return Ok(Vec::new());
                };
                let Some(expr_data) = body_data.expr(expr) else {
                    return Ok(Vec::new());
                };
                self.entities_for_body_resolution(Some(body), &expr_data.resolution, None)
            }
            SymbolAt::Field { field, .. } => Ok(vec![ResolvedEntity::Field(
                ResolvedFieldRef::Semantic(field),
            )]),
            SymbolAt::Function { function, .. } => {
                Ok(vec![ResolvedEntity::SemanticItem(function.into())])
            }
            SymbolAt::EnumVariant { variant, .. } => Ok(vec![ResolvedEntity::EnumVariant(
                ResolvedEnumVariantRef::Semantic(variant),
            )]),
            SymbolAt::LocalEnumVariant { variant, .. } => {
                Ok(vec![ResolvedEntity::BodyDeclaration(variant.into())])
            }
            SymbolAt::LocalItem { item, .. } => {
                Ok(vec![ResolvedEntity::BodyDeclaration(item.into())])
            }
            SymbolAt::LocalValueItem { item, .. } => {
                Ok(vec![ResolvedEntity::BodyDeclaration(item.into())])
            }
            SymbolAt::LocalField { field, .. } => {
                Ok(vec![ResolvedEntity::BodyDeclaration(field.into())])
            }
            SymbolAt::LocalFunction { function, .. } => {
                Ok(vec![ResolvedEntity::BodyDeclaration(function.into())])
            }
            SymbolAt::TypePath { context, path, .. } => {
                let entities = self.entities_for_semantic_type_path(context, &path)?;
                if entities.is_empty() {
                    self.entities_for_use_path(context.module, &path)
                } else {
                    Ok(entities)
                }
            }
            SymbolAt::UsePath { module, path, .. } => self.entities_for_use_path(module, &path),
        }
    }

    fn entities_for_semantic_type_path(
        &self,
        context: rg_semantic_ir::TypePathContext,
        path: &Path,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        Ok(self
            .0
            .semantic_ir
            .semantic_items_for_type_path(&self.0.def_map, context, path)?
            .into_iter()
            .map(ResolvedEntity::SemanticItem)
            .collect())
    }

    fn entities_for_def(&self, def: DefId) -> anyhow::Result<Vec<ResolvedEntity>> {
        self.entities_for_def_with_module_display(def, None)
    }

    fn entities_for_def_with_module_display(
        &self,
        def: DefId,
        display_name: Option<String>,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        match def {
            DefId::Module(module) => Ok(vec![ResolvedEntity::Module {
                module,
                display_name,
            }]),
            DefId::Local(local_def) => {
                Ok(vec![self.entity_for_local_def(local_def).map(
                    |entity| entity.unwrap_or(ResolvedEntity::LocalDef(local_def)),
                )?])
            }
        }
    }

    fn entity_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<ResolvedEntity>> {
        let Some(item) = self.0.semantic_ir.semantic_item_for_local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(ResolvedEntity::SemanticItem(item)))
    }

    fn entities_for_use_path(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        let display_name = path.last_segment_label();
        let mut entities = Vec::new();
        for def in self.0.def_map.resolve_path(module, path)?.resolved {
            entities.extend(self.entities_for_def_with_module_display(def, display_name.clone())?);
        }
        Ok(entities)
    }

    fn entities_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        let resolution = self.0.body_ir.resolve_type_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            body_ref,
            scope,
            path,
        )?;

        let entities = self.entities_for_body_type_path_resolution(resolution);
        if !entities.is_empty() {
            return Ok(entities);
        }

        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        self.entities_for_use_path(body.owner_module(), path)
    }

    fn entities_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        let (resolution, _) = self.0.body_ir.resolve_value_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            body_ref,
            scope,
            path,
        )?;
        self.entities_for_body_resolution(Some(body_ref), &resolution, path.last_segment_label())
    }

    fn entities_for_body_resolution(
        &self,
        body_ref: Option<BodyRef>,
        resolution: &BodyResolution,
        module_display_name: Option<String>,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        match resolution {
            BodyResolution::Local(binding) => Ok(body_ref
                .map(|body| BodyBindingRef {
                    body,
                    binding: *binding,
                })
                .map(BodyDeclarationRef::from)
                .map(ResolvedEntity::BodyDeclaration)
                .into_iter()
                .collect()),
            BodyResolution::LocalItem(item) => {
                Ok(vec![ResolvedEntity::BodyDeclaration((*item).into())])
            }
            BodyResolution::LocalValueItem(item) => {
                Ok(vec![ResolvedEntity::BodyDeclaration((*item).into())])
            }
            BodyResolution::Item(defs) => {
                let mut entities = Vec::new();
                for def in defs {
                    entities.extend(
                        self.entities_for_def_with_module_display(
                            *def,
                            module_display_name.clone(),
                        )?,
                    );
                }
                Ok(entities)
            }
            BodyResolution::Field(fields) => Ok(fields
                .iter()
                .copied()
                .map(Self::entity_for_resolved_field)
                .collect()),
            BodyResolution::Function(functions) | BodyResolution::Method(functions) => {
                Ok(functions
                    .iter()
                    .copied()
                    .map(Self::entity_for_resolved_function)
                    .collect())
            }
            BodyResolution::EnumVariant(variants) => Ok(variants
                .iter()
                .copied()
                .map(Self::entity_for_resolved_enum_variant)
                .collect()),
            BodyResolution::Unknown => Ok(Vec::new()),
        }
    }

    fn entity_for_resolved_function(function: ResolvedFunctionRef) -> ResolvedEntity {
        match function {
            ResolvedFunctionRef::Semantic(function) => {
                ResolvedEntity::SemanticItem(function.into())
            }
            ResolvedFunctionRef::BodyLocal(function) => {
                ResolvedEntity::BodyDeclaration(function.into())
            }
        }
    }

    fn entity_for_resolved_field(field: ResolvedFieldRef) -> ResolvedEntity {
        match field {
            ResolvedFieldRef::Semantic(field) => {
                ResolvedEntity::Field(ResolvedFieldRef::Semantic(field))
            }
            ResolvedFieldRef::BodyLocal(field) => ResolvedEntity::BodyDeclaration(field.into()),
        }
    }

    fn entity_for_resolved_enum_variant(variant: ResolvedEnumVariantRef) -> ResolvedEntity {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant) => {
                ResolvedEntity::EnumVariant(ResolvedEnumVariantRef::Semantic(variant))
            }
            ResolvedEnumVariantRef::BodyLocal(variant) => {
                ResolvedEntity::BodyDeclaration(variant.into())
            }
        }
    }

    fn entities_for_body_type_path_resolution(
        &self,
        resolution: BodyTypePathResolution,
    ) -> Vec<ResolvedEntity> {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => {
                vec![ResolvedEntity::BodyDeclaration(item.into())]
            }
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                types
                    .into_iter()
                    .map(|ty| ResolvedEntity::SemanticItem(ty.into()))
                    .collect()
            }
            BodyTypePathResolution::Traits(traits) => traits
                .into_iter()
                .map(|trait_ref| ResolvedEntity::SemanticItem(trait_ref.into()))
                .collect(),
            BodyTypePathResolution::Primitive(_) | BodyTypePathResolution::Unknown => Vec::new(),
        }
    }
}
