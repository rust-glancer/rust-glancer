//! Symbol enumeration over indexed declaration trees.

use std::fmt::Write as _;

use anyhow::Result;
use rg_def_map::DefMapSource;
use rg_ir_model::items::FieldKey;
use rg_ir_model::{
    AssocItemId, ConstRef, CrateRef, DefMapRef, EnumVariantRef as SemanticEnumVariantRef,
    FunctionRef as SemanticFunctionRef, ModuleId, ModuleRef, SemanticItemKind, TypeAliasRef,
    TypeDefId, TypeDefRef, identity::DeclarationRef,
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{ItemStoreQuery, SemanticItemView};

use crate::{
    IndexedViewDb,
    display::syntax::SyntaxRenderer,
    item::declaration::{Declaration, DeclarationView},
    symbol::{IndexedSymbolEntry, SourceOutlineDeclaration, SourceOutlineNode, SymbolKind},
    ty::locals::BodyView,
};

/// Syntax-only child shown in source outline.
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

/// Child of an indexed item tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexedItemChild {
    Declaration(IndexedItem),
    Syntax(IndexedSyntaxChild),
}

/// Semantic declaration with nested outline children.
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

/// Body-local item group attached to its owner declaration.
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

/// Builds declaration trees used by symbol views.
struct SymbolItemIndex<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SymbolItemIndex<'a, 'db> {
    fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return crates included in the indexed view.
    fn included_crates(&self) -> Result<Vec<CrateRef>> {
        Ok(ItemStoreQuery::new(self.db).included_crate_refs()?)
    }

    /// Return module declarations for one crate.
    fn module_declarations(&self, crate_ref: CrateRef) -> Result<Vec<DeclarationRef>> {
        Ok(self
            .db
            .module_refs(crate_ref)?
            .into_iter()
            .map(DeclarationRef::module)
            .collect())
    }

    /// Return a workspace-symbol container name for a module.
    fn module_container_name(&self, module_ref: ModuleRef) -> Result<Option<String>> {
        let Some(module) = self.db.module_data(module_ref)? else {
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

    /// Return module-owned items, optionally restricted to one file.
    fn module_owned_items(
        &self,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> Result<Vec<IndexedItem>> {
        let mut items = Vec::new();
        for item in
            ItemStoreQuery::new(self.db).semantic_items_for_origin(DefMapRef::Crate(crate_ref))?
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

    /// Return body-local item groups in one file.
    fn body_local_groups(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> Result<Vec<IndexedBodyLocalGroup>> {
        let body_view = BodyView::new(self.db);
        let mut groups = Vec::new();

        for group in body_view.local_groups(crate_ref, file_id)? {
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

    /// Convert one semantic item into an indexed item tree.
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

    /// Convert a declaration ref into an indexed item tree.
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

    /// Return a struct or union item with field children.
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

    /// Return an enum item with variant and syntax-field children.
    fn enum_item(
        &self,
        declaration: DeclarationRef,
        ty: TypeDefRef,
    ) -> Result<Option<IndexedItem>> {
        let mut children = Vec::new();
        let syntax = SyntaxRenderer::new(self.db.origin_edition(ty.origin)?);
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
                        Self::field_label(syntax, field.key.as_ref()),
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

    /// Return associated item children for a trait or impl.
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

    /// Convert an associated item id into a declaration ref.
    fn assoc_item(origin: DefMapRef, item: &AssocItemId) -> DeclarationRef {
        match item {
            AssocItemId::Function(id) => {
                DeclarationRef::from(SemanticFunctionRef { origin, id: *id })
            }
            AssocItemId::TypeAlias(id) => DeclarationRef::from(TypeAliasRef { origin, id: *id }),
            AssocItemId::Const(id) => DeclarationRef::from(ConstRef { origin, id: *id }),
        }
    }

    /// Return semantic refs for variants of an enum type.
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

    /// Return a display label for an outline-only field.
    fn field_label(syntax: SyntaxRenderer, key: Option<&FieldKey>) -> String {
        key.map(|key| syntax.field_declaration_label(key).to_string())
            .unwrap_or_else(|| "<unsupported>".to_string())
    }

    /// Return a local module path for workspace-symbol containers.
    fn module_path(&self, origin: DefMapRef, module: ModuleId) -> Result<String> {
        let syntax = SyntaxRenderer::new(self.db.origin_edition(origin)?);
        self.module_path_with_syntax(origin, module, syntax)
    }

    fn module_path_with_syntax(
        &self,
        origin: DefMapRef,
        mut module: ModuleId,
        syntax: SyntaxRenderer,
    ) -> Result<String> {
        let mut names = Vec::new();
        loop {
            let Some(data) = self.db.module_data(ModuleRef { origin, module })? else {
                return Ok(String::new());
            };
            let Some(name) = &data.name else {
                break;
            };
            names.push(name.clone());
            let Some(parent) = data.parent else {
                break;
            };
            module = parent;
        }

        let mut path = String::new();
        for name in names.iter().rev() {
            if !path.is_empty() {
                path.push_str("::");
            }
            write!(path, "{}", syntax.identifier(name)).expect("string writes should not fail");
        }
        Ok(path)
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

    /// Return source-outline symbols for one file.
    pub fn source_outline(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> Result<Vec<SourceOutlineNode>> {
        let index = SymbolItemIndex::new(self.db);
        let mut symbols = Vec::new();

        for declaration in index.module_declarations(crate_ref)? {
            if let Some(symbol) = self.declaration_source_outline_node(declaration)?
                && symbol.declaration().file_id() == file_id
            {
                symbols.push(symbol);
            }
        }

        for item in index.module_owned_items(crate_ref, Some(file_id))? {
            if let Some(symbol) = self.source_outline_item(&item, Some(file_id))? {
                symbols.push(symbol);
            }
        }

        // Body-local items belong to their owning function in a source outline. The owner may
        // already be nested under a trait or impl, so attachment searches the built tree.
        for group in index.body_local_groups(crate_ref, file_id)? {
            let Some(owner) = self.declaration(group.owner())? else {
                continue;
            };
            let owner_name = DeclarationView::new(self.db)
                .declaration_site_name(&owner)?
                .to_string();
            let Some(parent) =
                Self::find_function_symbol_mut(&mut symbols, &owner_name, owner.span())
            else {
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

    /// Return workspace-wide symbols for all included crates.
    pub fn workspace_symbols(&self) -> Result<Vec<IndexedSymbolEntry>> {
        let index = SymbolItemIndex::new(self.db);
        let mut symbols = Vec::new();

        for crate_ref in index.included_crates()? {
            for declaration in index.module_declarations(crate_ref)? {
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

            for item in index.module_owned_items(crate_ref, None)? {
                self.push_workspace_item(&item, None, &mut symbols)?;
            }
        }

        Ok(symbols)
    }

    /// Convert an indexed item tree into a source-outline node.
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

        let declaration = self.source_outline_declaration(declaration)?;
        Ok(Some(
            SourceOutlineNode::new(declaration).with_children(children),
        ))
    }

    /// Push an indexed item tree into the flat workspace-symbol list.
    fn push_workspace_item(
        &self,
        item: &IndexedItem,
        container_name: Option<String>,
        symbols: &mut Vec<IndexedSymbolEntry>,
    ) -> Result<()> {
        let Some(declaration) = self.declaration(item.declaration())? else {
            return Ok(());
        };
        let child_container_name = self.child_container_name(&declaration)?;
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

    /// Return the container name inherited by children of a declaration.
    fn child_container_name(&self, declaration: &Declaration) -> Result<Option<String>> {
        let declarations = DeclarationView::new(self.db);
        let display_name = declarations.declaration_site_name(declaration)?;
        Ok(match declaration.kind() {
            SymbolKind::Trait => Some(format!("trait {display_name}")),
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
            | SymbolKind::Variable => Some(display_name.to_string()),
        })
    }

    /// Load declaration facts for a symbol declaration ref.
    fn declaration(&self, declaration: DeclarationRef) -> Result<Option<Declaration>> {
        DeclarationView::new(self.db).declaration(declaration)
    }

    /// Build a source-outline leaf from a declaration ref.
    fn declaration_source_outline_node(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<SourceOutlineNode>> {
        let Some(declaration) = self.declaration(declaration)? else {
            return Ok(None);
        };
        Ok(Some(SourceOutlineNode::new(
            self.source_outline_declaration(declaration)?,
        )))
    }

    /// Materialize the one owned name required by the editor-facing outline result.
    fn source_outline_declaration(
        &self,
        declaration: Declaration,
    ) -> Result<SourceOutlineDeclaration> {
        let name = DeclarationView::new(self.db)
            .declaration_site_name(&declaration)?
            .to_string();
        Ok(SourceOutlineDeclaration::from_declaration(
            declaration,
            name,
        ))
    }

    /// Find a function or method node already present in the outline tree.
    fn find_function_symbol_mut<'s>(
        symbols: &'s mut [SourceOutlineNode],
        function_name: &str,
        function_span: Span,
    ) -> Option<&'s mut SourceOutlineNode> {
        // Associated functions may already be nested below traits or impls, so search the outline
        // tree instead of assuming module-level placement.
        for symbol in symbols {
            let is_owner = {
                let declaration = symbol.declaration();
                declaration.name() == function_name
                    && declaration.span() == function_span
                    && matches!(
                        declaration.kind(),
                        SymbolKind::Function | SymbolKind::Method
                    )
            };
            if is_owner {
                return Some(symbol);
            }
            if let Some(found) =
                Self::find_function_symbol_mut(symbol.children_mut(), function_name, function_span)
            {
                return Some(found);
            }
        }

        None
    }

    /// Nest source-outline nodes under containing module spans.
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

    /// Find the nearest containing module for each source-outline node.
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

    /// Build one nested source-outline subtree.
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

    /// Sort source-outline nodes by source order and stable tie-breakers.
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

    /// Return whether one span contains another without being equal.
    fn span_strictly_contains(parent: Span, child: Span) -> bool {
        parent.text.start <= child.text.start
            && child.text.end <= parent.text.end
            && parent.text != child.text
    }
}
