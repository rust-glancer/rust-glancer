//! Document symbol query for editor outlines.

use anyhow::Result;
use rg_body_ir::{
    BodyData, BodyFunctionId, BodyFunctionRef, BodyId, BodyImplData, BodyImplId, BodyImplRef,
    BodyItemId, BodyItemRef, BodyRef, BodyValueItemId, BodyValueItemRef,
};
use rg_def_map::TargetRef;
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    AssocItemId, ConstRef, EnumVariantRef, FunctionRef, SemanticItemKind, SemanticItemView,
    TypeAliasRef, TypeDefId, TypeDefRef,
};

use super::shared;
use crate::{
    api::{
        Analysis,
        view::declaration::{Declaration, DeclarationRef, DeclarationView},
    },
    model::{DocumentSymbol, SymbolKind},
};

pub(crate) struct DocumentSymbolCollector<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> DocumentSymbolCollector<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        let mut symbols = Vec::new();

        self.push_module_document_symbols(target, file_id, &mut symbols)?;
        self.push_semantic_document_symbols(target, file_id, &mut symbols)?;

        // Body-local items belong to their owning function in an editor outline. The semantic
        // function symbol may itself be top-level, trait-owned, or impl-owned, so attachment walks
        // the already-built hierarchy instead of duplicating owner-specific logic.
        self.attach_body_local_document_symbols(target, file_id, &mut symbols)?;

        let mut symbols = Self::nest_module_document_symbols(symbols);
        Self::sort_document_symbols(&mut symbols);
        Ok(symbols)
    }

    fn push_module_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (module_ref, _) in self.0.def_map.modules(target)? {
            let Some(symbol) = self.declaration(module_ref)?.map(DocumentSymbol::from) else {
                continue;
            };
            if symbol.file_id != file_id {
                continue;
            }

            symbols.push(symbol);
        }

        Ok(())
    }

    fn push_semantic_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for item in self.0.semantic_ir.semantic_items(target)? {
            if item.module_owner().is_none() || item.source().file_id != file_id {
                continue;
            }

            if let Some(symbol) = self.semantic_document_symbol(item, file_id)? {
                symbols.push(symbol);
            }
        }

        Ok(())
    }

    fn semantic_document_symbol(
        &self,
        item: SemanticItemView<'_>,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbol>> {
        match item.kind() {
            SemanticItemKind::Struct | SemanticItemKind::Union => {
                let Some(ty) = item.type_def() else {
                    return Ok(None);
                };
                self.type_def_document_symbol(ty, file_id)
            }
            SemanticItemKind::Enum => {
                let Some(ty) = item.type_def() else {
                    return Ok(None);
                };
                self.enum_document_symbol(ty, file_id)
            }
            SemanticItemKind::Trait | SemanticItemKind::Impl => {
                let Some(declaration) = self.declaration(item.item())? else {
                    return Ok(None);
                };
                let children = item
                    .assoc_items()
                    .map(|items| {
                        self.assoc_item_document_symbols(item.item().target(), items, file_id)
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(Some(
                    DocumentSymbol::from(declaration).with_children(children),
                ))
            }
            SemanticItemKind::Function
            | SemanticItemKind::TypeAlias
            | SemanticItemKind::Const
            | SemanticItemKind::Static => self.declaration_document_symbol(item.item()),
        }
    }

    fn type_def_document_symbol(
        &self,
        ty: TypeDefRef,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbol>> {
        let Some(declaration) = self.declaration(ty)? else {
            return Ok(None);
        };
        if declaration.file_id() != file_id {
            return Ok(None);
        };

        let mut children = Vec::new();
        for field_ref in self.0.semantic_ir.fields_for_type(ty)? {
            let Some(symbol) = self.declaration_document_symbol(field_ref)? else {
                continue;
            };
            if symbol.file_id == file_id {
                children.push(symbol);
            }
        }

        Ok(Some(
            DocumentSymbol::from(declaration).with_children(children),
        ))
    }

    fn enum_document_symbol(
        &self,
        ty: TypeDefRef,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbol>> {
        let Some(declaration) = self.declaration(ty)? else {
            return Ok(None);
        };
        if declaration.file_id() != file_id {
            return Ok(None);
        };
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(None);
        };
        let mut children = Vec::new();
        let Some(data) = self.0.semantic_ir.enum_data_for_type_def(ty)? else {
            return Ok(None);
        };
        for index in 0..data.variants.len() {
            let variant_ref = EnumVariantRef {
                target: ty.target,
                enum_id,
                index,
            };
            let Some(declaration) = self.declaration(variant_ref)? else {
                continue;
            };
            let Some(variant) = self.0.semantic_ir.enum_variant_data(variant_ref)? else {
                continue;
            };
            children.push(
                DocumentSymbol::from(declaration).with_children(
                    variant
                        .variant
                        .fields
                        .fields()
                        .iter()
                        .map(|field| {
                            Self::field_document_symbol(
                                file_id,
                                shared::field_label(field.key_declaration_label()),
                                field.span,
                            )
                        })
                        .collect(),
                ),
            );
        }

        Ok(Some(
            DocumentSymbol::from(declaration).with_children(children),
        ))
    }

    fn assoc_item_document_symbols(
        &self,
        target: TargetRef,
        items: &[AssocItemId],
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        let mut symbols = Vec::new();

        for item in items {
            match item {
                AssocItemId::Function(id) => {
                    let function_ref = FunctionRef { target, id: *id };
                    let Some(symbol) = self.declaration_document_symbol(function_ref)? else {
                        continue;
                    };
                    if symbol.file_id == file_id {
                        symbols.push(symbol);
                    }
                }
                AssocItemId::TypeAlias(id) => {
                    let type_alias_ref = TypeAliasRef { target, id: *id };
                    let Some(symbol) = self.declaration_document_symbol(type_alias_ref)? else {
                        continue;
                    };
                    if symbol.file_id == file_id {
                        symbols.push(symbol);
                    }
                }
                AssocItemId::Const(id) => {
                    let const_ref = ConstRef { target, id: *id };
                    let Some(symbol) = self.declaration_document_symbol(const_ref)? else {
                        continue;
                    };
                    if symbol.file_id == file_id {
                        symbols.push(symbol);
                    }
                }
            }
        }

        Ok(symbols)
    }

    fn declaration_document_symbol(
        &self,
        declaration: impl Into<DeclarationRef>,
    ) -> Result<Option<DocumentSymbol>> {
        Ok(self.declaration(declaration)?.map(DocumentSymbol::from))
    }

    fn attach_body_local_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut [DocumentSymbol],
    ) -> Result<()> {
        let Some(target_bodies) = self.0.body_ir.target_bodies(target)? else {
            return Ok(());
        };

        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source().file_id != file_id {
                continue;
            }

            let body_ref = BodyRef {
                target,
                body: BodyId(body_idx),
            };
            let mut children = self.body_local_document_symbols(body_ref, body, file_id)?;
            if children.is_empty() {
                continue;
            }

            let Some(function) = self.declaration(body.owner())? else {
                continue;
            };
            // Body-local structs and impls should appear under the function that contains them,
            // regardless of whether that function is module-owned or associated.
            if let Some(parent) = Self::find_function_symbol_mut(symbols, &function) {
                parent.children.append(&mut children);
            }
        }

        Ok(())
    }

    fn body_local_document_symbols(
        &self,
        body_ref: BodyRef,
        body: &BodyData,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbol>> {
        let mut symbols = Vec::new();

        for (item_idx, item) in body.local_items().iter().enumerate() {
            if item.source.file_id == file_id
                && matches!(item.owner, rg_body_ir::BodyItemOwner::LocalScope(_))
            {
                let item_ref = BodyItemRef {
                    body: body_ref,
                    item: BodyItemId(item_idx),
                };
                let Some(symbol) = self.body_item_document_symbol(item_ref)? else {
                    continue;
                };
                symbols.push(symbol);
            }
        }

        for (item_idx, item) in body.local_value_items().iter().enumerate() {
            if item.source.file_id == file_id
                && matches!(item.owner, rg_body_ir::BodyValueItemOwner::LocalScope(_))
            {
                let item_ref = BodyValueItemRef {
                    body: body_ref,
                    item: BodyValueItemId(item_idx),
                };
                let Some(symbol) = self.declaration_document_symbol(item_ref)? else {
                    continue;
                };
                symbols.push(symbol);
            }
        }

        for (function_idx, function) in body.local_functions().iter().enumerate() {
            if function.source.file_id == file_id
                && matches!(function.owner, rg_body_ir::BodyFunctionOwner::LocalScope(_))
            {
                let function = BodyFunctionRef {
                    body: body_ref,
                    function: BodyFunctionId(function_idx),
                };
                let Some(symbol) = self.declaration_document_symbol(function)? else {
                    continue;
                };
                symbols.push(symbol);
            }
        }

        for (impl_idx, impl_data) in body.local_impls().iter().enumerate() {
            if impl_data.source.file_id == file_id {
                let impl_ref = BodyImplRef {
                    body: body_ref,
                    impl_id: BodyImplId(impl_idx),
                };
                let Some(symbol) = self.body_impl_document_symbol(impl_ref, impl_data)? else {
                    continue;
                };
                symbols.push(symbol);
            }
        }

        Ok(symbols)
    }

    fn body_item_document_symbol(&self, item_ref: BodyItemRef) -> Result<Option<DocumentSymbol>> {
        let Some(declaration) = self.declaration(item_ref)? else {
            return Ok(None);
        };

        let mut children = Vec::new();
        for field_ref in self.0.body_ir.fields_for_local_type(item_ref)? {
            let Some(symbol) = self.declaration_document_symbol(field_ref)? else {
                continue;
            };
            if symbol.file_id == declaration.file_id() {
                children.push(symbol);
            }
        }

        Ok(Some(
            DocumentSymbol::from(declaration).with_children(children),
        ))
    }

    fn body_impl_document_symbol(
        &self,
        impl_ref: BodyImplRef,
        impl_data: &BodyImplData,
    ) -> Result<Option<DocumentSymbol>> {
        let mut children = Vec::new();

        for item in &impl_data.types {
            let item_ref = BodyItemRef {
                body: impl_ref.body,
                item: *item,
            };
            let Some(symbol) = self.body_item_document_symbol(item_ref)? else {
                continue;
            };
            children.push(symbol);
        }

        for item in &impl_data.consts {
            let item_ref = BodyValueItemRef {
                body: impl_ref.body,
                item: *item,
            };
            let Some(symbol) = self.declaration_document_symbol(item_ref)? else {
                continue;
            };
            children.push(symbol);
        }

        for function in &impl_data.functions {
            let function = BodyFunctionRef {
                body: impl_ref.body,
                function: *function,
            };
            let Some(symbol) = self.declaration_document_symbol(function)? else {
                continue;
            };
            children.push(symbol);
        }

        let Some(declaration) = self.declaration(impl_ref)? else {
            return Ok(None);
        };

        Ok(Some(
            DocumentSymbol::from(declaration).with_children(children),
        ))
    }

    fn find_function_symbol_mut<'s>(
        symbols: &'s mut [DocumentSymbol],
        function: &Declaration,
    ) -> Option<&'s mut DocumentSymbol> {
        // Associated functions may already be nested below traits or impls, so search the outline
        // tree instead of assuming module-level placement.
        for symbol in symbols {
            if symbol.name == function.name()
                && symbol.span == function.span()
                && matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
            {
                return Some(symbol);
            }
            if let Some(found) = Self::find_function_symbol_mut(&mut symbol.children, function) {
                return Some(found);
            }
        }

        None
    }

    fn field_document_symbol(file_id: FileId, name: String, span: Span) -> DocumentSymbol {
        DocumentSymbol {
            name,
            kind: SymbolKind::Field,
            file_id,
            span,
            selection_span: span,
            children: Vec::new(),
        }
    }

    fn declaration(&self, declaration: impl Into<DeclarationRef>) -> Result<Option<Declaration>> {
        DeclarationView::new(self.0).declaration(declaration.into())
    }

    fn nest_module_document_symbols(symbols: Vec<DocumentSymbol>) -> Vec<DocumentSymbol> {
        let parent_by_symbol = Self::module_parents_by_symbol(&symbols);
        let mut children_by_parent = vec![Vec::new(); symbols.len()];
        let mut roots = Vec::new();

        for (symbol_idx, parent) in parent_by_symbol.into_iter().enumerate() {
            match parent {
                Some(parent_idx) => children_by_parent[parent_idx].push(symbol_idx),
                None => roots.push(symbol_idx),
            }
        }

        roots
            .into_iter()
            .map(|idx| Self::build_nested_document_symbol(idx, &symbols, &children_by_parent))
            .collect()
    }

    fn module_parents_by_symbol(symbols: &[DocumentSymbol]) -> Vec<Option<usize>> {
        // Inline module spans contain their nested item spans. Choosing the smallest containing
        // module reconstructs the outline hierarchy without consulting def-map parent ids.
        symbols
            .iter()
            .enumerate()
            .map(|(symbol_idx, symbol)| {
                symbols
                    .iter()
                    .enumerate()
                    .filter(|(module_idx, module)| {
                        *module_idx != symbol_idx
                            && module.kind == SymbolKind::Module
                            && Self::span_strictly_contains(module.span, symbol.span)
                    })
                    .min_by_key(|(_, module)| module.span.len())
                    .map(|(module_idx, _)| module_idx)
            })
            .collect()
    }

    fn build_nested_document_symbol(
        idx: usize,
        symbols: &[DocumentSymbol],
        children_by_parent: &[Vec<usize>],
    ) -> DocumentSymbol {
        let mut symbol = symbols[idx].clone();
        symbol.children.extend(
            children_by_parent[idx]
                .iter()
                .map(|child_idx| {
                    Self::build_nested_document_symbol(*child_idx, symbols, children_by_parent)
                })
                .collect::<Vec<_>>(),
        );
        Self::sort_document_symbols(&mut symbol.children);
        symbol
    }

    fn span_strictly_contains(parent: Span, child: Span) -> bool {
        parent.text.start <= child.text.start
            && child.text.end <= parent.text.end
            && parent.text != child.text
    }

    fn sort_document_symbols(symbols: &mut [DocumentSymbol]) {
        for symbol in symbols.iter_mut() {
            Self::sort_document_symbols(&mut symbol.children);
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.span.text.start,
                symbol.span.text.end,
                symbol.kind,
                symbol.name.clone(),
            )
        });
    }
}
