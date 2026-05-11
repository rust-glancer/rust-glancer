//! Resolves analysis cursor symbols into semantic/body identities.
//!
//! Navigation and hover need different presentation payloads, but they start from the same core
//! question: "what declaration-like entity does this cursor symbol denote?"

use rg_body_ir::{
    BodyItemRef, BodyRef, BodyResolution, BodyTypePathResolution, ResolvedFieldRef,
    ResolvedFunctionRef, ScopeId,
};
use rg_def_map::{DefId, LocalDefRef, ModuleRef, Path};
use rg_semantic_ir::{
    ConstRef, EnumVariantRef, FunctionRef, ItemId, SemanticTypePathResolution, StaticRef, TraitRef,
    TypeAliasRef, TypeDefId, TypeDefRef,
};

use super::{Analysis, data::SymbolAt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResolvedEntity {
    Module {
        module: ModuleRef,
        display_name: Option<String>,
    },
    TypeDef(TypeDefRef),
    Trait(TraitRef),
    Function(ResolvedFunctionRef),
    Field(ResolvedFieldRef),
    EnumVariant(EnumVariantRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
    LocalBinding {
        body: BodyRef,
        binding: rg_body_ir::BindingId,
    },
    LocalItem(BodyItemRef),
    LocalDef(LocalDefRef),
}

pub(super) struct EntityResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> EntityResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn entities_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<ResolvedEntity>> {
        match symbol {
            SymbolAt::Body { .. } => Ok(Vec::new()),
            SymbolAt::Binding { body, binding } => {
                Ok(vec![ResolvedEntity::LocalBinding { body, binding }])
            }
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
            SymbolAt::Function { function, .. } => Ok(vec![ResolvedEntity::Function(
                ResolvedFunctionRef::Semantic(function),
            )]),
            SymbolAt::EnumVariant { variant, .. } => Ok(vec![ResolvedEntity::EnumVariant(variant)]),
            SymbolAt::LocalItem { item, .. } => Ok(vec![ResolvedEntity::LocalItem(item)]),
            SymbolAt::TypePath { context, path, .. } => {
                let resolution =
                    self.0
                        .semantic_ir
                        .resolve_type_path(&self.0.def_map, context, &path)?;
                let entities = self.entities_for_semantic_type_path_resolution(resolution);
                if entities.is_empty() {
                    self.entities_for_use_path(context.module, &path)
                } else {
                    Ok(entities)
                }
            }
            SymbolAt::UsePath { module, path, .. } => self.entities_for_use_path(module, &path),
        }
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
        let Some(target_ir) = self.0.semantic_ir.target_ir(local_def.target)? else {
            return Ok(None);
        };
        let Some(item) = target_ir.item_for_local_def(local_def.local_def) else {
            return Ok(None);
        };

        let entity = match item {
            ItemId::Struct(id) => ResolvedEntity::TypeDef(TypeDefRef {
                target: local_def.target,
                id: TypeDefId::Struct(id),
            }),
            ItemId::Union(id) => ResolvedEntity::TypeDef(TypeDefRef {
                target: local_def.target,
                id: TypeDefId::Union(id),
            }),
            ItemId::Enum(id) => ResolvedEntity::TypeDef(TypeDefRef {
                target: local_def.target,
                id: TypeDefId::Enum(id),
            }),
            ItemId::Trait(id) => ResolvedEntity::Trait(TraitRef {
                target: local_def.target,
                id,
            }),
            ItemId::Function(id) => {
                ResolvedEntity::Function(ResolvedFunctionRef::Semantic(FunctionRef {
                    target: local_def.target,
                    id,
                }))
            }
            ItemId::TypeAlias(id) => ResolvedEntity::TypeAlias(TypeAliasRef {
                target: local_def.target,
                id,
            }),
            ItemId::Const(id) => ResolvedEntity::Const(ConstRef {
                target: local_def.target,
                id,
            }),
            ItemId::Static(id) => ResolvedEntity::Static(StaticRef {
                target: local_def.target,
                id,
            }),
        };
        Ok(Some(entity))
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
                .map(|body| ResolvedEntity::LocalBinding {
                    body,
                    binding: *binding,
                })
                .into_iter()
                .collect()),
            BodyResolution::LocalItem(item) => Ok(vec![ResolvedEntity::LocalItem(*item)]),
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
            BodyResolution::Field(fields) => {
                Ok(fields.iter().copied().map(ResolvedEntity::Field).collect())
            }
            BodyResolution::Function(functions) | BodyResolution::Method(functions) => {
                Ok(functions
                    .iter()
                    .copied()
                    .map(ResolvedEntity::Function)
                    .collect())
            }
            BodyResolution::EnumVariant(variants) => Ok(variants
                .iter()
                .copied()
                .map(ResolvedEntity::EnumVariant)
                .collect()),
            BodyResolution::Unknown => Ok(Vec::new()),
        }
    }

    fn entities_for_semantic_type_path_resolution(
        &self,
        resolution: SemanticTypePathResolution,
    ) -> Vec<ResolvedEntity> {
        match resolution {
            SemanticTypePathResolution::SelfType(types)
            | SemanticTypePathResolution::TypeDefs(types) => {
                types.into_iter().map(ResolvedEntity::TypeDef).collect()
            }
            SemanticTypePathResolution::Traits(traits) => {
                traits.into_iter().map(ResolvedEntity::Trait).collect()
            }
            SemanticTypePathResolution::Unknown => Vec::new(),
        }
    }

    fn entities_for_body_type_path_resolution(
        &self,
        resolution: BodyTypePathResolution,
    ) -> Vec<ResolvedEntity> {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => vec![ResolvedEntity::LocalItem(item)],
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                types.into_iter().map(ResolvedEntity::TypeDef).collect()
            }
            BodyTypePathResolution::Traits(traits) => {
                traits.into_iter().map(ResolvedEntity::Trait).collect()
            }
            BodyTypePathResolution::Unknown => Vec::new(),
        }
    }
}
