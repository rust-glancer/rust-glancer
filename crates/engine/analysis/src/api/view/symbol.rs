//! Composite symbol enumeration over declaration-producing stores.

use anyhow::Result;
use rg_body_ir::{
    BodyData, BodyFunctionId, BodyFunctionRef, BodyId, BodyImplData, BodyImplId, BodyImplRef,
    BodyItemId, BodyItemRef, BodyRef, BodyValueItemId, BodyValueItemRef,
};
use rg_def_map::{ModuleId, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    AssocItemId, ConstRef, EnumVariantRef as SemanticEnumVariantRef,
    FunctionRef as SemanticFunctionRef, SemanticItemKind, SemanticItemView, TypeAliasRef,
    TypeDefId, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        view::declaration::{Declaration, DeclarationView},
    },
    model::{DeclarationRef, DocumentSymbol, SymbolKind, WorkspaceSymbol},
};

/// One outline declaration. Most nodes come from real declarations, but some syntax-only
/// children, such as tuple variant fields, only exist as document outline entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocumentSymbolDeclaration {
    name: String,
    kind: SymbolKind,
    file_id: FileId,
    span: Span,
    selection_span: Span,
}

impl DocumentSymbolDeclaration {
    fn field(file_id: FileId, name: String, span: Span) -> Self {
        Self {
            name,
            kind: SymbolKind::Field,
            file_id,
            span,
            selection_span: span,
        }
    }
}

impl From<Declaration> for DocumentSymbolDeclaration {
    fn from(declaration: Declaration) -> Self {
        Self {
            name: declaration.name().to_string(),
            kind: declaration.kind(),
            file_id: declaration.file_id(),
            span: declaration.span(),
            selection_span: declaration.selection_span(),
        }
    }
}

/// Hierarchical source outline node for a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocumentSymbolNode {
    declaration: DocumentSymbolDeclaration,
    children: Vec<DocumentSymbolNode>,
}

impl DocumentSymbolNode {
    fn new(declaration: impl Into<DocumentSymbolDeclaration>) -> Self {
        Self {
            declaration: declaration.into(),
            children: Vec::new(),
        }
    }

    fn with_children(mut self, children: Vec<DocumentSymbolNode>) -> Self {
        self.children = children;
        self
    }
}

impl From<DocumentSymbolDeclaration> for DocumentSymbol {
    fn from(declaration: DocumentSymbolDeclaration) -> Self {
        Self {
            name: declaration.name,
            kind: declaration.kind,
            file_id: declaration.file_id,
            span: declaration.span,
            selection_span: declaration.selection_span,
            children: Vec::new(),
        }
    }
}

impl From<DocumentSymbolNode> for DocumentSymbol {
    fn from(node: DocumentSymbolNode) -> Self {
        let mut symbol = DocumentSymbol::from(node.declaration);
        symbol.children = node
            .children
            .into_iter()
            .map(DocumentSymbol::from)
            .collect();
        symbol
    }
}

/// One workspace-wide symbol entry with enough context for search and rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceSymbolEntry {
    declaration: Declaration,
    container_name: Option<String>,
}

impl WorkspaceSymbolEntry {
    fn new(declaration: Declaration, container_name: Option<String>) -> Self {
        Self {
            declaration,
            container_name,
        }
    }

    pub(crate) fn name(&self) -> &str {
        self.declaration.name()
    }
}

impl From<WorkspaceSymbolEntry> for WorkspaceSymbol {
    fn from(entry: WorkspaceSymbolEntry) -> Self {
        Self {
            target: entry.declaration.target(),
            name: entry.declaration.name().to_string(),
            kind: entry.declaration.kind(),
            file_id: entry.declaration.file_id(),
            span: Some(entry.declaration.selection_span()),
            container_name: entry.container_name,
        }
    }
}

/// Enumerates symbols while hiding whether declarations came from def-map, semantic IR, or body IR.
pub(crate) struct SymbolView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> SymbolView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbolNode>> {
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

    pub(crate) fn workspace_symbols(&self) -> Result<Vec<WorkspaceSymbolEntry>> {
        let mut symbols = Vec::new();

        for (target, _) in self
            .analysis
            .semantic_ir
            .materialize_included_target_irs()?
        {
            self.push_module_workspace_symbols(target, &mut symbols)?;
            self.push_semantic_workspace_symbols(target, &mut symbols)?;
        }

        Ok(symbols)
    }

    fn push_module_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut Vec<DocumentSymbolNode>,
    ) -> Result<()> {
        for (module_ref, _) in self.analysis.def_map.modules(target)? {
            let Some(symbol) = self
                .declaration(DeclarationRef::module(module_ref))?
                .map(DocumentSymbolNode::new)
            else {
                continue;
            };
            if symbol.declaration.file_id != file_id {
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
        symbols: &mut Vec<DocumentSymbolNode>,
    ) -> Result<()> {
        for item in self.analysis.semantic_ir.semantic_items(target)? {
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
    ) -> Result<Option<DocumentSymbolNode>> {
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
                let Some(declaration) =
                    self.declaration(DeclarationRef::semantic(item.item().into()))?
                else {
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
                    DocumentSymbolNode::new(declaration).with_children(children),
                ))
            }
            SemanticItemKind::Function
            | SemanticItemKind::TypeAlias
            | SemanticItemKind::Const
            | SemanticItemKind::Static => {
                self.declaration_document_symbol(DeclarationRef::semantic(item.item().into()))
            }
        }
    }

    fn type_def_document_symbol(
        &self,
        ty: TypeDefRef,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbolNode>> {
        let Some(declaration) = self.declaration(DeclarationRef::semantic(ty.into()))? else {
            return Ok(None);
        };
        if declaration.file_id() != file_id {
            return Ok(None);
        };

        let mut children = Vec::new();
        for field_ref in self.field_declarations_for_type(ty)? {
            let Some(symbol) = self.declaration_document_symbol(field_ref)? else {
                continue;
            };
            if symbol.declaration.file_id == file_id {
                children.push(symbol);
            }
        }

        Ok(Some(
            DocumentSymbolNode::new(declaration).with_children(children),
        ))
    }

    fn enum_document_symbol(
        &self,
        ty: TypeDefRef,
        file_id: FileId,
    ) -> Result<Option<DocumentSymbolNode>> {
        let Some(declaration) = self.declaration(DeclarationRef::semantic(ty.into()))? else {
            return Ok(None);
        };
        if declaration.file_id() != file_id {
            return Ok(None);
        };

        let mut children = Vec::new();
        for variant_ref in self.enum_variant_refs(ty)? {
            let Some(declaration) =
                self.declaration(DeclarationRef::semantic(variant_ref.into()))?
            else {
                continue;
            };
            let Some(variant) = self.analysis.semantic_ir.enum_variant_data(variant_ref)? else {
                continue;
            };
            let fields = variant
                .variant
                .fields
                .fields()
                .iter()
                .map(|field| {
                    DocumentSymbolNode::new(DocumentSymbolDeclaration::field(
                        file_id,
                        Self::field_label(field.key_declaration_label()),
                        field.span,
                    ))
                })
                .collect();
            children.push(DocumentSymbolNode::new(declaration).with_children(fields));
        }

        Ok(Some(
            DocumentSymbolNode::new(declaration).with_children(children),
        ))
    }

    fn assoc_item_document_symbols(
        &self,
        target: TargetRef,
        items: &[AssocItemId],
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbolNode>> {
        let mut symbols = Vec::new();

        for declaration in Self::assoc_item_declarations(target, items) {
            let Some(symbol) = self.declaration_document_symbol(declaration)? else {
                continue;
            };
            if symbol.declaration.file_id == file_id {
                symbols.push(symbol);
            }
        }

        Ok(symbols)
    }

    fn assoc_item_declarations(
        target: TargetRef,
        items: &[AssocItemId],
    ) -> impl Iterator<Item = DeclarationRef> + '_ {
        items
            .iter()
            .map(move |item| Self::assoc_item_declaration(target, item))
    }

    fn assoc_item_declaration(target: TargetRef, item: &AssocItemId) -> DeclarationRef {
        match item {
            AssocItemId::Function(id) => {
                DeclarationRef::semantic(SemanticFunctionRef { target, id: *id }.into())
            }
            AssocItemId::TypeAlias(id) => {
                DeclarationRef::semantic(TypeAliasRef { target, id: *id }.into())
            }
            AssocItemId::Const(id) => DeclarationRef::semantic(ConstRef { target, id: *id }.into()),
        }
    }

    fn field_declarations_for_type(&self, ty: TypeDefRef) -> Result<Vec<DeclarationRef>> {
        Ok(self
            .analysis
            .semantic_ir
            .fields_for_type(ty)?
            .into_iter()
            .map(|field| DeclarationRef::semantic(field.into()))
            .collect())
    }

    fn enum_variant_refs(&self, ty: TypeDefRef) -> Result<Vec<SemanticEnumVariantRef>> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = self.analysis.semantic_ir.enum_data_for_type_def(ty)? else {
            return Ok(Vec::new());
        };

        Ok((0..data.variants.len())
            .map(|index| SemanticEnumVariantRef {
                target: ty.target,
                enum_id,
                index,
            })
            .collect())
    }

    fn declaration_document_symbol(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<DocumentSymbolNode>> {
        Ok(self.declaration(declaration)?.map(DocumentSymbolNode::new))
    }

    fn attach_body_local_document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
        symbols: &mut [DocumentSymbolNode],
    ) -> Result<()> {
        let Some(target_bodies) = self.analysis.body_ir.target_bodies(target)? else {
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

            let Some(function) = self.declaration(DeclarationRef::semantic(body.owner().into()))?
            else {
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
    ) -> Result<Vec<DocumentSymbolNode>> {
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
                let Some(symbol) =
                    self.declaration_document_symbol(DeclarationRef::body_value_item(item_ref))?
                else {
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
                let Some(symbol) =
                    self.declaration_document_symbol(DeclarationRef::body_function(function))?
                else {
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

    fn body_item_document_symbol(
        &self,
        item_ref: BodyItemRef,
    ) -> Result<Option<DocumentSymbolNode>> {
        let Some(declaration) = self.declaration(DeclarationRef::body_item(item_ref))? else {
            return Ok(None);
        };

        let mut children = Vec::new();
        for field_ref in self.analysis.body_ir.fields_for_local_type(item_ref)? {
            let Some(symbol) =
                self.declaration_document_symbol(DeclarationRef::body_field(field_ref))?
            else {
                continue;
            };
            if symbol.declaration.file_id == declaration.file_id() {
                children.push(symbol);
            }
        }

        Ok(Some(
            DocumentSymbolNode::new(declaration).with_children(children),
        ))
    }

    fn body_impl_document_symbol(
        &self,
        impl_ref: BodyImplRef,
        impl_data: &BodyImplData,
    ) -> Result<Option<DocumentSymbolNode>> {
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
            let Some(symbol) =
                self.declaration_document_symbol(DeclarationRef::body_value_item(item_ref))?
            else {
                continue;
            };
            children.push(symbol);
        }

        for function in &impl_data.functions {
            let function = BodyFunctionRef {
                body: impl_ref.body,
                function: *function,
            };
            let Some(symbol) =
                self.declaration_document_symbol(DeclarationRef::body_function(function))?
            else {
                continue;
            };
            children.push(symbol);
        }

        let Some(declaration) = self.declaration(DeclarationRef::body(impl_ref.into()))? else {
            return Ok(None);
        };

        Ok(Some(
            DocumentSymbolNode::new(declaration).with_children(children),
        ))
    }

    fn push_semantic_workspace_symbols(
        &self,
        target: TargetRef,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        for item in self.analysis.semantic_ir.semantic_items(target)? {
            if item.module_owner().is_none() {
                continue;
            }

            self.push_semantic_workspace_symbol(item, symbols)?;
        }

        Ok(())
    }

    fn push_semantic_workspace_symbol(
        &self,
        item: SemanticItemView<'_>,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        match item.kind() {
            SemanticItemKind::Struct | SemanticItemKind::Union => {
                let Some(ty) = item.type_def() else {
                    return Ok(());
                };
                self.push_declaration_workspace_symbol(
                    DeclarationRef::semantic(item.item().into()),
                    None,
                    symbols,
                )?;
                let Some(name) = item.name() else {
                    return Ok(());
                };
                self.push_field_workspace_symbols(ty, name.as_str(), symbols)?;
            }
            SemanticItemKind::Enum => {
                let Some(ty) = item.type_def() else {
                    return Ok(());
                };
                self.push_declaration_workspace_symbol(
                    DeclarationRef::semantic(item.item().into()),
                    None,
                    symbols,
                )?;
                let Some(name) = item.name() else {
                    return Ok(());
                };
                self.push_enum_variant_workspace_symbols(ty, name.as_str(), symbols)?;
            }
            SemanticItemKind::Trait => {
                self.push_declaration_workspace_symbol(
                    DeclarationRef::semantic(item.item().into()),
                    None,
                    symbols,
                )?;
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
                    symbols,
                )?;
            }
            SemanticItemKind::Impl => {
                let Some(declaration) =
                    self.declaration(DeclarationRef::semantic(item.item().into()))?
                else {
                    return Ok(());
                };
                let Some(items) = item.assoc_items() else {
                    return Ok(());
                };
                self.push_assoc_item_workspace_symbols(
                    item.item().target(),
                    items,
                    declaration.name(),
                    symbols,
                )?;
            }
            SemanticItemKind::Function
            | SemanticItemKind::TypeAlias
            | SemanticItemKind::Const
            | SemanticItemKind::Static => {
                self.push_declaration_workspace_symbol(
                    DeclarationRef::semantic(item.item().into()),
                    None,
                    symbols,
                )?;
            }
        }

        Ok(())
    }

    fn push_module_workspace_symbols(
        &self,
        target: TargetRef,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        for (module_ref, _) in self.analysis.def_map.modules(target)? {
            let Some(declaration) = self.declaration(DeclarationRef::module(module_ref))? else {
                continue;
            };
            let container_name = self.module_container_name(module_ref)?;

            symbols.push(WorkspaceSymbolEntry::new(declaration, container_name));
        }

        Ok(())
    }

    fn push_assoc_item_workspace_symbols(
        &self,
        target: TargetRef,
        items: &[AssocItemId],
        container_name: &str,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        for declaration in Self::assoc_item_declarations(target, items) {
            self.push_declaration_workspace_symbol(
                declaration,
                Some(container_name.to_string()),
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_enum_variant_workspace_symbols(
        &self,
        ty: TypeDefRef,
        container_name: &str,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        for variant_ref in self.enum_variant_refs(ty)? {
            self.push_declaration_workspace_symbol(
                DeclarationRef::semantic(variant_ref.into()),
                Some(container_name.to_string()),
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_field_workspace_symbols(
        &self,
        ty: TypeDefRef,
        container_name: &str,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        for field_ref in self.field_declarations_for_type(ty)? {
            self.push_declaration_workspace_symbol(
                field_ref,
                Some(container_name.to_string()),
                symbols,
            )?;
        }

        Ok(())
    }

    fn push_declaration_workspace_symbol(
        &self,
        declaration: DeclarationRef,
        container_name: Option<String>,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        let Some(declaration) = self.declaration(declaration)? else {
            return Ok(());
        };

        symbols.push(WorkspaceSymbolEntry::new(declaration, container_name));
        Ok(())
    }

    fn declaration(&self, declaration: DeclarationRef) -> Result<Option<Declaration>> {
        DeclarationView::new(self.analysis).declaration(declaration)
    }

    fn module_container_name(&self, module_ref: ModuleRef) -> Result<Option<String>> {
        let Some(module) = self.analysis.def_map.module(module_ref)? else {
            return Ok(None);
        };
        let Some(parent) = module.parent else {
            return Ok(None);
        };
        let path = self.module_path(module_ref.target, parent)?;

        Ok((!path.is_empty()).then_some(path))
    }

    fn module_path(&self, target: TargetRef, module: ModuleId) -> Result<String> {
        let Some(data) = self.analysis.def_map.module(ModuleRef { target, module })? else {
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

    fn find_function_symbol_mut<'s>(
        symbols: &'s mut [DocumentSymbolNode],
        function: &Declaration,
    ) -> Option<&'s mut DocumentSymbolNode> {
        // Associated functions may already be nested below traits or impls, so search the outline
        // tree instead of assuming module-level placement.
        for symbol in symbols {
            if symbol.declaration.name == function.name()
                && symbol.declaration.span == function.span()
                && matches!(
                    symbol.declaration.kind,
                    SymbolKind::Function | SymbolKind::Method
                )
            {
                return Some(symbol);
            }
            if let Some(found) = Self::find_function_symbol_mut(&mut symbol.children, function) {
                return Some(found);
            }
        }

        None
    }

    fn nest_module_document_symbols(symbols: Vec<DocumentSymbolNode>) -> Vec<DocumentSymbolNode> {
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

    fn module_parents_by_symbol(symbols: &[DocumentSymbolNode]) -> Vec<Option<usize>> {
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
                            && module.declaration.kind == SymbolKind::Module
                            && Self::span_strictly_contains(
                                module.declaration.span,
                                symbol.declaration.span,
                            )
                    })
                    .min_by_key(|(_, module)| module.declaration.span.len())
                    .map(|(module_idx, _)| module_idx)
            })
            .collect()
    }

    fn build_nested_document_symbol(
        idx: usize,
        symbols: &[DocumentSymbolNode],
        children_by_parent: &[Vec<usize>],
    ) -> DocumentSymbolNode {
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

    fn sort_document_symbols(symbols: &mut [DocumentSymbolNode]) {
        for symbol in symbols.iter_mut() {
            Self::sort_document_symbols(&mut symbol.children);
        }

        symbols.sort_by_key(|symbol| {
            (
                symbol.declaration.span.text.start,
                symbol.declaration.span.text.end,
                symbol.declaration.kind,
                symbol.declaration.name.clone(),
            )
        });
    }

    fn span_strictly_contains(parent: Span, child: Span) -> bool {
        parent.text.start <= child.text.start
            && child.text.end <= parent.text.end
            && parent.text != child.text
    }

    fn field_label(name: Option<String>) -> String {
        name.unwrap_or_else(|| "<unsupported>".to_string())
    }
}
