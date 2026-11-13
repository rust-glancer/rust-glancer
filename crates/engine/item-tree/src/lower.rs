//! File-local AST lowering. Source discovery interprets module and include paths separately.

use rg_arena::Arena;
use rg_ir_model::{FileId, PackageSlot, Span};
use rg_macro_expand::CfgSelect;
use rg_parse::syntax_edition;
use rg_syntax::{
    AstNode as _, Parse, TextRange, TextSize,
    ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility},
};
use rg_text::{Name, RustEdition};
use rg_tt::{
    Span as TtSpan, TokenOwner,
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
use crate::{DeclarationSourceStore, RecordSource, SourceRole};

/// Build declaration meaning and its source table in the same traversal.
///
/// Item nodes keep the IDs returned by their recorder. The separate table keeps file positions,
/// including positions of input tokens retained by macro definitions and calls. Lowering a
/// `cfg_select!` arm uses the same recorder with a mapper back to that arm's original file text.
pub(super) struct FileLowering<'a> {
    edition: RustEdition,
    expansion: Option<&'a FileExpansionSource>,
}

impl FileLowering<'_> {
    pub(super) fn lower(
        package: PackageSlot,
        file: FileId,
        edition: RustEdition,
        syntax: &Parse<ast::SourceFile>,
    ) -> (FileTree, DeclarationSourceStore<Span>) {
        let package = u32::try_from(package.0).expect("package slot should fit macro span storage");
        let mut builder = FileTreeBuilder::new(package, file);
        let source_file = syntax.tree();
        let docs =
            <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(&source_file, InnerDocs);
        let top_level = Self {
            edition,
            expansion: None,
        }
        .collect_items(&mut builder, source_file.items());
        // TODO: Store initializer previews with source outputs instead of declaration meaning.
        // TODO: Track source-sensitive attributes when attribute macro expansion is supported.
        (
            FileTree {
                file,
                docs,
                top_level,
                items: builder.items,
                source_sensitive: builder.source_sensitive
                    || Self::has_declaration_parse_errors(syntax),
            },
            builder.sources,
        )
    }

    /// Recovery inside a closed function body belongs to body queries. An incomplete
    /// declaration can change how neighboring syntax is parsed, so keep its text dependency.
    fn has_declaration_parse_errors(syntax: &Parse<ast::SourceFile>) -> bool {
        // Additional syntax validation does not change the recovered tree.
        let parse_errors = syntax.parse_errors();
        if parse_errors.is_empty() {
            return false;
        }

        let bodies: Vec<_> = syntax
            .tree()
            .syntax()
            .descendants()
            .filter_map(ast::Fn::cast)
            .filter_map(|function| function.body())
            .filter(|body| {
                body.stmt_list().is_some_and(|statements| {
                    statements.l_curly_token().is_some() && statements.r_curly_token().is_some()
                })
            })
            .map(|body| body.syntax().text_range())
            .collect();
        parse_errors
            .iter()
            .any(|error| !bodies.iter().any(|body| body.contains_range(error.range())))
    }

    /// Tokens entering macro expansion share the declaration's occurrence table. For a parsed
    /// `cfg_select!` arm, reuse the input origin instead of allocating an origin in temporary text.
    fn tt_span_for_range(
        &self,
        builder: &FileTreeBuilder,
        range: TextRange,
        edition: rg_tt::Edition,
        source: &mut dyn RecordSource,
    ) -> TtSpan {
        if let Some(span) = self
            .expansion
            .and_then(|source| source.tokens.span_for_range(range))
        {
            return span;
        }

        let source = source.record(range);
        SpanFactory::new(
            builder.package,
            builder.current_file_id.as_raw(),
            TokenOwner::Declaration(source.owner),
            edition,
        )
        .span_for(source.local)
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

        let mut map = |range: TextRange, _role| match self.expansion {
            Some(expansion) => expansion.span(range),
            None => Span::from_text_range(range),
        };
        let mut recorder = builder.sources.start_owner(&mut map);
        let source: &mut dyn RecordSource = &mut recorder;
        let macro_call = if let ast::Item::MacroCall(call) = &item {
            Some(call.clone())
        } else {
            None
        };
        let result = match item {
            ast::Item::AsmExpr(item) => {
                // Keep an exact-text dependency because this item omits the assembly payload.
                builder.source_sensitive = true;
                Some(builder.alloc_item(
                    ItemKind::AsmExpr,
                    None,
                    None,
                    VisibilityLevel::Private,
                    item.syntax().text_range(),
                    source,
                ))
            }
            ast::Item::Const(item) => Some(builder.alloc_documented_item(
                ItemKind::Const(ConstItem::from_ast(&item, source)),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::Enum(item) => Some(builder.alloc_documented_item(
                ItemKind::Enum(EnumItem::from_ast(&item, (None, source))),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::ExternBlock(item) => {
                let extern_block = self.lower_extern_block(builder, &item);
                Some(builder.alloc_documented_item(
                    ItemKind::ExternBlock(extern_block),
                    None,
                    None,
                    VisibilityLevel::Private,
                    &item,
                    source,
                ))
            }
            ast::Item::ExternCrate(item) => Some(
                builder.alloc_documented_item(
                    ItemKind::ExternCrate(ExternCrateItem::from_ast(&item, source)),
                    Self::name_ref_from_ast(item.name_ref()),
                    item.name_ref()
                        .map(|name_ref| name_ref.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                    source,
                ),
            ),
            ast::Item::Fn(item) => Some(builder.alloc_documented_item(
                ItemKind::Function(FunctionItem::from_ast(&item, source)),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::Impl(item) => {
                let impl_item = self.lower_impl_item(builder, &item, source);
                Some(builder.alloc_documented_item(
                    ItemKind::Impl(impl_item),
                    None,
                    None,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                    source,
                ))
            }
            ast::Item::MacroCall(item) => {
                let mut span_for_range =
                    |range| self.tt_span_for_range(builder, range, macro_edition, source);
                Some(builder.alloc_documented_item(
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
                    source,
                ))
            }
            ast::Item::MacroDef(item) => {
                let mut span_for_range =
                    |range| self.tt_span_for_range(builder, range, macro_edition, source);
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
                    source,
                ))
            }
            ast::Item::MacroRules(item) => {
                let mut span_for_range =
                    |range| self.tt_span_for_range(builder, range, macro_edition, source);
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
                    source,
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
                    source,
                ))
            }
            ast::Item::Static(item) => Some(builder.alloc_documented_item(
                ItemKind::Static(StaticItem::from_ast(&item, source)),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::Struct(item) => Some(builder.alloc_documented_item(
                ItemKind::Struct(StructItem::from_ast(&item, (None, source))),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::Trait(item) => {
                let trait_item = self.lower_trait_item(builder, &item, source);
                Some(builder.alloc_documented_item(
                    ItemKind::Trait(trait_item),
                    Self::name_from_ast(item.name()),
                    item.name().map(|name| name.syntax().text_range()),
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                    source,
                ))
            }
            ast::Item::TypeAlias(item) => Some(builder.alloc_documented_item(
                ItemKind::TypeAlias(TypeAliasItem::from_ast(&item, source)),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::Union(item) => Some(builder.alloc_documented_item(
                ItemKind::Union(UnionItem::from_ast(&item, source)),
                Self::name_from_ast(item.name()),
                item.name().map(|name| name.syntax().text_range()),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
            ast::Item::Use(item) => Some(builder.alloc_documented_item(
                ItemKind::Use(UseItem::from_ast(&item, source)),
                None,
                None,
                VisibilityLevel::from_ast(&item.visibility(), ()),
                &item,
                source,
            )),
        };
        builder.sources.finish_owner(recorder);
        // The call's input table is complete before cfg_select arms refer back to it.
        if let (Some(call), Some(id)) = (macro_call, result) {
            let builtin = self.lower_builtin_macro(builder, id, &call);
            if let ItemKind::MacroCall(call) = &mut builder.items[id].kind {
                call.builtin = builtin;
            }
        }
        result
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
                    ast::ExternItem::Fn(item) => ast::Item::Fn(item),
                    ast::ExternItem::Static(item) => ast::Item::Static(item),
                    ast::ExternItem::TypeAlias(item) => ast::Item::TypeAlias(item),
                    ast::ExternItem::MacroCall(item) => ast::Item::MacroCall(item),
                };
                if let Some(item) = self.lower_item(builder, item) {
                    items.push(item);
                }
            }
        }

        ExternBlockItem::from_ast(block, items)
    }

    /// Retains enough syntax for DefMap to interpret source-like builtins after name resolution.
    fn lower_builtin_macro(
        &self,
        builder: &mut FileTreeBuilder,
        id: ItemTreeId,
        item: &ast::MacroCall,
    ) -> Option<BuiltinMacroItem> {
        if let Some(path) = IncludePathExpression::from_macro_call(item, self.edition) {
            return Some(BuiltinMacroItem::Include { path });
        }

        if item.path()?.segment()?.name_ref()?.text() != "cfg_select" {
            return None;
        }
        let ItemKind::MacroCall(call) = &builder.items[id].kind else {
            return None;
        };
        let select = CfgSelect::parse(call.args.as_ref()?)?;
        let arms = select
            .arms()
            .iter()
            .map(|arm| {
                // A target that doesn't select this arm can still use the other arms.
                let Some((file, span_map)) = arm.parse_items() else {
                    return CfgSelectArmItem::lowering_failed(arm.predicate.clone());
                };
                let positions = span_map
                    .entries()
                    .map(|(end, token)| builder.sources.token_span(token).map(|span| (end, span)))
                    .collect::<Option<Vec<_>>>();
                let Some(positions) = positions else {
                    return CfgSelectArmItem::lowering_failed(arm.predicate.clone());
                };
                let invocation = *builder
                    .sources
                    .get(builder.items[id].location)
                    .expect("call sources were finished before its arms");
                let expansion = FileExpansionSource {
                    tokens: span_map,
                    positions,
                    invocation,
                };
                let lowering = FileLowering {
                    edition: self.edition,
                    expansion: Some(&expansion),
                };
                let items = lowering.collect_items(builder, file.items());
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

    fn lower_trait_item(
        &self,
        builder: &mut FileTreeBuilder,
        item: &ast::Trait,
        source: &mut dyn RecordSource,
    ) -> TraitItem {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self.collect_assoc_items(builder, assoc_items);

        TraitItem::from_ast(item, (items, source))
    }

    fn lower_impl_item(
        &self,
        builder: &mut FileTreeBuilder,
        item: &ast::Impl,
        source: &mut dyn RecordSource,
    ) -> ImplItem {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self.collect_assoc_items(builder, assoc_items);
        ImplItem::from_ast(item, (items, source))
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
        self.lower_item(
            builder,
            match item {
                ast::AssocItem::Const(item) => ast::Item::Const(item),
                ast::AssocItem::Fn(item) => ast::Item::Fn(item),
                ast::AssocItem::MacroCall(item) => ast::Item::MacroCall(item),
                ast::AssocItem::TypeAlias(item) => ast::Item::TypeAlias(item),
            },
        )
    }
}

/// A cfg_select arm is parsed from tokens, but every retained occurrence belongs to the
/// original file. Keep that interpretation immutable for the whole arm, including its children.
/// `tokens` preserves input-token identities for nested macro calls; `positions` maps the parsed
/// arm's byte offsets to physical file spans for declaration lowering.
struct FileExpansionSource {
    tokens: ExpansionSpanMap,
    positions: Vec<(TextSize, Span)>,
    invocation: Span,
}

impl FileExpansionSource {
    fn span(&self, range: TextRange) -> Span {
        let first = self
            .positions
            .partition_point(|(end, _)| *end <= range.start());
        let last = self
            .positions
            .partition_point(|(end, _)| *end < range.end());
        match (self.positions.get(first), self.positions.get(last)) {
            (Some((_, first)), Some((_, last))) => first.cover(*last),
            _ => self.invocation,
        }
    }
}

/// File-local item arena under construction.
struct FileTreeBuilder {
    package: u32,
    current_file_id: FileId,
    items: Arena<ItemTreeId, ItemNode>,
    sources: DeclarationSourceStore<Span>,
    source_sensitive: bool,
}

impl FileTreeBuilder {
    fn new(package: u32, current_file_id: FileId) -> Self {
        Self {
            package,
            current_file_id,
            items: Arena::new(),
            sources: DeclarationSourceStore::default(),
            source_sensitive: false,
        }
    }

    fn alloc_item(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        text_range: TextRange,
        source: &mut dyn RecordSource,
    ) -> ItemTreeId {
        self.alloc_item_with_docs(kind, name, name_range, visibility, None, text_range, source)
    }

    fn alloc_documented_item<T>(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        item: &T,
        source: &mut dyn RecordSource,
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
            source,
        );
        self.items[item_id].cfg = CfgExpr::from_attrs(item);
        self.items[item_id].lang_item = LangItem::maybe_from_ast(item, ());
        self.items[item_id].user_facing_attrs = UserFacingAttrs::from_ast(item);
        item_id
    }

    #[allow(clippy::too_many_arguments)]
    fn alloc_item_with_docs(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        text_range: TextRange,
        source: &mut dyn RecordSource,
    ) -> ItemTreeId {
        self.items.alloc(ItemNode::new(
            kind,
            name,
            name_range.map(|range| source.record(range)),
            visibility,
            docs,
            source.record_as(text_range, SourceRole::Declaration),
            self.current_file_id,
        ))
    }
}
