//! AST-to-item-tree lowering for one parsed package.
//!
//! This phase is deliberately file-oriented: each source file is lowered once into a `FileTree`,
//! and targets only point at their root file. Out-of-line modules therefore reuse the same lowered
//! file tree whenever multiple targets reach them.

use std::collections::HashSet;

use anyhow::Context as _;
use ra_syntax::{
    AstNode as _,
    ast::{self, HasDocComments, HasModuleItem, HasName, HasVisibility},
};
use rg_arena::Arena;

use rg_parse::{FileId, LineIndex, ModuleFileContext, Package as ParsePackage};
use rg_text::{Name, NameInterner};

use super::{
    ConstItem, Documentation, EnumItem, ExternCrateItem, FileTree, FunctionItem, ImplItem,
    ItemKind, ItemNode, ItemTreeId, ModuleItem, ModuleSource, Package, StaticItem, StructItem,
    TargetRoot, TraitItem, TypeAliasItem, UnionItem, UseItem, VisibilityLevel,
};

/// Lowers all known files for one parsed package and records target entrypoints into them.
pub(super) fn build_package(
    parse_package: &mut ParsePackage,
    interner: &mut NameInterner,
) -> anyhow::Result<Package> {
    PackageLowering::new(parse_package, interner).build()
}

/// Mutable lowering context shared while walking all target roots in one package.
///
/// `file_trees` is the cache being built, and `active_stack` prevents infinite recursion while
/// following out-of-line `mod foo;` chains.
struct PackageLowering<'db> {
    parse_package: &'db mut ParsePackage,
    interner: &'db mut NameInterner,
    active_stack: HashSet<FileId>,
    file_trees: Arena<FileId, Option<FileTree>>,
}

impl<'db> PackageLowering<'db> {
    fn new(parse_package: &'db mut ParsePackage, interner: &'db mut NameInterner) -> Self {
        Self {
            parse_package,
            interner,
            active_stack: HashSet::default(),
            file_trees: Arena::new(),
        }
    }

    /// Starts from every target root file and lowers the reachable file set once.
    fn build(mut self) -> anyhow::Result<Package> {
        let targets = self.parse_package.targets().to_vec();
        let target_roots = targets
            .iter()
            .map(|target| {
                self.lower_file(target.root_file).with_context(|| {
                    format!(
                        "while attempting to lower root file for target {}",
                        target.name
                    )
                })?;
                Ok(TargetRoot {
                    target: target.id,
                    root_file: target.root_file,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Package {
            files: self.file_trees,
            target_roots: Arena::from_vec(target_roots),
        })
    }

    /// Lowers one file into a cached `FileTree` unless it was already lowered earlier.
    fn lower_file(&mut self, current_file_id: FileId) -> anyhow::Result<()> {
        self.ensure_file_tree_slot(current_file_id);
        if self.file_trees[current_file_id].is_some() {
            return Ok(());
        }

        // Recursive module graphs can revisit a file before the first traversal finishes.
        if !self.active_stack.insert(current_file_id) {
            return Ok(());
        }

        self.parse_package
            .ensure_file_syntax(current_file_id)
            .with_context(|| {
                format!("while attempting to load syntax for {:?}", current_file_id)
            })?;

        let (items, docs, line_index, module_file_context) = {
            let parsed_file = self
                .parse_package
                .parsed_file(current_file_id)
                .with_context(|| {
                    format!(
                        "while attempting to fetch parsed file {:?}",
                        current_file_id
                    )
                })?;
            let syntax = parsed_file.syntax().with_context(|| {
                format!(
                    "while attempting to access retained syntax for {:?}",
                    current_file_id
                )
            })?;
            (
                syntax.items().collect::<Vec<_>>(),
                Documentation::inner_from_ast(&syntax),
                parsed_file.line_index()?.clone(),
                ModuleFileContext::from_definition_file(parsed_file.path()),
            )
        };

        let mut builder = FileTreeBuilder::new(current_file_id, &line_index);
        let top_level = self
            .collect_items(&mut builder, items, &module_file_context)
            .with_context(|| {
                format!(
                    "while attempting to collect file items for {:?}",
                    current_file_id
                )
            })?;

        self.file_trees[current_file_id] = Some(FileTree {
            file: current_file_id,
            docs,
            top_level,
            items: builder.items,
        });
        self.active_stack.remove(&current_file_id);
        Ok(())
    }

    /// Grows the sparse file-tree table so `file_id` can be addressed directly by index.
    fn ensure_file_tree_slot(&mut self, file_id: FileId) {
        let required_len = file_id.0 + 1;
        if self.file_trees.len() < required_len {
            self.file_trees.resize_with(required_len, || None);
        }
    }

    fn intern_name(&mut self, text: impl AsRef<str>) -> Name {
        self.interner.intern(text)
    }

    fn intern_ast_name(&mut self, name: Option<ast::Name>) -> Option<Name> {
        name.map(|name| self.intern_name(name.text()))
    }

    fn intern_ast_name_ref(&mut self, name: Option<ast::NameRef>) -> Option<Name> {
        name.map(|name| self.intern_name(name.syntax().text().to_string()))
    }

    /// Lowers all top-level items from one file into item-tree nodes.
    fn collect_items(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        items: Vec<ast::Item>,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<Vec<ItemTreeId>> {
        let mut item_ids = Vec::new();

        for item in items {
            let item_id = self
                .lower_item(builder, item, module_file_context)
                .with_context(|| {
                    format!(
                        "while attempting to lower item in {:?}",
                        builder.current_file_id
                    )
                })?;

            if let Some(item_id) = item_id {
                item_ids.push(item_id);
            }
        }

        Ok(item_ids)
    }

    /// Lowers one syntax item into the corresponding item-tree node, when this item kind matters
    /// to later phases.
    fn lower_item(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: ast::Item,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<Option<ItemTreeId>> {
        let item_id = match item {
            ast::Item::AsmExpr(item) => Some(builder.alloc_item(
                ItemKind::AsmExpr,
                None,
                None,
                VisibilityLevel::Private,
                item.syntax().text_range(),
            )),
            ast::Item::Const(item) => Some(builder.alloc_documented_item(
                ItemKind::Const(ConstItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Enum(item) => Some(builder.alloc_documented_item(
                ItemKind::Enum(EnumItem::from_ast(&item, builder.line_index, self.interner)),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::ExternBlock(item) => Some(builder.alloc_item(
                ItemKind::ExternBlock,
                None,
                None,
                VisibilityLevel::Private,
                item.syntax().text_range(),
            )),
            ast::Item::ExternCrate(item) => Some(
                builder.alloc_documented_item(
                    ItemKind::ExternCrate(ExternCrateItem::from_ast(&item, self.interner)),
                    self.intern_ast_name_ref(item.name_ref()),
                    item.name_ref()
                        .map(|name_ref| name_ref.syntax().text_range()),
                    VisibilityLevel::from_ast(item.visibility()),
                    &item,
                ),
            ),
            ast::Item::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Impl(item) => {
                let impl_item = self
                    .lower_impl_item(builder, &item)
                    .context("while attempting to lower impl declaration")?;
                Some(builder.alloc_documented_item(
                    ItemKind::Impl(impl_item),
                    None,
                    None,
                    VisibilityLevel::from_ast(item.visibility()),
                    &item,
                ))
            }
            ast::Item::MacroCall(_) => None,
            ast::Item::MacroDef(item) => Some(builder.alloc_documented_item(
                ItemKind::MacroDefinition,
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::MacroRules(item) => Some(builder.alloc_documented_item(
                ItemKind::MacroDefinition,
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Module(item) => {
                let module_name = self.intern_ast_name(item.name());
                let module_name_range = item.name().map(|name| name.syntax().text_range());
                let module_item = self
                    .collect_module(builder, &item, module_file_context)
                    .with_context(|| {
                        format!(
                            "while attempting to collect module item for {}",
                            module_name
                                .as_ref()
                                .map(Name::as_str)
                                .unwrap_or("<unnamed>")
                        )
                    })?;
                Some(builder.alloc_documented_item(
                    ItemKind::Module(module_item),
                    module_name,
                    module_name_range,
                    VisibilityLevel::from_ast(item.visibility()),
                    &item,
                ))
            }
            ast::Item::Static(item) => Some(builder.alloc_documented_item(
                ItemKind::Static(StaticItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Struct(item) => Some(builder.alloc_documented_item(
                ItemKind::Struct(StructItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Trait(item) => {
                let trait_item = self
                    .lower_trait_item(builder, &item)
                    .context("while attempting to lower trait declaration")?;
                Some(builder.alloc_documented_item(
                    ItemKind::Trait(trait_item),
                    self.intern_ast_name(item.name()),
                    item.name().map(|name| name.syntax().text_range()),
                    VisibilityLevel::from_ast(item.visibility()),
                    &item,
                ))
            }
            ast::Item::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Union(item) => Some(builder.alloc_documented_item(
                ItemKind::Union(UnionItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::Item::Use(item) => Some(builder.alloc_documented_item(
                ItemKind::Use(UseItem::from_ast(&item, self.interner)),
                normalized_use_name(&item).map(|name| self.intern_name(name)),
                None,
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
        };

        Ok(item_id)
    }

    /// Lowers one module declaration into either an inline item list or an out-of-line file link.
    fn collect_module(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: &ast::Module,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<ModuleItem> {
        if let Some(item_list) = item.item_list() {
            // Inline modules reuse the current file, but their out-of-line descendants are
            // resolved under a directory named after the inline module path.
            let inline_module_context = item
                .name()
                .map(|name| module_file_context.descend(name.text().as_str()))
                .unwrap_or_else(|| module_file_context.clone());
            let inline_items = item_list.items().collect::<Vec<_>>();
            let items = self
                .collect_items(builder, inline_items, &inline_module_context)
                .context("while attempting to collect inline module items")?;
            return Ok(ModuleItem {
                inner_docs: Documentation::inner_from_ast(item),
                source: ModuleSource::Inline { items },
            });
        }

        let Some(module_file_path) = module_file_context.resolve_module_file(item) else {
            return Ok(ModuleItem {
                inner_docs: None,
                source: ModuleSource::OutOfLine {
                    definition_file: None,
                },
            });
        };

        let module_file_id = self
            .parse_package
            .parse_file(&module_file_path)
            .with_context(|| {
                format!(
                    "while attempting to parse module file {}",
                    module_file_path.display()
                )
            })?;

        // Lower the target file eagerly so later phases can treat every module source uniformly.
        self.lower_file(module_file_id).with_context(|| {
            format!(
                "while attempting to collect module items from {}",
                module_file_path.display()
            )
        })?;

        Ok(ModuleItem {
            inner_docs: None,
            source: ModuleSource::OutOfLine {
                definition_file: Some(module_file_id),
            },
        })
    }

    fn lower_trait_item(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: &ast::Trait,
    ) -> anyhow::Result<TraitItem> {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self
            .collect_assoc_items(builder, assoc_items)
            .context("while attempting to lower trait associated items")?;

        Ok(TraitItem::from_ast(
            item,
            items,
            builder.line_index,
            self.interner,
        ))
    }

    fn lower_impl_item(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: &ast::Impl,
    ) -> anyhow::Result<ImplItem> {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self
            .collect_assoc_items(builder, assoc_items)
            .context("while attempting to lower impl associated items")?;
        Ok(ImplItem::from_ast(
            item,
            items,
            builder.line_index,
            self.interner,
        ))
    }

    fn collect_assoc_items(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        items: Vec<ast::AssocItem>,
    ) -> anyhow::Result<Vec<ItemTreeId>> {
        let mut item_ids = Vec::new();

        for item in items {
            if let Some(item_id) = self.lower_assoc_item(builder, item)? {
                item_ids.push(item_id);
            }
        }

        Ok(item_ids)
    }

    fn lower_assoc_item(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: ast::AssocItem,
    ) -> anyhow::Result<Option<ItemTreeId>> {
        let item_id = match item {
            ast::AssocItem::Const(item) => Some(builder.alloc_documented_item(
                ItemKind::Const(ConstItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::AssocItem::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
            ast::AssocItem::MacroCall(_) => None,
            ast::AssocItem::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    &item,
                    builder.line_index,
                    self.interner,
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(item.visibility()),
                &item,
            )),
        };

        Ok(item_id)
    }
}

/// File-local item arena under construction.
struct FileTreeBuilder<'a> {
    current_file_id: FileId,
    line_index: &'a LineIndex,
    items: Arena<ItemTreeId, ItemNode>,
}

impl<'a> FileTreeBuilder<'a> {
    fn new(current_file_id: FileId, line_index: &'a LineIndex) -> Self {
        Self {
            current_file_id,
            line_index,
            items: Arena::new(),
        }
    }

    fn alloc_item(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<ra_syntax::TextRange>,
        visibility: VisibilityLevel,
        text_range: ra_syntax::TextRange,
    ) -> ItemTreeId {
        self.alloc_item_with_docs(kind, name, name_range, visibility, None, text_range)
    }

    fn alloc_documented_item<T>(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<ra_syntax::TextRange>,
        visibility: VisibilityLevel,
        item: &T,
    ) -> ItemTreeId
    where
        T: HasDocComments,
    {
        self.alloc_item_with_docs(
            kind,
            name,
            name_range,
            visibility,
            Documentation::from_ast(item),
            item.syntax().text_range(),
        )
    }

    fn alloc_item_with_docs(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<ra_syntax::TextRange>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        text_range: ra_syntax::TextRange,
    ) -> ItemTreeId {
        self.items.alloc(ItemNode::new(
            kind,
            name,
            name_range,
            visibility,
            docs,
            text_range,
            self.current_file_id,
        ))
    }
}

/// Keeps the original `use ...` text in a compact, human-readable form for debugging and tests.
fn normalized_use_name(use_item: &ast::Use) -> Option<String> {
    let use_tree = use_item.use_tree()?;
    let text = use_tree.syntax().text().to_string();

    // Normalize all whitespace in an extracted syntax fragment to single spaces.
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}
