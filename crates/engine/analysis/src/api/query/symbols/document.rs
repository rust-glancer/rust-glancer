//! Document symbol query for editor outlines.

use anyhow::Result;
use rg_body_ir::{BodyData, BodyImplData, BodyItemData};
use rg_def_map::{LocalDefRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    AssocItemId, ConstData, ConstRef, EnumData, FunctionData, FunctionRef, ItemOwner, StaticData,
    StructData, TypeAliasData, TypeAliasRef, UnionData,
};

use super::shared;
use crate::{
    api::Analysis,
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
        self.push_nominal_document_symbols(target, file_id, &mut symbols)?;
        self.push_trait_document_symbols(target, file_id, &mut symbols)?;
        self.push_impl_document_symbols(target, file_id, &mut symbols)?;
        self.push_module_function_document_symbols(target, file_id, &mut symbols)?;
        self.push_module_type_alias_document_symbols(target, file_id, &mut symbols)?;
        self.push_module_const_document_symbols(target, file_id, &mut symbols)?;
        self.push_static_document_symbols(target, file_id, &mut symbols)?;

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
        for (_, data) in self.0.def_map.modules(target)? {
            let Some(name) = data.name.as_ref().map(ToString::to_string) else {
                continue;
            };
            let Some(source) = shared::module_declaration_source(data) else {
                continue;
            };
            if source.file_id != file_id {
                continue;
            }

            symbols.push(DocumentSymbol {
                name,
                kind: SymbolKind::Module,
                file_id,
                span: source.span,
                selection_span: source.selection_span,
                children: Vec::new(),
            });
        }

        Ok(())
    }

    fn push_nominal_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (_, data) in self.0.semantic_ir.structs(target)? {
            if let Some(symbol) = self.struct_document_symbol(data, file_id)? {
                symbols.push(symbol);
            }
        }
        for (_, data) in self.0.semantic_ir.unions(target)? {
            if let Some(symbol) = self.union_document_symbol(data, file_id)? {
                symbols.push(symbol);
            }
        }
        for (_, data) in self.0.semantic_ir.enums(target)? {
            if let Some(symbol) = self.enum_document_symbol(data, file_id)? {
                symbols.push(symbol);
            }
        }

        Ok(())
    }

    fn struct_document_symbol(
        &self,
        data: &StructData,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbol>> {
        let Some(def) = self.local_def_symbol_source(data.local_def, file_id)? else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbol {
            name: data.name.to_string(),
            kind: SymbolKind::Struct,
            file_id,
            span: def.span,
            selection_span: def.selection_span,
            children: data
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
        }))
    }

    fn union_document_symbol(
        &self,
        data: &UnionData,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbol>> {
        let Some(def) = self.local_def_symbol_source(data.local_def, file_id)? else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbol {
            name: data.name.to_string(),
            kind: SymbolKind::Union,
            file_id,
            span: def.span,
            selection_span: def.selection_span,
            children: data
                .fields
                .iter()
                .map(|field| {
                    Self::field_document_symbol(
                        file_id,
                        shared::field_label(field.key_declaration_label()),
                        field.span,
                    )
                })
                .collect(),
        }))
    }

    fn enum_document_symbol(
        &self,
        data: &EnumData,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbol>> {
        let Some(def) = self.local_def_symbol_source(data.local_def, file_id)? else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbol {
            name: data.name.to_string(),
            kind: SymbolKind::Enum,
            file_id,
            span: def.span,
            selection_span: def.selection_span,
            children: data
                .variants
                .iter()
                .map(|variant| DocumentSymbol {
                    name: variant.name.to_string(),
                    kind: SymbolKind::EnumVariant,
                    file_id,
                    span: variant.span,
                    selection_span: variant.name_span,
                    children: variant
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
                })
                .collect(),
        }))
    }

    fn push_trait_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (_, data) in self.0.semantic_ir.traits(target)? {
            let Some(def) = self.local_def_symbol_source(data.local_def, file_id)? else {
                continue;
            };

            symbols.push(DocumentSymbol {
                name: data.name.to_string(),
                kind: SymbolKind::Trait,
                file_id,
                span: def.span,
                selection_span: def.selection_span,
                children: self.assoc_item_document_symbols(target, &data.items, file_id)?,
            });
        }

        Ok(())
    }

    fn push_impl_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (impl_ref, data) in self.0.semantic_ir.impls(target)? {
            let Some(local_impl) = self.0.def_map.local_impl(data.local_impl)? else {
                continue;
            };
            if local_impl.file_id != file_id {
                continue;
            }

            symbols.push(DocumentSymbol {
                name: shared::impl_label(data),
                kind: SymbolKind::Impl,
                file_id,
                span: local_impl.span,
                selection_span: local_impl.span,
                children: self.assoc_item_document_symbols(
                    impl_ref.target,
                    &data.items,
                    file_id,
                )?,
            });
        }

        Ok(())
    }

    fn push_module_function_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (_, data) in self.0.semantic_ir.functions(target)? {
            if !matches!(data.owner, ItemOwner::Module(_)) || data.source.file_id != file_id {
                continue;
            }
            symbols.push(self.function_document_symbol(data, SymbolKind::Function));
        }

        Ok(())
    }

    fn push_module_type_alias_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (_, data) in self.0.semantic_ir.type_aliases(target)? {
            if !matches!(data.owner, ItemOwner::Module(_)) || data.source.file_id != file_id {
                continue;
            }
            symbols.push(self.type_alias_document_symbol(data));
        }

        Ok(())
    }

    fn push_module_const_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (_, data) in self.0.semantic_ir.consts(target)? {
            if !matches!(data.owner, ItemOwner::Module(_)) || data.source.file_id != file_id {
                continue;
            }
            symbols.push(self.const_document_symbol(data));
        }

        Ok(())
    }

    fn push_static_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbol>,
    ) -> Result<()> {
        for (_, data) in self.0.semantic_ir.statics(target)? {
            if data.source.file_id != file_id {
                continue;
            }
            symbols.push(self.static_document_symbol(data));
        }

        Ok(())
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
                    let Some(data) = self.0.semantic_ir.function_data(function_ref)? else {
                        continue;
                    };
                    if data.source.file_id == file_id {
                        symbols.push(self.function_document_symbol(data, SymbolKind::Method));
                    }
                }
                AssocItemId::TypeAlias(id) => {
                    let type_alias_ref = TypeAliasRef { target, id: *id };
                    let Some(data) = self.0.semantic_ir.type_alias_data(type_alias_ref)? else {
                        continue;
                    };
                    if data.source.file_id == file_id {
                        symbols.push(self.type_alias_document_symbol(data));
                    }
                }
                AssocItemId::Const(id) => {
                    let const_ref = ConstRef { target, id: *id };
                    let Some(data) = self.0.semantic_ir.const_data(const_ref)? else {
                        continue;
                    };
                    if data.source.file_id == file_id {
                        symbols.push(self.const_document_symbol(data));
                    }
                }
            }
        }

        Ok(symbols)
    }

    fn function_document_symbol(&self, data: &FunctionData, kind: SymbolKind) -> DocumentSymbol {
        DocumentSymbol {
            name: data.name.to_string(),
            kind,
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
            children: Vec::new(),
        }
    }

    fn type_alias_document_symbol(&self, data: &TypeAliasData) -> DocumentSymbol {
        DocumentSymbol {
            name: data.name.to_string(),
            kind: SymbolKind::TypeAlias,
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
            children: Vec::new(),
        }
    }

    fn const_document_symbol(&self, data: &ConstData) -> DocumentSymbol {
        DocumentSymbol {
            name: data.name.to_string(),
            kind: SymbolKind::Const,
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
            children: Vec::new(),
        }
    }

    fn static_document_symbol(&self, data: &StaticData) -> DocumentSymbol {
        DocumentSymbol {
            name: data.name.to_string(),
            kind: SymbolKind::Static,
            file_id: data.source.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
            children: Vec::new(),
        }
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

        for body in target_bodies.bodies() {
            if body.source().file_id != file_id {
                continue;
            }

            let mut children = self.body_local_document_symbols(body, file_id);
            if children.is_empty() {
                continue;
            }

            let Some(function) = self.0.semantic_ir.function_data(body.owner())? else {
                continue;
            };
            // Body-local structs and impls should appear under the function that contains them,
            // regardless of whether that function is module-owned or associated.
            if let Some(parent) = Self::find_function_symbol_mut(symbols, function) {
                parent.children.append(&mut children);
            }
        }

        Ok(())
    }

    fn body_local_document_symbols(&self, body: &BodyData, file_id: FileId) -> Vec<DocumentSymbol> {
        let mut symbols = Vec::new();

        for item in body.local_items() {
            if item.source.file_id == file_id
                && matches!(item.owner, rg_body_ir::BodyItemOwner::LocalScope(_))
            {
                symbols.push(self.body_item_document_symbol(file_id, item));
            }
        }

        for item in body.local_value_items() {
            if item.source.file_id == file_id
                && matches!(item.owner, rg_body_ir::BodyValueItemOwner::LocalScope(_))
            {
                symbols.push(self.body_value_item_document_symbol(file_id, item));
            }
        }

        for function in body.local_functions() {
            if function.source.file_id == file_id
                && matches!(function.owner, rg_body_ir::BodyFunctionOwner::LocalScope(_))
            {
                symbols.push(DocumentSymbol {
                    name: function.name.to_string(),
                    kind: SymbolKind::Function,
                    file_id,
                    span: function.source.span,
                    selection_span: function.name_source.span,
                    children: Vec::new(),
                });
            }
        }

        for impl_data in body.local_impls() {
            if impl_data.source.file_id == file_id {
                symbols.push(self.body_impl_document_symbol(body, impl_data));
            }
        }

        symbols
    }

    fn body_item_document_symbol(&self, file_id: FileId, item: &BodyItemData) -> DocumentSymbol {
        DocumentSymbol {
            name: item.name.to_string(),
            kind: SymbolKind::from_body_item_kind(item.kind),
            file_id,
            span: item.source.span,
            selection_span: item.name_source.span,
            children: item
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
        }
    }

    fn body_value_item_document_symbol(
        &self,
        file_id: FileId,
        item: &rg_body_ir::BodyValueItemData,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: item.name.to_string(),
            kind: SymbolKind::from_body_value_item_kind(item.kind),
            file_id,
            span: item.source.span,
            selection_span: item.name_source.span,
            children: Vec::new(),
        }
    }

    fn body_impl_document_symbol(
        &self,
        body: &BodyData,
        impl_data: &BodyImplData,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: shared::body_impl_label(impl_data),
            kind: SymbolKind::Impl,
            file_id: impl_data.source.file_id,
            span: impl_data.source.span,
            selection_span: impl_data.source.span,
            children: impl_data
                .types
                .iter()
                .filter_map(|item| {
                    let data = body.local_item(*item)?;
                    Some(self.body_item_document_symbol(data.source.file_id, data))
                })
                .chain(impl_data.consts.iter().filter_map(|item| {
                    let data = body.local_value_item(*item)?;
                    Some(self.body_value_item_document_symbol(data.source.file_id, data))
                }))
                .chain(impl_data.functions.iter().filter_map(|function| {
                    let data = body.local_function(*function)?;
                    Some(DocumentSymbol {
                        name: data.name.to_string(),
                        kind: SymbolKind::Method,
                        file_id: data.source.file_id,
                        span: data.source.span,
                        selection_span: data.name_source.span,
                        children: Vec::new(),
                    })
                }))
                .collect(),
        }
    }

    fn find_function_symbol_mut<'s>(
        symbols: &'s mut [DocumentSymbol],
        function: &FunctionData,
    ) -> Option<&'s mut DocumentSymbol> {
        // Associated functions may already be nested below traits or impls, so search the outline
        // tree instead of assuming module-level placement.
        for symbol in symbols {
            if symbol.name == function.name.as_str()
                && symbol.span == function.span
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

    fn local_def_symbol_source(
        &self,
        local_def: LocalDefRef,
        file_id: FileId,
    ) -> Result<Option<shared::SymbolSource>> {
        let Some(data) = self.0.def_map.local_def(local_def)? else {
            return Ok(None);
        };
        if data.file_id != file_id {
            return Ok(None);
        }

        Ok(Some(shared::SymbolSource {
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
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
