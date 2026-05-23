//! Workspace-wide symbol search.

use anyhow::Result;
use rg_def_map::{ModuleId, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    AssocItemId, ConstRef, EnumVariantRef, FunctionRef, SemanticItemKind, SemanticItemView,
    TypeAliasRef, TypeDefId, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        view::declaration::{Declaration, DeclarationRef, DeclarationView},
    },
    model::{SymbolKind, WorkspaceSymbol},
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
            self.push_semantic_workspace_symbols(target, &query, &mut symbols)?;
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

    fn push_semantic_workspace_symbols(
        &self,
        target: TargetRef,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        for item in self.0.semantic_ir.semantic_items(target)? {
            if item.module_owner().is_none() {
                continue;
            }

            self.push_semantic_workspace_symbol(item, query, symbols)?;
        }

        Ok(())
    }

    fn push_semantic_workspace_symbol(
        &self,
        item: SemanticItemView<'_>,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        match item.kind() {
            SemanticItemKind::Struct | SemanticItemKind::Union => {
                let Some(ty) = item.type_def() else {
                    return Ok(());
                };
                self.push_declaration_workspace_symbol(item.item(), None, query, symbols)?;
                let Some(name) = item.name() else {
                    return Ok(());
                };
                self.push_field_workspace_symbols(ty, name.as_str(), query, symbols)?;
            }
            SemanticItemKind::Enum => {
                let Some(ty) = item.type_def() else {
                    return Ok(());
                };
                self.push_declaration_workspace_symbol(item.item(), None, query, symbols)?;
                let Some(name) = item.name() else {
                    return Ok(());
                };
                self.push_enum_variant_workspace_symbols(ty, name.as_str(), query, symbols)?;
            }
            SemanticItemKind::Trait => {
                self.push_declaration_workspace_symbol(item.item(), None, query, symbols)?;
                let Some(name) = item.name() else {
                    return Ok(());
                };
                let Some(items) = item.assoc_items() else {
                    return Ok(());
                };
                let container_name = format!("trait {name}");
                self.push_assoc_item_workspace_symbols(
                    item.item().target(),
                    items,
                    &container_name,
                    query,
                    symbols,
                )?;
            }
            SemanticItemKind::Impl => {
                let Some(declaration) = self.declaration(item.item())? else {
                    return Ok(());
                };
                let Some(items) = item.assoc_items() else {
                    return Ok(());
                };
                self.push_assoc_item_workspace_symbols(
                    item.item().target(),
                    items,
                    declaration.name(),
                    query,
                    symbols,
                )?;
            }
            SemanticItemKind::Function
            | SemanticItemKind::TypeAlias
            | SemanticItemKind::Const
            | SemanticItemKind::Static => {
                self.push_declaration_workspace_symbol(item.item(), None, query, symbols)?;
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
                    target: declaration.target(),
                    name: declaration.name().to_string(),
                    kind: declaration.kind(),
                    file_id: declaration.file_id(),
                    span: Some(declaration.selection_span()),
                    container_name: self.module_container_name(module_ref)?,
                },
                query,
                symbols,
            );
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
                        function_ref,
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

    fn push_enum_variant_workspace_symbols(
        &self,
        ty: TypeDefRef,
        container_name: &str,
        query: &WorkspaceSymbolQuery,
        symbols: &mut Vec<WorkspaceSymbol>,
    ) -> Result<()> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(());
        };
        let Some(data) = self.0.semantic_ir.enum_data_for_type_def(ty)? else {
            return Ok(());
        };
        for index in 0..data.variants.len() {
            let variant_ref = EnumVariantRef {
                target: ty.target,
                enum_id,
                index,
            };
            self.push_declaration_workspace_symbol(
                variant_ref,
                Some(container_name.to_string()),
                query,
                symbols,
            )?;
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
                field_ref,
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
                target: declaration.target(),
                name: declaration.name().to_string(),
                kind: declaration.kind(),
                file_id: declaration.file_id(),
                span: Some(declaration.selection_span()),
                container_name,
            },
            query,
            symbols,
        );

        Ok(())
    }

    fn declaration(&self, declaration: impl Into<DeclarationRef>) -> Result<Option<Declaration>> {
        DeclarationView::new(self.0).declaration(declaration.into())
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
