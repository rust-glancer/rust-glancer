//! AST-to-item-tree lowering for one parsed package.
//!
//! This phase is deliberately file-oriented: each source file is lowered once into a `FileTree`,
//! and targets only point at their root file. Out-of-line modules therefore reuse the same lowered
//! file tree whenever multiple targets reach them. If macro expansion discovers another file after
//! the initial pass, `extend_package` sends it through this same lowerer and preserves every file
//! tree that was already built.

use std::collections::HashSet;

use anyhow::Context as _;
use rg_arena::Arena;
use rg_macro_expand::{CfgSelect, ExpansionParseKind, ExpansionSyntax};
use rg_syntax::{
    AstNode as _, AstToken as _, SyntaxKind,
    ast::{self, HasDocComments, HasModuleItem, HasName, HasVisibility},
};

use rg_parse::{FileId, LineIndex, ModuleFileContext, Package as ParsePackage, Span as ParseSpan};
use rg_text::{Name, NameInterner};
use rg_tt::{
    Span as TtSpan,
    syntax_bridge::{ExpansionSpanMap, SpanFactory, syntax_node_to_token_tree_with_span},
};

use super::{
    BuiltinMacroItem, CfgExpr, CfgSelectArmItem, ConstItem, Documentation, EnumItem,
    ExternBlockItem, ExternCrateItem, FileTree, FromAst, FunctionItem, ImplItem, ImplItemContext,
    InnerDocs, ItemKind, ItemNode, ItemTreeId, LangItem, MacroCallContext, MacroCallItem,
    MacroDefAst, MacroDefContext, MacroDefinitionItem, MacroRulesAst, MacroRulesContext,
    MacroUseAttr, MaybeFromAst, ModuleItem, ModuleSource, Package, StaticItem, StructItem,
    TargetRoot, TraitItem, TraitItemContext, TypeAliasItem, UnionItem, UseItem, UserFacingAttrs,
    VisibilityLevel,
};

/// Lowers the files reachable from every target root and records those target entrypoints.
pub(super) fn build_package(
    parse_package: &mut ParsePackage,
    sources: &rg_source::SourceInventory,
    interner: &mut NameInterner,
) -> anyhow::Result<Package> {
    let mut file_trees = Arena::new();
    let target_roots = PackageLowering::new(parse_package, sources, interner, &mut file_trees)
        .lower_target_roots()?;

    Ok(Package {
        files: file_trees,
        target_roots,
    })
}

/// Adds one file and its ordinary out-of-line descendants to an existing package tree.
pub(super) fn extend_package(
    parse_package: &mut ParsePackage,
    sources: &rg_source::SourceInventory,
    interner: &mut NameInterner,
    item_tree_package: &mut Package,
    file_id: FileId,
    module_file_context: ModuleFileContext,
) -> anyhow::Result<PackageLoweringStats> {
    let mut lowering = PackageLowering::new(
        parse_package,
        sources,
        interner,
        &mut item_tree_package.files,
    );
    lowering.lower_file(file_id, module_file_context)?;
    Ok(lowering.stats)
}

/// Counts new and already-present file trees visited by one lowering entry point.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PackageLoweringStats {
    pub(super) lowered_files: usize,
    pub(super) reused_files: usize,
}

/// Mutable lowering context shared while walking all target roots in one package.
///
/// `file_trees` is either a fresh package arena or an existing arena being extended. In both cases,
/// an existing file tree is reused rather than replaced. Contextual visits remain distinct because
/// one physical file can be included from multiple logical modules.
struct PackageLowering<'db> {
    parse_package: &'db mut ParsePackage,
    sources: &'db rg_source::SourceInventory,
    interner: &'db mut NameInterner,
    active_stack: HashSet<(FileId, ModuleFileContext)>,
    file_trees: &'db mut Arena<FileId, Option<FileTree>>,
    stats: PackageLoweringStats,
}

impl<'db> PackageLowering<'db> {
    fn new(
        parse_package: &'db mut ParsePackage,
        sources: &'db rg_source::SourceInventory,
        interner: &'db mut NameInterner,
        file_trees: &'db mut Arena<FileId, Option<FileTree>>,
    ) -> Self {
        Self {
            parse_package,
            sources,
            interner,
            active_stack: HashSet::default(),
            file_trees,
            stats: PackageLoweringStats::default(),
        }
    }

    /// Starts from every target root file and lowers the reachable file set once.
    fn lower_target_roots(mut self) -> anyhow::Result<Arena<rg_parse::CargoTargetId, TargetRoot>> {
        let targets = self.parse_package.targets().to_vec();
        let target_roots = targets
            .iter()
            .map(|target| {
                let root_path = self
                    .parse_package
                    .file_path(target.root_file)
                    .expect("target root should have a parsed path");
                let root_context = ModuleFileContext::for_target_root(root_path);
                self.lower_file(target.root_file, root_context)
                    .with_context(|| {
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

        Ok(Arena::from_vec(target_roots))
    }

    /// Lowers one file once, while revisiting its module edges for every distinct context.
    fn lower_file(
        &mut self,
        current_file_id: FileId,
        module_file_context: ModuleFileContext,
    ) -> anyhow::Result<()> {
        let visit = (current_file_id, module_file_context.clone());
        // Recursive module/include graphs can revisit the same semantic edge before it finishes.
        if !self.active_stack.insert(visit.clone()) {
            return Ok(());
        }

        self.ensure_file_tree_slot(current_file_id);
        self.parse_package
            .ensure_file_syntax(current_file_id)
            .with_context(|| {
                format!("while attempting to load syntax for {:?}", current_file_id)
            })?;

        let (items, docs, line_index) = {
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
                <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(&syntax, InnerDocs),
                parsed_file.line_index()?.clone(),
            )
        };

        if self.file_trees[current_file_id].is_some() {
            self.stats.reused_files += 1;
            self.discover_items(current_file_id, items, &module_file_context)?;
            self.active_stack.remove(&visit);
            return Ok(());
        }

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
        self.stats.lowered_files += 1;
        self.active_stack.remove(&visit);
        Ok(())
    }

    /// Follows context-sensitive source edges when the file-local ItemTree already exists.
    fn discover_items(
        &mut self,
        current_file_id: FileId,
        items: Vec<ast::Item>,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<()> {
        for item in items {
            match item {
                ast::Item::Module(module) => {
                    if let Some(item_list) = module.item_list() {
                        let inline_context = module.name().map_or_else(
                            || module_file_context.clone(),
                            |name| {
                                let text = name.text();
                                module_file_context.descend_inline(
                                    rg_text::identifier_text(&text),
                                    rg_parse::module_path_override(&module).as_deref(),
                                )
                            },
                        );
                        self.discover_items(
                            current_file_id,
                            item_list.items().collect(),
                            &inline_context,
                        )?;
                        continue;
                    }

                    let Some(resolution) =
                        module_file_context.resolve_module_file(self.sources, &module)?
                    else {
                        continue;
                    };
                    let (path, child_context) = resolution.into_parts();
                    let file_id = self.parse_package.parse_file(self.sources, &path)?;
                    self.lower_file(file_id, child_context)?;
                }
                ast::Item::MacroCall(macro_call) => {
                    self.discover_macro_source_edges(
                        current_file_id,
                        &macro_call,
                        module_file_context,
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Replays only the source-discovery part of supported source-like builtin macros.
    fn discover_macro_source_edges(
        &mut self,
        current_file_id: FileId,
        item: &ast::MacroCall,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<()> {
        if let Some(include_path) = literal_include_path(item) {
            let current_file = self
                .parse_package
                .parsed_file(current_file_id)
                .with_context(|| {
                    format!("while attempting to fetch parsed file {current_file_id:?}")
                })?;
            if let Some(include_path) = current_file.resolve_path(&include_path)
                && let Ok(include_file_id) =
                    self.parse_package.parse_file(self.sources, &include_path)
            {
                self.lower_file(include_file_id, module_file_context.clone())?;
            }
            return Ok(());
        }

        if macro_call_terminal_name(item).as_deref() != Some("cfg_select") {
            return Ok(());
        }
        let Some(args) = item.token_tree() else {
            return Ok(());
        };
        let span_factory = SpanFactory::new(
            current_file_id.0.try_into().unwrap_or(u32::MAX),
            macro_edition(self.parse_package.edition()),
        );
        let mut span_for_range = |range| span_factory.span_for(range);
        let args = syntax_node_to_token_tree_with_span(&args, &mut span_for_range);
        let Some(cfg_select) = CfgSelect::parse(&args) else {
            return Ok(());
        };
        for arm in cfg_select.arms() {
            let expansion =
                ExpansionSyntax::from_token_tree(arm.payload.clone(), ExpansionParseKind::Items);
            let Some(file) = ast::MacroItems::cast(expansion.parse.syntax_node()) else {
                continue;
            };
            self.discover_items(current_file_id, file.items().collect(), module_file_context)?;
        }
        Ok(())
    }

    /// Grows the sparse file-tree table so `file_id` can be addressed directly by index.
    fn ensure_file_tree_slot(&mut self, file_id: FileId) {
        let required_len = file_id.0 + 1;
        if self.file_trees.len() < required_len {
            self.file_trees.resize_with(required_len, || None);
        }
    }

    fn intern_ast_name(&mut self, name: Option<ast::Name>) -> Option<Name> {
        name.map(|name| self.interner.intern(name.text()))
    }

    fn intern_ast_name_ref(&mut self, name: Option<ast::NameRef>) -> Option<Name> {
        name.map(|name| self.interner.intern(name.text()))
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
        let edition = self.parse_package.edition();
        let macro_edition = macro_edition(edition);
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
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Enum(item) => Some(builder.alloc_documented_item(
                ItemKind::Enum(EnumItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::ExternBlock(item) => {
                let extern_block = self.lower_extern_block(builder, &item);
                Some(builder.alloc_documented_item(
                    ItemKind::ExternBlock(extern_block),
                    None,
                    None,
                    VisibilityLevel::Private,
                    &item,
                ))
            }
            ast::Item::ExternCrate(item) => Some(
                builder.alloc_documented_item(
                    ItemKind::ExternCrate(ExternCrateItem::from_ast(&item, &mut *self.interner)),
                    self.intern_ast_name_ref(item.name_ref()),
                    item.name_ref()
                        .map(|name_ref| name_ref.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ),
            ),
            ast::Item::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
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
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::MacroCall(item) => {
                let builtin = self
                    .lower_builtin_macro(builder, &item, module_file_context)
                    .context("while attempting to lower builtin macro payload")?;
                let mut span_for_range = |range| builder.tt_span_for_range(range, macro_edition);
                Some(builder.alloc_documented_item(
                    ItemKind::MacroCall(MacroCallItem::from_ast(
                        &item,
                        MacroCallContext {
                            interner: &mut *self.interner,
                            builtin,
                            span_for_range: &mut span_for_range,
                        },
                    )),
                    None,
                    None,
                    VisibilityLevel::Private,
                    &item,
                ))
            }
            ast::Item::MacroDef(item) => {
                let mut span_for_range = |range| builder.tt_span_for_range(range, macro_edition);
                Some(builder.alloc_documented_item(
                    ItemKind::MacroDefinition(
                        <MacroDefinitionItem as FromAst<MacroDefAst>>::from_ast(
                            &item,
                            MacroDefContext {
                                span_for_range: &mut span_for_range,
                            },
                        ),
                    ),
                    self.intern_ast_name(item.name()),
                    item.name().map(|name| name.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::MacroRules(item) => {
                let mut span_for_range = |range| builder.tt_span_for_range(range, macro_edition);
                Some(builder.alloc_documented_item(
                    ItemKind::MacroDefinition(
                        <MacroDefinitionItem as FromAst<MacroRulesAst>>::from_ast(
                            &item,
                            MacroRulesContext {
                                span_for_range: &mut span_for_range,
                            },
                        ),
                    ),
                    self.intern_ast_name(item.name()),
                    item.name().map(|name| name.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
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
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::Static(item) => Some(builder.alloc_documented_item(
                ItemKind::Static(StaticItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Struct(item) => Some(builder.alloc_documented_item(
                ItemKind::Struct(StructItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
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
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Union(item) => Some(builder.alloc_documented_item(
                ItemKind::Union(UnionItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Use(item) => Some(builder.alloc_documented_item(
                ItemKind::Use(UseItem::from_ast(&item, &mut *self.interner)),
                None,
                None,
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
        };

        Ok(item_id)
    }

    /// Lowers direct declarations while keeping the extern block as their stable owner.
    ///
    /// Nested macro calls remain as children so source scans can still see them. DefMap does not
    /// enqueue those calls: expanding inside a foreign block needs declaration-only handling,
    /// rather than ordinary item-position macro collection.
    fn lower_extern_block(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        block: &ast::ExternBlock,
    ) -> ExternBlockItem {
        let mut items = Vec::new();
        if let Some(item_list) = block.extern_item_list() {
            for item in item_list.extern_items() {
                let item = match item {
                    ast::ExternItem::Fn(item) => builder.alloc_documented_item(
                        ItemKind::Function(FunctionItem::from_ast(
                            &item,
                            (builder.line_index, &mut *self.interner),
                        )),
                        self.intern_ast_name(item.name()),
                        item.name().map(|name| name.syntax().text_range()),
                        VisibilityLevel::from_ast(&item.visibility(), ()),
                        &item,
                    ),
                    ast::ExternItem::Static(item) => builder.alloc_documented_item(
                        ItemKind::Static(StaticItem::from_ast(
                            &item,
                            (builder.line_index, &mut *self.interner),
                        )),
                        self.intern_ast_name(item.name()),
                        item.name().map(|name| name.syntax().text_range()),
                        VisibilityLevel::from_ast(&item.visibility(), ()),
                        &item,
                    ),
                    ast::ExternItem::TypeAlias(item) => builder.alloc_documented_item(
                        ItemKind::TypeAlias(TypeAliasItem::from_ast(
                            &item,
                            (builder.line_index, &mut *self.interner),
                        )),
                        self.intern_ast_name(item.name()),
                        item.name().map(|name| name.syntax().text_range()),
                        VisibilityLevel::from_ast(&item.visibility(), ()),
                        &item,
                    ),
                    ast::ExternItem::MacroCall(item) => {
                        let macro_edition = macro_edition(self.parse_package.edition());
                        let mut span_for_range =
                            |range| builder.tt_span_for_range(range, macro_edition);
                        builder.alloc_documented_item(
                            ItemKind::MacroCall(MacroCallItem::from_ast(
                                &item,
                                MacroCallContext {
                                    interner: &mut *self.interner,
                                    builtin: None,
                                    span_for_range: &mut span_for_range,
                                },
                            )),
                            None,
                            None,
                            VisibilityLevel::Private,
                            &item,
                        )
                    }
                };
                items.push(item);
            }
        }

        ExternBlockItem::from_ast(block, items)
    }

    /// Eagerly lowers source-like builtin macros while this file context is still available.
    ///
    /// Def-map later decides whether the call really resolves to a builtin; item-tree only records
    /// enough pre-lowered payload for that decision to be applied without reparsing files.
    fn lower_builtin_macro(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: &ast::MacroCall,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<Option<BuiltinMacroItem>> {
        if let Some(include_file) = self
            .lower_literal_include_file(builder.current_file_id, item, module_file_context)
            .context("while attempting to lower literal include macro file")?
        {
            return Ok(Some(BuiltinMacroItem::Include { file: include_file }));
        }

        self.lower_cfg_select_arms(builder, item, module_file_context)
    }

    /// Eagerly parses the simple `include!("file.rs")` form so def-map can splice real item-tree
    /// nodes after macro resolution confirms that the builtin was not shadowed.
    fn lower_literal_include_file(
        &mut self,
        current_file_id: FileId,
        item: &ast::MacroCall,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<Option<FileId>> {
        let Some(include_path) = literal_include_path(item) else {
            return Ok(None);
        };

        let current_file = self
            .parse_package
            .parsed_file(current_file_id)
            .with_context(|| {
                format!("while attempting to fetch parsed file {current_file_id:?}")
            })?;
        let Some(include_path) = current_file.resolve_path(&include_path) else {
            return Ok(None);
        };

        // This probe is intentionally best-effort. `include` can be a user-defined macro, so an
        // eager filesystem miss or read error must not make item-tree lowering fail before def-map
        // has a chance to resolve the call.
        let Ok(include_file_id) = self.parse_package.parse_file(self.sources, &include_path) else {
            return Ok(None);
        };
        // Included items inherit the caller's logical module directory. The included filename is
        // only their source location and must not become a new module-resolution base.
        if self
            .lower_file(include_file_id, module_file_context.clone())
            .is_err()
        {
            return Ok(None);
        }

        let lowered = self
            .file_trees
            .get(include_file_id)
            .and_then(Option::as_ref)
            .is_some();
        Ok(lowered.then_some(include_file_id))
    }

    /// Lowers every `cfg_select!` arm as a source fragment rooted at the call site.
    ///
    /// The selected arm is target-dependent, so item-tree cannot discard inactive arms. Lowering
    /// all arms here preserves file-relative module resolution for whichever arm def-map chooses.
    fn lower_cfg_select_arms(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: &ast::MacroCall,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<Option<BuiltinMacroItem>> {
        if macro_call_terminal_name(item).as_deref() != Some("cfg_select") {
            return Ok(None);
        }

        let Some(args) = item.token_tree() else {
            return Ok(None);
        };
        let mut span_for_range =
            |range| builder.tt_span_for_range(range, macro_edition(self.parse_package.edition()));
        let args = syntax_node_to_token_tree_with_span(&args, &mut span_for_range);
        let Some(cfg_select) = CfgSelect::parse(&args) else {
            return Ok(None);
        };

        let mut arms = Vec::new();
        for arm in cfg_select.arms() {
            let expansion =
                ExpansionSyntax::from_token_tree(arm.payload.clone(), ExpansionParseKind::Items);
            let file = match ast::MacroItems::cast(expansion.parse.syntax_node()) {
                Some(file) if expansion.parse.errors().is_empty() => file,
                _ => {
                    arms.push(CfgSelectArmItem::lowering_failed(arm.predicate.clone()));
                    continue;
                }
            };
            let items = file.items().collect::<Vec<_>>();
            let Ok(lowered_items) = builder.with_span_map(expansion.span_map, |builder| {
                self.collect_items(builder, items, module_file_context)
            }) else {
                // Inactive arms may be malformed or mention files that are not valid for this
                // target. Preserve the failed arm and let def-map care only if target cfg selects
                // it.
                arms.push(CfgSelectArmItem::lowering_failed(arm.predicate.clone()));
                continue;
            };
            arms.push(CfgSelectArmItem::lowered(
                arm.predicate.clone(),
                lowered_items,
            ));
        }

        Ok(Some(BuiltinMacroItem::CfgSelect { arms }))
    }

    /// Lowers one module declaration while keeping out-of-line file selection context-free.
    fn collect_module(
        &mut self,
        builder: &mut FileTreeBuilder<'_>,
        item: &ast::Module,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<ModuleItem> {
        let path_override = rg_parse::module_path_override(item);
        if let Some(item_list) = item.item_list() {
            let inline_module_context = item.name().map_or_else(
                || module_file_context.clone(),
                |name| {
                    let text = name.text();
                    module_file_context
                        .descend_inline(rg_text::identifier_text(&text), path_override.as_deref())
                },
            );
            let inline_items = item_list.items().collect::<Vec<_>>();
            let items = self
                .collect_items(builder, inline_items, &inline_module_context)
                .context("while attempting to collect inline module items")?;
            return Ok(ModuleItem {
                inner_docs: <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(
                    item, InnerDocs,
                ),
                macro_use: MacroUseAttr::maybe_from_ast(item, &mut *self.interner),
                path_override,
                source: ModuleSource::Inline { items },
            });
        }

        let Some(resolution) = module_file_context.resolve_module_file(self.sources, item)? else {
            return Ok(ModuleItem {
                inner_docs: None,
                macro_use: MacroUseAttr::maybe_from_ast(item, &mut *self.interner),
                path_override,
                source: ModuleSource::OutOfLine,
            });
        };
        let (module_file_path, child_context) = resolution.into_parts();

        let module_file_id = self
            .parse_package
            .parse_file(self.sources, &module_file_path)
            .with_context(|| {
                format!(
                    "while attempting to parse module file {}",
                    module_file_path.display()
                )
            })?;

        // Lower the target file eagerly so later phases can treat every module source uniformly.
        self.lower_file(module_file_id, child_context)
            .with_context(|| {
                format!(
                    "while attempting to collect module items from {}",
                    module_file_path.display()
                )
            })?;

        Ok(ModuleItem {
            inner_docs: None,
            macro_use: MacroUseAttr::maybe_from_ast(item, &mut *self.interner),
            path_override,
            source: ModuleSource::OutOfLine,
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
            TraitItemContext {
                items,
                line_index: builder.line_index,
                interner: &mut *self.interner,
            },
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
            ImplItemContext {
                items,
                line_index: builder.line_index,
                interner: &mut *self.interner,
            },
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
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::AssocItem::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::AssocItem::MacroCall(_) => None,
            ast::AssocItem::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    &item,
                    (builder.line_index, &mut *self.interner),
                )),
                self.intern_ast_name(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
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
    span_map: Option<ExpansionSpanMap>,
}

impl<'a> FileTreeBuilder<'a> {
    fn new(current_file_id: FileId, line_index: &'a LineIndex) -> Self {
        Self {
            current_file_id,
            line_index,
            items: Arena::new(),
            span_map: None,
        }
    }

    fn with_span_map<R>(
        &mut self,
        span_map: ExpansionSpanMap,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = self.span_map.replace(span_map);
        let result = f(self);
        self.span_map = previous;
        result
    }

    fn parse_span_for_range(&self, range: rg_syntax::TextRange) -> ParseSpan {
        self.span_map
            .as_ref()
            .and_then(|span_map| span_map.span_for_range(range))
            .map(|span| ParseSpan::from_text_range(span.range))
            .unwrap_or_else(|| ParseSpan::from_text_range(range))
    }

    fn tt_span_for_range(&self, range: rg_syntax::TextRange, edition: rg_tt::Edition) -> TtSpan {
        if let Some(span) = self
            .span_map
            .as_ref()
            .and_then(|span_map| span_map.span_for_range(range))
        {
            return span;
        }

        SpanFactory::new(file_id_u32(self.current_file_id), edition).span_for(range)
    }

    fn alloc_item(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<rg_syntax::TextRange>,
        visibility: VisibilityLevel,
        text_range: rg_syntax::TextRange,
    ) -> ItemTreeId {
        self.alloc_item_with_docs(kind, name, name_range, visibility, None, text_range)
    }

    fn alloc_documented_item<T>(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<rg_syntax::TextRange>,
        visibility: VisibilityLevel,
        item: &T,
    ) -> ItemTreeId
    where
        T: HasDocComments + 'static,
    {
        let item_id = self.alloc_item_with_docs(
            kind,
            name,
            name_range,
            visibility,
            <Documentation as MaybeFromAst<super::OuterDocs>>::maybe_from_ast(
                item,
                super::OuterDocs,
            ),
            item.syntax().text_range(),
        );
        self.items[item_id].cfg = CfgExpr::from_attrs(item);
        self.items[item_id].lang_item = LangItem::maybe_from_ast(item, ());
        self.items[item_id].user_facing_attrs = UserFacingAttrs::from_ast(item);
        item_id
    }

    fn alloc_item_with_docs(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<rg_syntax::TextRange>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        text_range: rg_syntax::TextRange,
    ) -> ItemTreeId {
        self.items.alloc(ItemNode::new(
            kind,
            name,
            name_range.map(|range| self.parse_span_for_range(range)),
            visibility,
            docs,
            self.parse_span_for_range(text_range),
            self.current_file_id,
        ))
    }
}

fn file_id_u32(file_id: FileId) -> u32 {
    u32::try_from(file_id.0).expect("file id should fit macro span storage")
}

fn macro_edition(edition: rg_text::RustEdition) -> rg_tt::Edition {
    match edition {
        rg_text::RustEdition::Edition2015 => rg_tt::Edition::Edition2015,
        rg_text::RustEdition::Edition2018 => rg_tt::Edition::Edition2018,
        rg_text::RustEdition::Edition2021 => rg_tt::Edition::Edition2021,
        rg_text::RustEdition::Edition2024 => rg_tt::Edition::Edition2024,
    }
}

fn literal_include_path(item: &ast::MacroCall) -> Option<String> {
    if macro_call_terminal_name(item).as_deref() != Some("include") {
        return None;
    }

    let token_tree = item.token_tree()?;
    let tokens = token_tree
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| !token.kind().is_trivia())
        .collect::<Vec<_>>();
    let [open, path, close] = tokens.as_slice() else {
        return None;
    };
    if !matching_delimiters(open.kind(), close.kind()) {
        return None;
    }

    ast::String::cast(path.clone())
        .and_then(|path| path.value().ok().map(|value| value.into_owned()))
}

fn macro_call_terminal_name(item: &ast::MacroCall) -> Option<String> {
    item.path()?
        .segment()?
        .name_ref()
        .map(|name| name.text().to_string())
}

fn matching_delimiters(open: SyntaxKind, close: SyntaxKind) -> bool {
    matches!(
        (open, close),
        (SyntaxKind::L_PAREN, SyntaxKind::R_PAREN)
            | (SyntaxKind::L_CURLY, SyntaxKind::R_CURLY)
            | (SyntaxKind::L_BRACK, SyntaxKind::R_BRACK)
    )
}
