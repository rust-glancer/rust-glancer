//! File-local AST lowering. Source discovery interprets module and include paths separately.

use rg_arena::Arena;
use rg_ir_model::{FileId, PackageSlot, Span};
use rg_macro_expand::CfgSelect;
use rg_parse::syntax_edition;
use rg_syntax::{
    AstNode as _, TextRange,
    ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility},
};
use rg_text::{Name, RustEdition};
use rg_tt::{
    Span as TtSpan,
    syntax_bridge::{ExpansionSpanMap, SpanFactory},
};

use super::{
    BuiltinMacroItem, CfgExpr, CfgSelectArmItem, ConstItem, Documentation, EnumItem,
    ExternBlockItem, ExternCrateItem, FileTree, FromAst, FunctionItem, ImplItem,
    IncludePathExpression, InnerDocs, ItemKind, ItemNode, ItemTreeId, LangItem, MacroCallContext,
    MacroCallItem, MacroDefAst, MacroDefContext, MacroDefinitionItem, MacroRulesAst,
    MacroRulesContext, MacroUseAttr, MaybeFromAst, ModuleItem, ModuleSource, StaticItem,
    StructItem, TraitItem, TypeAliasItem, UnionItem, UseItem, UserFacingAttrs, VisibilityLevel,
};

pub(super) struct FileLowering {
    edition: RustEdition,
}

impl FileLowering {
    pub(super) fn lower(
        package: PackageSlot,
        file: FileId,
        edition: RustEdition,
        syntax: &ast::SourceFile,
    ) -> FileTree {
        let package = u32::try_from(package.0).expect("package slot should fit macro span storage");
        let mut builder = FileTreeBuilder::new(package, file);
        let docs = <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(syntax, InnerDocs);
        let top_level = Self { edition }.collect_items(&mut builder, syntax.items());
        FileTree {
            file,
            docs,
            top_level,
            items: builder.items,
        }
    }

    fn name_from_ast(name: Option<ast::Name>) -> Option<Name> {
        name.map(|name| Name::new(name.text()))
    }

    fn name_ref_from_ast(name: Option<ast::NameRef>) -> Option<Name> {
        name.map(|name| Name::new(name.text()))
    }

    /// Lowers all top-level items from one file into item-tree nodes.
    fn collect_items(
        &self,
        builder: &mut FileTreeBuilder,
        items: impl Iterator<Item = ast::Item>,
    ) -> Vec<ItemTreeId> {
        items
            .filter_map(|item| self.lower_item(builder, item))
            .collect()
    }

    /// Lowers one syntax item into the corresponding item-tree node, when this item kind matters
    /// to later phases.
    fn lower_item(&self, builder: &mut FileTreeBuilder, item: ast::Item) -> Option<ItemTreeId> {
        let edition = self.edition;
        let macro_edition = syntax_edition(edition);
        match item {
            ast::Item::AsmExpr(item) => Some(builder.alloc_item(
                ItemKind::AsmExpr,
                None,
                None,
                VisibilityLevel::Private,
                item.syntax().text_range(),
            )),
            ast::Item::Const(item) => Some(builder.alloc_documented_item(
                ItemKind::Const(ConstItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Enum(item) => Some(builder.alloc_documented_item(
                ItemKind::Enum(EnumItem::from_ast(&item, None)),
                Self::name_from_ast(item.name()),
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
                    ItemKind::ExternCrate(ExternCrateItem::from_ast(&item, ())),
                    Self::name_ref_from_ast(item.name_ref()),
                    item.name_ref()
                        .map(|name_ref| name_ref.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ),
            ),
            ast::Item::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Impl(item) => {
                let impl_item = self.lower_impl_item(builder, &item);
                Some(builder.alloc_documented_item(
                    ItemKind::Impl(impl_item),
                    None,
                    None,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::MacroCall(item) => {
                let builtin = self.lower_builtin_macro(builder, &item);
                let mut span_for_range = |range| builder.tt_span_for_range(range, macro_edition);
                Some(builder.alloc_documented_item(
                    ItemKind::MacroCall(MacroCallItem::from_ast(
                        &item,
                        MacroCallContext {
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
                    Self::name_from_ast(item.name()),
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
                    Self::name_from_ast(item.name()),
                    item.name().map(|name| name.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::Module(item) => {
                let module_name = Self::name_from_ast(item.name());
                let module_name_range = item.name().map(|name| name.syntax().text_range());
                let module_item = self.collect_module(builder, &item);
                Some(builder.alloc_documented_item(
                    ItemKind::Module(module_item),
                    module_name,
                    module_name_range,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::Static(item) => Some(builder.alloc_documented_item(
                ItemKind::Static(StaticItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Struct(item) => Some(builder.alloc_documented_item(
                ItemKind::Struct(StructItem::from_ast(&item, None)),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Trait(item) => {
                let trait_item = self.lower_trait_item(builder, &item);
                Some(builder.alloc_documented_item(
                    ItemKind::Trait(trait_item),
                    Self::name_from_ast(item.name()),
                    item.name().map(|name| name.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Union(item) => Some(builder.alloc_documented_item(
                ItemKind::Union(UnionItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::Item::Use(item) => Some(builder.alloc_documented_item(
                ItemKind::Use(UseItem::from_ast(&item, ())),
                None,
                None,
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
        }
    }

    /// Lowers direct declarations while keeping the extern block as their stable owner.
    ///
    /// Nested macro calls remain as children so source scans can still see them. DefMap does not
    /// enqueue those calls: expanding inside a foreign block needs declaration-only handling,
    /// rather than ordinary item-position macro collection.
    fn lower_extern_block(
        &self,
        builder: &mut FileTreeBuilder,
        block: &ast::ExternBlock,
    ) -> ExternBlockItem {
        let mut items = Vec::new();
        if let Some(item_list) = block.extern_item_list() {
            for item in item_list.extern_items() {
                let item = match item {
                    ast::ExternItem::Fn(item) => builder.alloc_documented_item(
                        ItemKind::Function(FunctionItem::from_ast(&item, ())),
                        Self::name_from_ast(item.name()),
                        item.name().map(|name| name.syntax().text_range()),
                        VisibilityLevel::from_ast(&item.visibility(), ()),
                        &item,
                    ),
                    ast::ExternItem::Static(item) => builder.alloc_documented_item(
                        ItemKind::Static(StaticItem::from_ast(&item, ())),
                        Self::name_from_ast(item.name()),
                        item.name().map(|name| name.syntax().text_range()),
                        VisibilityLevel::from_ast(&item.visibility(), ()),
                        &item,
                    ),
                    ast::ExternItem::TypeAlias(item) => builder.alloc_documented_item(
                        ItemKind::TypeAlias(TypeAliasItem::from_ast(&item, ())),
                        Self::name_from_ast(item.name()),
                        item.name().map(|name| name.syntax().text_range()),
                        VisibilityLevel::from_ast(&item.visibility(), ()),
                        &item,
                    ),
                    ast::ExternItem::MacroCall(item) => {
                        let macro_edition = syntax_edition(self.edition);
                        let mut span_for_range =
                            |range| builder.tt_span_for_range(range, macro_edition);
                        builder.alloc_documented_item(
                            ItemKind::MacroCall(MacroCallItem::from_ast(
                                &item,
                                MacroCallContext {
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

    /// Retains enough syntax for DefMap to interpret source-like builtins after name resolution.
    fn lower_builtin_macro(
        &self,
        builder: &mut FileTreeBuilder,
        item: &ast::MacroCall,
    ) -> Option<BuiltinMacroItem> {
        if let Some(path) = IncludePathExpression::from_macro_call(item, self.edition) {
            return Some(BuiltinMacroItem::Include { path });
        }

        let mut span_for_range =
            |range| builder.tt_span_for_range(range, syntax_edition(self.edition));
        let select = CfgSelect::from_macro_call(item, &mut span_for_range)?;
        let arms = select
            .arms()
            .iter()
            .map(|arm| {
                // A target that doesn't select this arm can still use the other arms.
                let Some((file, span_map)) = arm.parse_items() else {
                    return CfgSelectArmItem::lowering_failed(arm.predicate.clone());
                };
                let items = builder.with_span_map(span_map, |builder| {
                    self.collect_items(builder, file.items())
                });
                CfgSelectArmItem::lowered(arm.predicate.clone(), items)
            })
            .collect();
        Some(BuiltinMacroItem::CfgSelect { arms })
    }

    fn collect_module(&self, builder: &mut FileTreeBuilder, item: &ast::Module) -> ModuleItem {
        let source = match item.item_list() {
            Some(items) => ModuleSource::Inline {
                items: self.collect_items(builder, items.items()),
            },
            None => ModuleSource::OutOfLine,
        };
        ModuleItem {
            inner_docs: <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(item, InnerDocs),
            macro_use: MacroUseAttr::maybe_from_ast(item, ()),
            path_override: rg_parse::module_path_override(item),
            source,
        }
    }

    fn lower_trait_item(&self, builder: &mut FileTreeBuilder, item: &ast::Trait) -> TraitItem {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self.collect_assoc_items(builder, assoc_items);

        TraitItem::from_ast(item, items)
    }

    fn lower_impl_item(&self, builder: &mut FileTreeBuilder, item: &ast::Impl) -> ImplItem {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self.collect_assoc_items(builder, assoc_items);
        ImplItem::from_ast(item, items)
    }

    fn collect_assoc_items(
        &self,
        builder: &mut FileTreeBuilder,
        items: Vec<ast::AssocItem>,
    ) -> Vec<ItemTreeId> {
        items
            .into_iter()
            .filter_map(|item| self.lower_assoc_item(builder, item))
            .collect()
    }

    fn lower_assoc_item(
        &self,
        builder: &mut FileTreeBuilder,
        item: ast::AssocItem,
    ) -> Option<ItemTreeId> {
        match item {
            ast::AssocItem::Const(item) => Some(builder.alloc_documented_item(
                ItemKind::Const(ConstItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::AssocItem::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
            ast::AssocItem::MacroCall(item) => {
                // In `impl User { methods!(); }`, the call must remain in the impl's item list even
                // though expansion happens after module macro resolution. Retain the same payload
                // as an item-position macro so DefMap can replace it with generated functions,
                // consts, and types without losing the `impl User` owner.
                let builtin = self.lower_builtin_macro(builder, &item);
                let mut span_for_range =
                    |range| builder.tt_span_for_range(range, syntax_edition(self.edition));
                Some(builder.alloc_documented_item(
                    ItemKind::MacroCall(MacroCallItem::from_ast(
                        &item,
                        MacroCallContext {
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
            ast::AssocItem::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(&item, ())),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
            )),
        }
    }
}

/// File-local item arena under construction.
struct FileTreeBuilder {
    package: u32,
    current_file_id: FileId,
    items: Arena<ItemTreeId, ItemNode>,
    span_map: Option<ExpansionSpanMap>,
}

impl FileTreeBuilder {
    fn new(package: u32, current_file_id: FileId) -> Self {
        Self {
            package,
            current_file_id,
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

    fn source_span_for_range(&self, range: TextRange) -> Span {
        self.span_map
            .as_ref()
            .and_then(|span_map| span_map.span_for_range(range))
            .map(|span| Span::from_text_range(span.range))
            .unwrap_or_else(|| Span::from_text_range(range))
    }

    fn tt_span_for_range(&self, range: TextRange, edition: rg_tt::Edition) -> TtSpan {
        if let Some(span) = self
            .span_map
            .as_ref()
            .and_then(|span_map| span_map.span_for_range(range))
        {
            return span;
        }

        SpanFactory::new(self.package, self.current_file_id.as_raw(), edition).span_for(range)
    }

    fn alloc_item(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        text_range: TextRange,
    ) -> ItemTreeId {
        self.alloc_item_with_docs(kind, name, name_range, visibility, None, text_range)
    }

    fn alloc_documented_item<T>(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        item: &T,
    ) -> ItemTreeId
    where
        T: HasAttrs + 'static,
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
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        text_range: TextRange,
    ) -> ItemTreeId {
        self.items.alloc(ItemNode::new(
            kind,
            name,
            name_range.map(|range| self.source_span_for_range(range)),
            visibility,
            docs,
            self.source_span_for_range(text_range),
            self.current_file_id,
        ))
    }
}
