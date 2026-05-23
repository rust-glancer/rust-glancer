//! Workspace-wide symbol search.

use anyhow::Result;
use rg_body_ir::{ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef};
use rg_def_map::{ModuleId, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    AssocItemId, ConstRef, EnumVariantRef, FunctionRef, ItemOwner, TypeAliasRef, TypeDefId,
    TypeDefRef,
};

use super::shared;
use crate::{
    api::{
        Analysis,
        view::declaration::{DeclarationLookup, DeclarationRef},
    },
    model::{Declaration, SymbolKind, WorkspaceSymbol},
};

pub(crate) struct WorkspaceSymbolCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> WorkspaceSymbolCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>> {
        let query = WorkspaceSymbolQuery::new(query);
        let mut symbols = Vec::new();

        for (target, _) in self.0.semantic_ir.materialize_included_target_irs()? {
            self.push_module_workspace_symbols(target, &query, &mut symbols)?;
            self.push_nominal_workspace_symbols(target, &query, &mut symbols)?;
            self.push_trait_workspace_symbols(target, &query, &mut symbols)?;
            self.push_impl_workspace_symbols(target, &query, &mut symbols)?;
            self.push_function_workspace_symbols(target, &query, &mut symbols)?;
            self.push_type_alias_workspace_symbols(target, &query, &mut symbols)?;
            self.push_const_workspace_symbols(target, &query, &mut symbols)?;
            self.push_static_workspace_symbols(target, &query, &mut symbols)?;
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.name.to_lowercase(),
                symbol.kind,
                symbol.container_name.clone(),
                symbol.target.package.0,
                symbol.target.target.0,
                symbol.file_id.0,
                symbol.span.map(|span| span.text.start),
            )
        });
        Ok(symbols)
    }

    fn push_nominal_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (ty, data) in self.0.semantic_ir.structs(target)? {
            self.push_declaration_workspace_symbol(ty, None, query, symbols)?;
            self.push_field_workspace_symbols(ty, &data.name, query, symbols)?;
        }

        for (ty, data) in self.0.semantic_ir.unions(target)? {
            self.push_declaration_workspace_symbol(ty, None, query, symbols)?;
            self.push_field_workspace_symbols(ty, &data.name, query, symbols)?;
        }

        for (ty, data) in self.0.semantic_ir.enums(target)? {
            self.push_declaration_workspace_symbol(ty, None, query, symbols)?;
            let TypeDefId::Enum(enum_id) = ty.id else {
                continue;
            };
            for (index, _) in data.variants.iter().enumerate() {
                let variant_ref = ResolvedEnumVariantRef::Semantic(EnumVariantRef {
                    target,
                    enum_id,
                    index,
                });
                self.push_declaration_workspace_symbol(
                    variant_ref,
                    Some(data.name.to_string()),
                    query,
                    symbols,
                )?;
            }
        }

        Ok(())
    }

    fn push_module_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (module_ref, _) in self.0.def_map.modules(target)? {
            let Some(declaration) = self.declaration(module_ref)? else {
                continue;
            };

            self.push_workspace_symbol(
                WorkspaceSymbolInput {
                    target: declaration.target,
                    name: declaration.name,
                    kind: declaration.kind,
                    file_id: declaration.file_id,
                    span: Some(declaration.selection_span),
                    container_name: self.module_container_name(module_ref)?,
                },
                query,
                symbols,
            );
        }

        Ok(())
    }

    fn push_trait_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (trait_ref, data) in self.0.semantic_ir.traits(target)? {
            self.push_declaration_workspace_symbol(trait_ref, None, query, symbols)?;
            self.push_assoc_item_workspace_symbols(
                target,
                &data.items,
                &shared::trait_label(data),
                query,
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_impl_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (impl_ref, data) in self.0.semantic_ir.impls(target)? {
            let Some(declaration) = self.declaration(impl_ref)? else {
                continue;
            };
            self.push_assoc_item_workspace_symbols(
                impl_ref.target,
                &data.items,
                &declaration.name,
                query,
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_function_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (function, data) in self.0.semantic_ir.functions(target)? {
            if !matches!(data.owner, ItemOwner::Module(_)) {
                continue;
            }
            self.push_declaration_workspace_symbol(
                ResolvedFunctionRef::Semantic(function),
                None,
                query,
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_type_alias_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (type_alias_ref, data) in self.0.semantic_ir.type_aliases(target)? {
            if !matches!(data.owner, ItemOwner::Module(_)) {
                continue;
            }
            self.push_declaration_workspace_symbol(type_alias_ref, None, query, symbols)?;
        }

        Ok(())
    }

    fn push_const_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (const_ref, data) in self.0.semantic_ir.consts(target)? {
            if !matches!(data.owner, ItemOwner::Module(_)) {
                continue;
            }
            self.push_declaration_workspace_symbol(const_ref, None, query, symbols)?;
        }

        Ok(())
    }

    fn push_static_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for (static_ref, _) in self.0.semantic_ir.statics(target)? {
            self.push_declaration_workspace_symbol(static_ref, None, query, symbols)?;
        }

        Ok(())
    }

    fn push_assoc_item_workspace_symbols(
        &self,
        target: TargetRef,
        items: &[AssocItemId],
        container_name: &str,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for item in items {
            match item {
                AssocItemId::Function(id) => {
                    let function_ref = FunctionRef { target, id: *id };
                    self.push_declaration_workspace_symbol(
                        ResolvedFunctionRef::Semantic(function_ref),
                        Some(container_name.to_string()),
                        query,
                        symbols,
                    )?;
                }
                AssocItemId::TypeAlias(id) => {
                    let type_alias_ref = TypeAliasRef { target, id: *id };
                    self.push_declaration_workspace_symbol(
                        type_alias_ref,
                        Some(container_name.to_string()),
                        query,
                        symbols,
                    )?;
                }
                AssocItemId::Const(id) => {
                    let const_ref = ConstRef { target, id: *id };
                    self.push_declaration_workspace_symbol(
                        const_ref,
                        Some(container_name.to_string()),
                        query,
                        symbols,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn push_field_workspace_symbols(
        &self,
        ty: TypeDefRef,
        container_name: &str,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for field_ref in self.0.semantic_ir.fields_for_type(ty)? {
            self.push_declaration_workspace_symbol(
                ResolvedFieldRef::Semantic(field_ref),
                Some(container_name.to_string()),
                query,
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_declaration_workspace_symbol(
        &self,
        declaration: impl Into<DeclarationRef>,
        container_name: Option<String>,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        let Some(declaration) = self.declaration(declaration)? else {
            return Ok(());
        };

        self.push_workspace_symbol(
            WorkspaceSymbolInput {
                target: declaration.target,
                name: declaration.name,
                kind: declaration.kind,
                file_id: declaration.file_id,
                span: Some(declaration.selection_span),
                container_name,
            },
            query,
            symbols,
        );

        Ok(())
    }

    fn declaration(&self, declaration: impl Into<DeclarationRef>) -> Result<Option<Declaration>> {
        DeclarationLookup::new(self.0).declaration(declaration.into())
    }

    fn push_workspace_symbol(
        &self,
        input: WorkspaceSymbolInput,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) {
        if !query.matches(&input.name) {
            return;
        }

        symbols.push(WorkspaceSymbol {
            target: input.target,
            name: input.name,
            kind: input.kind,
            file_id: input.file_id,
            span: input.span,
            container_name: input.container_name,
        });
    }

    fn module_container_name(&self, module_ref: ModuleRef) -> Result<Option<String>> {
        let Some(module) = self.0.def_map.module(module_ref)? else {
            return Ok(None);
        };
        let Some(parent) = module.parent else {
            return Ok(None);
        };
        let path = self.module_path(module_ref.target, parent)?;

        Ok((!path.is_empty()).then_some(path))
    }

    fn module_path(&self, target: TargetRef, module: ModuleId) -> Result<String> {
        let Some(data) = self.0.def_map.module(ModuleRef { target, module })? else {
            return Ok(String::new());
        };
        let Some(name) = &data.name else {
            return Ok(String::new());
        };
        let Some(parent) = data.parent else {
            return Ok(name.to_string());
        };

        let parent_path = self.module_path(target, parent)?;
        if parent_path.is_empty() {
            Ok(name.to_string())
        } else {
            Ok(format!("{parent_path}::{name}"))
        }
    }
}

struct WorkspaceSymbolInput {
    target: TargetRef,
    name: String,
    kind: SymbolKind,
    file_id: FileId,
    span: Option<Span>,
    container_name: Option<String>,
}

struct WorkspaceSymbolQuery {
    needle: String,
}

impl WorkspaceSymbolQuery {
    fn new(query: &str) -> Self {
        Self {
            needle: query.to_lowercase(),
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.needle.is_empty() || name.to_lowercase().contains(&self.needle)
    }
}
