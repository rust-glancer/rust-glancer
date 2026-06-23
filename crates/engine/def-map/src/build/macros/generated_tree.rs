//! Lowers declarative macro output into an item-tree-shaped generated source.
//!
//! The generated payload intentionally lives in def-map, not item-tree: macro expansion is
//! target-local, import-dependent, and `$crate`-sensitive. Keeping the payload item-tree-shaped
//! lets later phases reuse normal semantic lowering without pretending generated items came from a
//! real parsed file.

use anyhow::{Context as _, Result};

use rg_arena::Arena;
use rg_ir_model::hir::source::GeneratedSourceData;
use rg_item_tree::{
    CfgExpr, ConstItem, Documentation, EnumItem, ExternCrateItem, FromAst, FunctionItem, ImplItem,
    ImplItemContext, InnerDocs, ItemKind, ItemNode, ItemTreeId, MacroCallContext, MacroCallItem,
    MacroDefAst, MacroDefContext, MacroDefinitionItem, MacroRulesAst, MacroRulesContext,
    MaybeFromAst, ModuleItem, ModuleSource, OuterDocs, StaticItem, StructItem, TraitItem,
    TraitItemContext, TypeAliasItem, UnionItem, UseItem, VisibilityLevel,
};
use rg_macro_runtime::{ExpansionSyntax, macro_edition};
use rg_parse::{FileId, LineIndex, Span};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasDocComments, HasModuleItem, HasName, HasVisibility},
};
use rg_text::{Name, NameInterner};
use rg_tt::{
    Span as TtSpan,
    syntax_bridge::{ExpansionSpanMap, SpanFactory},
};

use super::generated::GeneratedOrigin;

/// Lowers one parsed macro expansion into retained generated item payloads.
pub(super) struct GeneratedSourceLowering<'a> {
    origin: &'a GeneratedOrigin,
    interner: &'a mut NameInterner,
    edition: rg_workspace::RustEdition,
    span_map: ExpansionSpanMap,
    line_index: LineIndex,
    items: Arena<ItemTreeId, ItemNode>,
}

impl<'a> GeneratedSourceLowering<'a> {
    pub(super) fn lower(
        origin: &'a GeneratedOrigin,
        expansion: ExpansionSyntax,
        interner: &'a mut NameInterner,
        edition: rg_workspace::RustEdition,
    ) -> Result<GeneratedSourceData> {
        let ExpansionSyntax { parse, span_map } = expansion;
        let source_text = parse.syntax_node().text().to_string();
        let line_index = LineIndex::new(&source_text);
        let file = ast::MacroItems::cast(parse.syntax_node())
            .context("while attempting to cast macro expansion syntax root")?;
        let mut lowering = Self {
            origin,
            interner,
            edition,
            span_map,
            line_index,
            items: Arena::new(),
        };
        let top_level = lowering
            .collect_items(file.items().collect())
            .context("while attempting to lower generated source items")?;

        Ok(GeneratedSourceData {
            origin_file_id: origin.file_id,
            origin_span: origin.span,
            origin_source: origin.source,
            top_level,
            items: lowering.items,
        })
    }

    fn collect_items(&mut self, items: Vec<ast::Item>) -> Result<Vec<ItemTreeId>> {
        let mut item_ids = Vec::new();

        for item in items {
            if let Some(item_id) = self
                .lower_item(item)
                .context("while attempting to lower generated item")?
            {
                item_ids.push(item_id);
            }
        }

        Ok(item_ids)
    }

    fn lower_item(&mut self, item: ast::Item) -> Result<Option<ItemTreeId>> {
        let item_id = match item {
            ast::Item::AsmExpr(item) => Some(self.alloc_item(
                ItemKind::AsmExpr,
                None,
                None,
                VisibilityLevel::Private,
                item.syntax().text_range(),
            )),
            ast::Item::Const(item) => {
                let kind = ItemKind::Const(ConstItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Enum(item) => {
                let kind = ItemKind::Enum(EnumItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::ExternBlock(item) => Some(self.alloc_item(
                ItemKind::ExternBlock,
                None,
                None,
                VisibilityLevel::Private,
                item.syntax().text_range(),
            )),
            ast::Item::ExternCrate(item) => {
                let kind =
                    ItemKind::ExternCrate(ExternCrateItem::from_ast(&item, &mut *self.interner));
                let name = self.intern_ast_name_ref(item.name_ref());
                let name_range = item
                    .name_ref()
                    .map(|name_ref| name_ref.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Fn(item) => {
                let kind = ItemKind::Function(FunctionItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Impl(item) => {
                let impl_item = self
                    .lower_impl_item(&item)
                    .context("while attempting to lower generated impl declaration")?;
                Some(self.alloc_documented_item(
                    ItemKind::Impl(impl_item),
                    None,
                    None,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::MacroCall(item) => {
                let span_map = self.span_map.clone();
                let origin_file_id = self.origin.file_id;
                let origin_span = self.origin.span;
                let edition = self.edition;
                let mut span_for_range = move |range| {
                    tt_span_for_range(&span_map, origin_file_id, origin_span, edition, range)
                };
                let kind = ItemKind::MacroCall(MacroCallItem::from_ast(
                    &item,
                    MacroCallContext {
                        interner: &mut *self.interner,
                        builtin: None,
                        span_for_range: &mut span_for_range,
                    },
                ));
                Some(self.alloc_documented_item(kind, None, None, VisibilityLevel::Private, &item))
            }
            ast::Item::MacroDef(item) => {
                let span_map = self.span_map.clone();
                let origin_file_id = self.origin.file_id;
                let origin_span = self.origin.span;
                let edition = self.edition;
                let mut span_for_range = move |range| {
                    tt_span_for_range(&span_map, origin_file_id, origin_span, edition, range)
                };
                let kind = ItemKind::MacroDefinition(<MacroDefinitionItem as FromAst<
                    MacroDefAst,
                >>::from_ast(
                    &item,
                    MacroDefContext {
                        span_for_range: &mut span_for_range,
                    },
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::MacroRules(item) => {
                let span_map = self.span_map.clone();
                let origin_file_id = self.origin.file_id;
                let origin_span = self.origin.span;
                let edition = self.edition;
                let mut span_for_range = move |range| {
                    tt_span_for_range(&span_map, origin_file_id, origin_span, edition, range)
                };
                let kind = ItemKind::MacroDefinition(<MacroDefinitionItem as FromAst<
                    MacroRulesAst,
                >>::from_ast(
                    &item,
                    MacroRulesContext {
                        span_for_range: &mut span_for_range,
                    },
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Module(item) => {
                let module_name = self.intern_ast_name(item.name());
                let module_name_range = item.name().map(|name| name.syntax().text_range());
                let module_item = self
                    .lower_module_item(&item)
                    .context("while attempting to lower generated module declaration")?;
                Some(self.alloc_documented_item(
                    ItemKind::Module(module_item),
                    module_name,
                    module_name_range,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                ))
            }
            ast::Item::Static(item) => {
                let kind = ItemKind::Static(StaticItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Struct(item) => {
                let kind = ItemKind::Struct(StructItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Trait(item) => {
                let trait_item = self
                    .lower_trait_item(&item)
                    .context("while attempting to lower generated trait declaration")?;
                let kind = ItemKind::Trait(trait_item);
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::TypeAlias(item) => {
                let kind = ItemKind::TypeAlias(TypeAliasItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Union(item) => {
                let kind = ItemKind::Union(UnionItem::from_ast(
                    &item,
                    (&self.line_index, &mut *self.interner),
                ));
                let name = self.intern_ast_name(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
            }
            ast::Item::Use(item) => {
                let kind = ItemKind::Use(UseItem::from_ast(&item, &mut *self.interner));
                let name = normalized_use_name(&item).map(|name| self.intern_name(name));
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, None, visibility, &item))
            }
        };

        Ok(item_id)
    }

    fn lower_module_item(&mut self, item: &ast::Module) -> Result<ModuleItem> {
        let source = if let Some(item_list) = item.item_list() {
            let items = self
                .collect_items(item_list.items().collect())
                .context("while attempting to lower generated inline module items")?;
            ModuleSource::Inline { items }
        } else {
            ModuleSource::OutOfLine {
                definition_file: None,
            }
        };

        Ok(ModuleItem {
            inner_docs: <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(item, InnerDocs),
            macro_use: None,
            source,
        })
    }

    fn lower_trait_item(&mut self, item: &ast::Trait) -> Result<TraitItem> {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self
            .collect_assoc_items(assoc_items)
            .context("while attempting to lower generated trait associated items")?;

        Ok(TraitItem::from_ast(
            item,
            TraitItemContext {
                items,
                line_index: &self.line_index,
                interner: &mut *self.interner,
            },
        ))
    }

    fn lower_impl_item(&mut self, item: &ast::Impl) -> Result<ImplItem> {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self
            .collect_assoc_items(assoc_items)
            .context("while attempting to lower generated impl associated items")?;

        Ok(ImplItem::from_ast(
            item,
            ImplItemContext {
                items,
                line_index: &self.line_index,
                interner: &mut *self.interner,
            },
        ))
    }

    fn collect_assoc_items(&mut self, items: Vec<ast::AssocItem>) -> Result<Vec<ItemTreeId>> {
        let mut item_ids = Vec::new();

        for item in items {
            let item_id = match item {
                ast::AssocItem::Const(item) => {
                    let kind = ItemKind::Const(ConstItem::from_ast(
                        &item,
                        (&self.line_index, &mut *self.interner),
                    ));
                    let name = self.intern_ast_name(item.name());
                    let name_range = item.name().map(|name| name.syntax().text_range());
                    let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                    Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
                }
                ast::AssocItem::Fn(item) => {
                    let kind = ItemKind::Function(FunctionItem::from_ast(
                        &item,
                        (&self.line_index, &mut *self.interner),
                    ));
                    let name = self.intern_ast_name(item.name());
                    let name_range = item.name().map(|name| name.syntax().text_range());
                    let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                    Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
                }
                ast::AssocItem::TypeAlias(item) => {
                    let kind = ItemKind::TypeAlias(TypeAliasItem::from_ast(
                        &item,
                        (&self.line_index, &mut *self.interner),
                    ));
                    let name = self.intern_ast_name(item.name());
                    let name_range = item.name().map(|name| name.syntax().text_range());
                    let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                    Some(self.alloc_documented_item(kind, name, name_range, visibility, &item))
                }
                _ => None,
            };

            if let Some(item_id) = item_id {
                item_ids.push(item_id);
            }
        }

        Ok(item_ids)
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
            <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
            item.syntax().text_range(),
        );
        self.items[item_id].cfg = CfgExpr::from_attrs(item);
        item_id
    }

    fn alloc_item_with_docs(
        &mut self,
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<rg_syntax::TextRange>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        _text_range: rg_syntax::TextRange,
    ) -> ItemTreeId {
        let span = self.origin.span;
        let name_span = name_range
            .and_then(|range| self.parse_span_for_range(range))
            .filter(|name_span| span.contains_span(*name_span));

        // Generated item syntax is shaped by both invocation tokens and transcriber tokens. Use the
        // macro call as the stable editor-facing item range, while preserving a precise selection
        // span when the declaration name genuinely comes from the invocation.
        self.items.alloc(ItemNode {
            kind,
            name,
            name_span,
            visibility,
            cfg: CfgExpr::default(),
            docs,
            file_id: self.origin.file_id,
            span,
        })
    }

    fn parse_span_for_range(&self, range: rg_syntax::TextRange) -> Option<Span> {
        self.span_map
            .span_for_range(range)
            .filter(|span| span.anchor.file_id.raw_file_id() as usize == self.origin.file_id.0)
            .map(|span| Span::from_text_range(span.range))
    }
}

fn tt_span_for_range(
    span_map: &ExpansionSpanMap,
    origin_file_id: FileId,
    origin_span: Span,
    edition: rg_workspace::RustEdition,
    range: rg_syntax::TextRange,
) -> TtSpan {
    if let Some(span) = span_map.span_for_range(range) {
        return span;
    }

    let text_range =
        rg_syntax::TextRange::new(origin_span.text.start.into(), origin_span.text.end.into());
    SpanFactory::new(
        u32::try_from(origin_file_id.0).expect("file id should fit macro span storage"),
        macro_edition(edition),
    )
    .span_for(text_range)
}

/// Keeps the original `use ...` text in a compact, human-readable form for debugging and tests.
fn normalized_use_name(use_item: &ast::Use) -> Option<String> {
    let use_tree = use_item.use_tree()?;
    let text = use_tree.syntax().text().to_string();

    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}
