//! Symbol enumeration over indexed declaration trees.

use anyhow::Result;
use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, EnumVariantRef as SemanticEnumVariantRef,
    FunctionRef as SemanticFunctionRef, ModuleId, ModuleRef, SemanticItemKind, TargetRef,
    TypeAliasRef, TypeDefId, TypeDefRef, identity::DeclarationRef,
};
use rg_ir_storage::{DefMapQuery, ItemStoreQuery, SemanticItemView};
use rg_parse::{FileId, Span};

use crate::{
    IndexedViewDb,
    item::declaration::{Declaration, DeclarationView},
    symbol::{IndexedSymbolEntry, SourceOutlineDeclaration, SourceOutlineNode, SymbolKind},
    ty::locals::BodyView,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexedSyntaxChild {
    name: String,
    file_id: FileId,
    span: Span,
}

impl IndexedSyntaxChild {
    fn field(file_id: FileId, name: String, span: Span) -> Self {
        Self {
            name,
            file_id,
            span,
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_id(&self) -> FileId {
        self.file_id
    }

    fn span(&self) -> Span {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexedItemChild {
    Declaration(IndexedItem),
    Syntax(IndexedSyntaxChild),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexedItem {
    declaration: DeclarationRef,
    children: Vec<IndexedItemChild>,
}

impl IndexedItem {
    fn declaration(&self) -> DeclarationRef {
        self.declaration
    }

    fn children(&self) -> &[IndexedItemChild] {
        &self.children
    }

    fn leaf(declaration: DeclarationRef) -> Self {
        Self {
            declaration,
            children: Vec::new(),
        }
    }

    fn with_children(declaration: DeclarationRef, children: Vec<IndexedItemChild>) -> Self {
        Self {
            declaration,
            children,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexedBodyLocalGroup {
    owner: DeclarationRef,
    children: Vec<IndexedItem>,
}

impl IndexedBodyLocalGroup {
    fn owner(&self) -> DeclarationRef {
        self.owner
    }

    fn children(&self) -> &[IndexedItem] {
        &self.children
    }
}

struct SymbolItemIndex<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SymbolItemIndex<'a, 'db> {
    fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    fn included_targets(&self) -> Result<Vec<TargetRef>> {
        Ok(ItemStoreQuery::new(self.db).included_target_refs()?)
    }

    fn module_declarations(&self, target: TargetRef) -> Result<Vec<DeclarationRef>> {
        Ok(DefMapQuery::new(self.db)
            .module_refs(target)?
            .into_iter()
            .map(DeclarationRef::module)
            .collect())
    }

    fn module_container_name(&self, module_ref: ModuleRef) -> Result<Option<String>> {
        let def_maps = DefMapQuery::new(self.db);
        let Some(module) = def_maps.module_data(module_ref)? else {
            return Ok(None);
        };
        let Some(parent) = module.parent else {
            return Ok(None);
        };
        // Workspace-symbol containers are local module paths, not canonical package paths. A
        // direct child of the root module therefore has no visible container.
        let path = self.module_path(module_ref.origin, parent)?;

        Ok((!path.is_empty()).then_some(path))
    }

    fn module_owned_items(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Result<Vec<IndexedItem>> {
        let mut items = Vec::new();
        for item in
            ItemStoreQuery::new(self.db).semantic_items_for_origin(DefMapRef::Target(target))?
        {
            if item.module_owner().is_none() {
                continue;
            }
            if file_id.is_some_and(|file_id| item.source().file_id != file_id) {
                continue;
            }
            if let Some(item) = self.semantic_item(item)? {
                items.push(item);
            }
        }
        Ok(items)
    }

    fn body_local_groups(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<IndexedBodyLocalGroup>> {
        let body_view = BodyView::new(self.db);
        let mut groups = Vec::new();

        for group in body_view.local_groups(target, file_id)? {
            let mut children = Vec::new();
            for declaration in body_view.local_scope_declarations(group.body(), file_id)? {
                if let Some(item) = self.item_for_declaration(declaration)? {
                    children.push(item);
                }
            }
            if children.is_empty() {
                continue;
            }
            groups.push(IndexedBodyLocalGroup {
                owner: group.owner(),
                children,
            });
        }

        Ok(groups)
    }

    fn semantic_item(&self, item: SemanticItemView<'_>) -> Result<Option<IndexedItem>> {
        let declaration = DeclarationRef::from(item.item());
        match item.kind() {
            SemanticItemKind::Struct | SemanticItemKind::Union => {
                let Some(ty) = item.type_def() else {
                    return Ok(None);
                };
                self.type_def_item(declaration, ty)
            }
            SemanticItemKind::Enum => {
                let Some(ty) = item.type_def() else {
                    return Ok(None);
                };
                self.enum_item(declaration, ty)
            }
            SemanticItemKind::Trait | SemanticItemKind::Impl => {
                let children = item
                    .assoc_items()
                    .map(|items| self.assoc_item_children(item.item().origin(), items))
                    .transpose()?
                    .unwrap_or_default();
                Ok(Some(IndexedItem::with_children(declaration, children)))
            }
            SemanticItemKind::Function
            | SemanticItemKind::TypeAlias
            | SemanticItemKind::Const
            | SemanticItemKind::Static => Ok(Some(IndexedItem::leaf(declaration))),
        }
    }

    fn item_for_declaration(&self, declaration: DeclarationRef) -> Result<Option<IndexedItem>> {
        match declaration {
            DeclarationRef::Item(item) => {
                let Some(item) = ItemStoreQuery::new(self.db).semantic_item_view(item)? else {
                    return Ok(None);
                };
                self.semantic_item(item)
            }
            DeclarationRef::Module(_)
            | DeclarationRef::LocalDef(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => Ok(Some(IndexedItem::leaf(declaration))),
        }
    }

    fn type_def_item(
        &self,
        declaration: DeclarationRef,
        ty: TypeDefRef,
    ) -> Result<Option<IndexedItem>> {
        let mut children = Vec::new();
        for field in ItemStoreQuery::new(self.db).fields_for_type(ty)? {
            children.push(IndexedItemChild::Declaration(IndexedItem::leaf(
                DeclarationRef::from(field),
            )));
        }
        Ok(Some(IndexedItem::with_children(declaration, children)))
    }

    fn enum_item(
        &self,
        declaration: DeclarationRef,
        ty: TypeDefRef,
    ) -> Result<Option<IndexedItem>> {
        let mut children = Vec::new();
        for variant_ref in self.enum_variant_refs(ty)? {
            let Some(variant) = ItemStoreQuery::new(self.db).enum_variant_data(variant_ref)? else {
                continue;
            };
            let fields = variant
                .variant
                .fields
                .fields()
                .iter()
                .map(|field| {
                    IndexedItemChild::Syntax(IndexedSyntaxChild::field(
                        variant.file_id,
                        Self::field_label(field.key_declaration_label()),
                        field.span,
                    ))
                })
                .collect();
            children.push(IndexedItemChild::Declaration(IndexedItem::with_children(
                DeclarationRef::from(variant_ref),
                fields,
            )));
        }
        Ok(Some(IndexedItem::with_children(declaration, children)))
    }

    fn assoc_item_children(
        &self,
        origin: DefMapRef,
        items: &[AssocItemId],
    ) -> Result<Vec<IndexedItemChild>> {
        Ok(items
            .iter()
            .map(|item| {
                IndexedItemChild::Declaration(IndexedItem::leaf(Self::assoc_item(origin, item)))
            })
            .collect())
    }

    fn assoc_item(origin: DefMapRef, item: &AssocItemId) -> DeclarationRef {
        match item {
            AssocItemId::Function(id) => {
                DeclarationRef::from(SemanticFunctionRef { origin, id: *id })
            }
            AssocItemId::TypeAlias(id) => DeclarationRef::from(TypeAliasRef { origin, id: *id }),
            AssocItemId::Const(id) => DeclarationRef::from(ConstRef { origin, id: *id }),
        }
    }

    fn enum_variant_refs(&self, ty: TypeDefRef) -> Result<Vec<SemanticEnumVariantRef>> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = ItemStoreQuery::new(self.db).enum_data_for_type_def(ty)? else {
            return Ok(Vec::new());
        };

        Ok((0..data.variants.len())
            .map(|index| SemanticEnumVariantRef {
                origin: ty.origin,
                enum_id,
                index,
            })
            .collect())
    }

    fn field_label(name: Option<String>) -> String {
        name.unwrap_or_else(|| "<unsupported>".to_string())
    }

    fn module_path(&self, origin: DefMapRef, module: ModuleId) -> Result<String> {
        let def_maps = DefMapQuery::new(self.db);
        let Some(data) = def_maps.module_data(ModuleRef { origin, module })? else {
            return Ok(String::new());
        };
        let Some(name) = &data.name else {
            return Ok(String::new());
        };
        let Some(parent) = data.parent else {
            return Ok(name.to_string());
        };

        let parent_path = self.module_path(origin, parent)?;
        if parent_path.is_empty() {
            Ok(name.to_string())
        } else {
            Ok(format!("{parent_path}::{name}"))
        }
    }
}

/// Enumerates reusable symbol projections from indexed items.
pub struct SymbolView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SymbolView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn source_outline(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> Result<Vec<SourceOutlineNode>> {
        let index = SymbolItemIndex::new(self.db);
        let mut symbols = Vec::new();

        for declaration in index.module_declarations(target)? {
            if let Some(symbol) = self.declaration_source_outline_node(declaration)?
                && symbol.declaration().file_id() == file_id
            {
                symbols.push(symbol);
            }
        }

        for item in index.module_owned_items(target, Some(file_id))? {
            if let Some(symbol) = self.source_outline_item(&item, Some(file_id))? {
                symbols.push(symbol);
            }
        }

        // Body-local items belong to their owning function in a source outline. The owner may
        // already be nested under a trait or impl, so attachment searches the built tree.
        for group in index.body_local_groups(target, file_id)? {
            let Some(owner) = self.declaration(group.owner())? else {
                continue;
            };
            let Some(parent) = Self::find_function_symbol_mut(&mut symbols, &owner) else {
                continue;
            };
            for item in group.children() {
                if let Some(symbol) = self.source_outline_item(item, Some(file_id))? {
                    parent.children_mut().push(symbol);
                }
            }
        }

        let mut symbols = Self::nest_module_source_outline(symbols);
        Self::sort_source_outline(&mut symbols);
        Ok(symbols)
    }

    pub fn workspace_symbols(&self) -> Result<Vec<IndexedSymbolEntry>> {
        let index = SymbolItemIndex::new(self.db);
        let mut symbols = Vec::new();

        for target in index.included_targets()? {
            for declaration in index.module_declarations(target)? {
                let Some(module) = self.declaration(declaration)? else {
                    continue;
                };
                let container_name = match declaration {
                    DeclarationRef::Module(module_ref) => {
                        index.module_container_name(module_ref)?
                    }
                    DeclarationRef::LocalDef(_)
                    | DeclarationRef::Item(_)
                    | DeclarationRef::Field(_)
                    | DeclarationRef::EnumVariant(_)
                    | DeclarationRef::BodyBinding(_) => None,
                };
                symbols.push(IndexedSymbolEntry::new(module, container_name));
            }

            for item in index.module_owned_items(target, None)? {
                self.push_workspace_item(&item, None, &mut symbols)?;
            }
        }

        Ok(symbols)
    }

    fn source_outline_item(
        &self,
        item: &IndexedItem,
        file_id: Option<FileId>,
    ) -> Result<Option<SourceOutlineNode>> {
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
                    if let Some(symbol) = self.source_outline_item(child, file_id)? {
                        children.push(symbol);
                    }
                }
                IndexedItemChild::Syntax(child) => {
                    if file_id.is_some_and(|file_id| child.file_id() != file_id) {
                        continue;
                    }
                    children.push(SourceOutlineNode::new(SourceOutlineDeclaration::field(
                        child.file_id(),
                        child.name().to_string(),
                        child.span(),
                    )));
                }
            }
        }

        Ok(Some(
            SourceOutlineNode::new(declaration).with_children(children),
        ))
    }

    fn push_workspace_item(
        &self,
        item: &IndexedItem,
        container_name: Option<String>,
        symbols: &mut Vec<IndexedSymbolEntry>,
    ) -> Result<()> {
        let Some(declaration) = self.declaration(item.declaration())? else {
            return Ok(());
        };
        let child_container_name = Self::child_container_name(&declaration);
        if declaration.kind() != SymbolKind::Impl {
            symbols.push(IndexedSymbolEntry::new(declaration, container_name));
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

    fn declaration_source_outline_node(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<SourceOutlineNode>> {
        Ok(self.declaration(declaration)?.map(SourceOutlineNode::new))
    }

    fn find_function_symbol_mut<'s>(
        symbols: &'s mut [SourceOutlineNode],
        function: &Declaration,
    ) -> Option<&'s mut SourceOutlineNode> {
        // Associated functions may already be nested below traits or impls, so search the outline
        // tree instead of assuming module-level placement.
        for symbol in symbols {
            let is_owner = {
                let declaration = symbol.declaration();
                declaration.name() == function.name()
                    && declaration.span() == function.span()
                    && matches!(
                        declaration.kind(),
                        SymbolKind::Function | SymbolKind::Method
                    )
            };
            if is_owner {
                return Some(symbol);
            }
            if let Some(found) = Self::find_function_symbol_mut(symbol.children_mut(), function) {
                return Some(found);
            }
        }

        None
    }

    fn nest_module_source_outline(symbols: Vec<SourceOutlineNode>) -> Vec<SourceOutlineNode> {
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
            .map(|idx| Self::build_nested_source_outline(idx, &symbols, &children_by_parent))
            .collect()
    }

    fn module_parents_by_symbol(symbols: &[SourceOutlineNode]) -> Vec<Option<usize>> {
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
                            && module.declaration().kind() == SymbolKind::Module
                            && Self::span_strictly_contains(
                                module.declaration().span(),
                                symbol.declaration().span(),
                            )
                    })
                    .min_by_key(|(_, module)| module.declaration().span().len())
                    .map(|(module_idx, _)| module_idx)
            })
            .collect()
    }

    fn build_nested_source_outline(
        idx: usize,
        symbols: &[SourceOutlineNode],
        children_by_parent: &[Vec<usize>],
    ) -> SourceOutlineNode {
        let mut symbol = symbols[idx].clone();
        symbol.children_mut().extend(
            children_by_parent[idx]
                .iter()
                .map(|child_idx| {
                    Self::build_nested_source_outline(*child_idx, symbols, children_by_parent)
                })
                .collect::<Vec<_>>(),
        );
        Self::sort_source_outline(symbol.children_mut());
        symbol
    }

    fn sort_source_outline(symbols: &mut [SourceOutlineNode]) {
        for symbol in symbols.iter_mut() {
            Self::sort_source_outline(symbol.children_mut());
        }

        symbols.sort_by_key(|symbol| {
            let declaration = symbol.declaration();
            (
                declaration.span().text.start,
                declaration.span().text.end,
                declaration.kind(),
                declaration.name().to_string(),
            )
        });
    }

    fn span_strictly_contains(parent: Span, child: Span) -> bool {
        parent.text.start <= child.text.start
            && child.text.end <= parent.text.end
            && parent.text != child.text
    }
}
