//! Symbol enumeration over generic indexed declaration trees.

use anyhow::Result;
use rg_ir_model::{
    TargetRef,
    identity::{DeclarationRef, DeclarationRefRepr},
};
use rg_ir_view::{
    IndexedViewDb, SymbolKind,
    item::declaration::{Declaration, DeclarationView},
    item::index::{IndexedItem, IndexedItemChild, ItemIndexView},
};
use rg_parse::{FileId, Span};

use crate::model::{DocumentSymbol, WorkspaceSymbol};

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
            kind: declaration.kind().into(),
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
            kind: entry.declaration.kind().into(),
            file_id: entry.declaration.file_id(),
            span: Some(entry.declaration.selection_span()),
            container_name: entry.container_name,
        }
    }
}

/// Enumerates symbols from generic indexed items, leaving editor shaping in this module.
pub(crate) struct IndexedSymbols<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> IndexedSymbols<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn document_symbols(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<DocumentSymbolNode>> {
        let index = ItemIndexView::new(self.db);
        let mut symbols = Vec::new();

        for declaration in index.module_declarations(target)? {
            if let Some(symbol) = self.declaration_document_symbol(declaration)?
                && symbol.declaration.file_id == file_id
            {
                symbols.push(symbol);
            }
        }

        for item in index.module_owned_items(target, Some(file_id))? {
            if let Some(symbol) = self.document_item(&item, Some(file_id))? {
                symbols.push(symbol);
            }
        }

        // Body-local items belong to their owning function in an editor outline. The owner may
        // already be nested under a trait or impl, so attachment searches the built tree.
        for group in index.body_local_groups(target, file_id)? {
            let Some(owner) = self.declaration(group.owner())? else {
                continue;
            };
            let Some(parent) = Self::find_function_symbol_mut(&mut symbols, &owner) else {
                continue;
            };
            for item in group.children() {
                if let Some(symbol) = self.document_item(item, Some(file_id))? {
                    parent.children.push(symbol);
                }
            }
        }

        let mut symbols = Self::nest_module_document_symbols(symbols);
        Self::sort_document_symbols(&mut symbols);
        Ok(symbols)
    }

    pub(crate) fn workspace_symbols(&self) -> Result<Vec<WorkspaceSymbolEntry>> {
        let index = ItemIndexView::new(self.db);
        let mut symbols = Vec::new();

        for target in index.included_targets()? {
            for declaration in index.module_declarations(target)? {
                let Some(module) = self.declaration(declaration)? else {
                    continue;
                };
                let container_name = match declaration.repr() {
                    DeclarationRefRepr::Module(module_ref) => {
                        index.module_container_name(module_ref)?
                    }
                    DeclarationRefRepr::NameDef(_)
                    | DeclarationRefRepr::Item(_)
                    | DeclarationRefRepr::Function(_)
                    | DeclarationRefRepr::Field(_)
                    | DeclarationRefRepr::EnumVariant(_)
                    | DeclarationRefRepr::Binding(_)
                    | DeclarationRefRepr::Impl(_) => None,
                };
                symbols.push(WorkspaceSymbolEntry::new(module, container_name));
            }

            for item in index.module_owned_items(target, None)? {
                self.push_workspace_item(&item, None, &mut symbols)?;
            }
        }

        Ok(symbols)
    }

    fn document_item(
        &self,
        item: &IndexedItem,
        file_id: Option<FileId>,
    ) -> Result<Option<DocumentSymbolNode>> {
        let Some(declaration) = self.declaration(item.declaration())? else {
            return Ok(None);
        };
        if file_id.is_some_and(|file_id| declaration.file_id() != file_id) {
            return Ok(None);
        }

        let mut children = Vec::new();
        for child in item.children() {
            match child {
                IndexedItemChild::Declaration(child) => {
                    if let Some(symbol) = self.document_item(child, file_id)? {
                        children.push(symbol);
                    }
                }
                IndexedItemChild::Syntax(child) => {
                    if file_id.is_some_and(|file_id| child.file_id() != file_id) {
                        continue;
                    }
                    children.push(DocumentSymbolNode::new(DocumentSymbolDeclaration::field(
                        child.file_id(),
                        child.name().to_string(),
                        child.span(),
                    )));
                }
            }
        }

        Ok(Some(
            DocumentSymbolNode::new(declaration).with_children(children),
        ))
    }

    fn push_workspace_item(
        &self,
        item: &IndexedItem,
        container_name: Option<String>,
        symbols: &mut Vec<WorkspaceSymbolEntry>,
    ) -> Result<()> {
        let Some(declaration) = self.declaration(item.declaration())? else {
            return Ok(());
        };
        let child_container_name = Self::child_container_name(&declaration);
        if declaration.kind() != SymbolKind::Impl {
            symbols.push(WorkspaceSymbolEntry::new(declaration, container_name));
        }

        for child in item.children() {
            let IndexedItemChild::Declaration(child) = child else {
                continue;
            };
            self.push_workspace_item(child, child_container_name.clone(), symbols)?;
        }

        Ok(())
    }

    fn child_container_name(declaration: &Declaration) -> Option<String> {
        match declaration.kind() {
            SymbolKind::Trait => Some(format!("trait {}", declaration.name())),
            SymbolKind::Struct
            | SymbolKind::Union
            | SymbolKind::Enum
            | SymbolKind::Impl
            | SymbolKind::EnumVariant
            | SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Module
            | SymbolKind::Const
            | SymbolKind::Field
            | SymbolKind::Macro
            | SymbolKind::Static
            | SymbolKind::TypeAlias
            | SymbolKind::Variable => Some(declaration.name().to_string()),
        }
    }

    fn declaration(&self, declaration: DeclarationRef) -> Result<Option<Declaration>> {
        DeclarationView::new(self.db).declaration(declaration)
    }

    fn declaration_document_symbol(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<DocumentSymbolNode>> {
        Ok(self.declaration(declaration)?.map(DocumentSymbolNode::new))
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
}
