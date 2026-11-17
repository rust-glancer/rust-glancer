//! Lowers declarative macro output into an item-tree-shaped generated source.
//!
//! The generated payload intentionally lives in def-map, not item-tree: macro expansion is
//! crate-local, import-dependent, and `$crate`-sensitive. Keeping the payload item-tree-shaped
//! lets later phases reuse normal semantic lowering without pretending generated items came from a
//! real parsed file.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use rg_arena::Arena;
use rg_item_tree::{
    CfgExpr, ConstItem, DeclarationSourceStore, Documentation, EnumItem, ExternBlockItem,
    ExternCrateItem, FromAst, FunctionItem, ImplItem, InnerDocs, ItemKind, ItemNode, ItemTreeId,
    LangItem, MacroCallContext, MacroCallItem, MacroDefAst, MacroDefContext, MacroDefinitionItem,
    MacroRulesAst, MacroRulesContext, MacroUseAttr, MaybeFromAst, ModuleItem, ModuleSource,
    OuterDocs, RecordSource, SourceRole, StaticItem, StructItem, TraitItem, TypeAliasItem,
    UnionItem, UseItem, UserFacingAttrs, VisibilityLevel,
};
use rg_macro_runtime::ExpansionSyntax;
use rg_syntax::{
    AstNode as _, TextRange,
    ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility},
};
use rg_text::Name;
use rg_tt::{Span as TtSpan, syntax_bridge::ExpansionSpanMap};

use super::generated::GeneratedOrigin;
use crate::source::{
    GeneratedExpansionLocations, GeneratedLocation, GeneratedLocations, GeneratedSourceData,
};

/// Lowers one parsed macro expansion into retained generated item payloads.
///
/// The parsed tree has its own byte offsets. A recorder assigns fresh IDs to the declarations
/// lowered here and stores how each occurrence relates to input tokens. Semantic lowering can
/// then copy the declaration meaning without retaining either the parsed tree or its offsets.
pub(super) struct GeneratedSourceLowering<'a> {
    origin: &'a GeneratedOrigin,
    fallback_span: TtSpan,
    span_map: Arc<ExpansionSpanMap>,
    locations: DeclarationSourceStore<GeneratedLocation>,
    items: Arena<ItemTreeId, ItemNode>,
}

impl<'a> GeneratedSourceLowering<'a> {
    pub(super) fn lower(
        origin: &'a GeneratedOrigin,
        expansion: ExpansionSyntax,
    ) -> Result<GeneratedSourceData> {
        let ExpansionSyntax { parse, span_map } = expansion;
        let file = ast::MacroItems::cast(parse.syntax_node())
            .context("while attempting to cast macro expansion syntax root")?;
        // Unmapped tokens belong to the invocation as a whole. Prepare this once so nested
        // calls and definitions can borrow the token map without rebuilding their fallback.
        let fallback_span = origin.call_site;
        let mut lowering = Self {
            origin,
            fallback_span,
            span_map: Arc::new(span_map),
            locations: DeclarationSourceStore::default(),
            items: Arena::new(),
        };
        let top_level = lowering
            .collect_items(file.items().collect())
            .context("while attempting to lower generated source items")?;

        Ok(GeneratedSourceData {
            origin_file_id: origin.file_id,
            locations: GeneratedLocations::Expansion(GeneratedExpansionLocations::new(
                lowering.locations,
            )),
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
        let span_map = Arc::clone(&self.span_map);
        let fallback = self.fallback_span;
        // Only syntax copied from this invocation gets its own selectable occurrence. Template
        // names, mixed-origin ranges, and whole declaration containers navigate to the call.
        // Store this decision with the origin so later readers cannot mistake the fallback for
        // an editable declaration name.
        let mut map = |range, role| match (role, span_map.span_for_range(range)) {
            (SourceRole::Syntax, Some(origin))
                if origin.anchor == fallback.anchor && origin.origins.last().is_some() =>
            {
                GeneratedLocation {
                    origin,
                    editable: true,
                }
            }
            _ => GeneratedLocation {
                origin: fallback,
                editable: false,
            },
        };
        let mut recorder = self.locations.start_owner(&mut map);
        let source: &mut dyn RecordSource = &mut recorder;
        let item_id = match item {
            ast::Item::AsmExpr(item) => Some(self.alloc_item(
                ItemKind::AsmExpr,
                None,
                None,
                VisibilityLevel::Private,
                item.syntax().text_range(),
                source,
            )),
            ast::Item::Const(item) => {
                let kind = ItemKind::Const(ConstItem::from_ast(&item, source));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Enum(item) => {
                let kind = ItemKind::Enum(EnumItem::from_ast(
                    &item,
                    (self.origin.dollar_crate, source),
                ));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::ExternBlock(item) => {
                let extern_block = self.lower_extern_block(&item)?;
                Some(self.alloc_documented_item(
                    ItemKind::ExternBlock(extern_block),
                    None,
                    None,
                    VisibilityLevel::Private,
                    &item,
                    source,
                ))
            }
            ast::Item::ExternCrate(item) => {
                let kind = ItemKind::ExternCrate(ExternCrateItem::from_ast(&item, source));
                let name = Self::name_ref_from_ast(item.name_ref());
                let name_range = item
                    .name_ref()
                    .map(|name_ref| name_ref.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Fn(item) => {
                let kind = ItemKind::Function(FunctionItem::from_ast(&item, source));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Impl(item) => {
                let impl_item = self
                    .lower_impl_item(&item, source)
                    .context("while attempting to lower generated impl declaration")?;
                Some(self.alloc_documented_item(
                    ItemKind::Impl(impl_item),
                    None,
                    None,
                    VisibilityLevel::from_ast(&item.visibility(), ()),
                    &item,
                    source,
                ))
            }
            ast::Item::MacroCall(item) => {
                let mut span_for_range = Self::span_mapper(&self.span_map, self.fallback_span);
                let kind = ItemKind::MacroCall(MacroCallItem::from_ast(
                    &item,
                    MacroCallContext {
                        builtin: None,
                        span_for_range: &mut span_for_range,
                    },
                ));
                Some(self.alloc_documented_item(
                    kind,
                    None,
                    None,
                    VisibilityLevel::Private,
                    &item,
                    source,
                ))
            }
            ast::Item::MacroDef(item) => {
                let mut span_for_range = Self::span_mapper(&self.span_map, self.fallback_span);
                let kind = ItemKind::MacroDefinition(<MacroDefinitionItem as FromAst<
                    MacroDefAst,
                >>::from_ast(
                    &item,
                    MacroDefContext {
                        span_for_range: &mut span_for_range,
                    },
                ));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::MacroRules(item) => {
                let mut span_for_range = Self::span_mapper(&self.span_map, self.fallback_span);
                let kind = ItemKind::MacroDefinition(<MacroDefinitionItem as FromAst<
                    MacroRulesAst,
                >>::from_ast(
                    &item,
                    MacroRulesContext {
                        span_for_range: &mut span_for_range,
                    },
                ));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Module(item) => {
                let module_name = Self::name_from_ast(item.name());
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
                    source,
                ))
            }
            ast::Item::Static(item) => {
                let kind = ItemKind::Static(StaticItem::from_ast(&item, source));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Struct(item) => {
                let kind = ItemKind::Struct(StructItem::from_ast(
                    &item,
                    (self.origin.dollar_crate, source),
                ));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Trait(item) => {
                let trait_item = self
                    .lower_trait_item(&item, source)
                    .context("while attempting to lower generated trait declaration")?;
                let kind = ItemKind::Trait(trait_item);
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::TypeAlias(item) => {
                let kind = ItemKind::TypeAlias(TypeAliasItem::from_ast(&item, source));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Union(item) => {
                let kind = ItemKind::Union(UnionItem::from_ast(&item, source));
                let name = Self::name_from_ast(item.name());
                let name_range = item.name().map(|name| name.syntax().text_range());
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, name, name_range, visibility, &item, source))
            }
            ast::Item::Use(item) => {
                let kind = ItemKind::Use(UseItem::from_ast(&item, source));
                let visibility = VisibilityLevel::from_ast(&item.visibility(), ());
                Some(self.alloc_documented_item(kind, None, None, visibility, &item, source))
            }
        };

        self.locations.finish_owner(recorder);
        Ok(item_id)
    }

    /// Mirrors source ItemTree lowering for declarations owned by a generated extern block.
    fn lower_extern_block(&mut self, block: &ast::ExternBlock) -> Result<ExternBlockItem> {
        let items = block
            .extern_item_list()
            .into_iter()
            .flat_map(|list| list.extern_items())
            .map(|item| match item {
                ast::ExternItem::Fn(item) => ast::Item::Fn(item),
                ast::ExternItem::Static(item) => ast::Item::Static(item),
                ast::ExternItem::TypeAlias(item) => ast::Item::TypeAlias(item),
                ast::ExternItem::MacroCall(item) => ast::Item::MacroCall(item),
            })
            .collect();
        Ok(ExternBlockItem::from_ast(block, self.collect_items(items)?))
    }

    fn lower_module_item(&mut self, item: &ast::Module) -> Result<ModuleItem> {
        let source = if let Some(item_list) = item.item_list() {
            let items = self
                .collect_items(item_list.items().collect())
                .context("while attempting to lower generated inline module items")?;
            ModuleSource::Inline { items }
        } else {
            ModuleSource::OutOfLine
        };

        Ok(ModuleItem {
            inner_docs: <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(item, InnerDocs),
            // Legacy macro-use affects the parent's textual macro scope after the generated module
            // is collected, so it must survive the late-source pause too.
            macro_use: MacroUseAttr::maybe_from_ast(item, ()),
            path_override: rg_parse::module_path_override(item),
            source,
        })
    }

    fn lower_trait_item(
        &mut self,
        item: &ast::Trait,
        source: &mut dyn RecordSource,
    ) -> Result<TraitItem> {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self
            .collect_assoc_items(assoc_items)
            .context("while attempting to lower generated trait associated items")?;

        Ok(TraitItem::from_ast(item, (items, source)))
    }

    fn lower_impl_item(
        &mut self,
        item: &ast::Impl,
        source: &mut dyn RecordSource,
    ) -> Result<ImplItem> {
        let assoc_items = item
            .assoc_item_list()
            .map(|item_list| item_list.assoc_items().collect::<Vec<_>>())
            .unwrap_or_default();
        let items = self
            .collect_assoc_items(assoc_items)
            .context("while attempting to lower generated impl associated items")?;

        Ok(ImplItem::from_ast(item, (items, source)))
    }

    fn collect_assoc_items(&mut self, items: Vec<ast::AssocItem>) -> Result<Vec<ItemTreeId>> {
        self.collect_items(
            items
                .into_iter()
                .map(|item| match item {
                    ast::AssocItem::Const(item) => ast::Item::Const(item),
                    ast::AssocItem::Fn(item) => ast::Item::Fn(item),
                    ast::AssocItem::MacroCall(item) => ast::Item::MacroCall(item),
                    ast::AssocItem::TypeAlias(item) => ast::Item::TypeAlias(item),
                })
                .collect(),
        )
    }

    fn name_from_ast(name: Option<ast::Name>) -> Option<Name> {
        name.map(|name| Name::new(name.text()))
    }

    fn name_ref_from_ast(name: Option<ast::NameRef>) -> Option<Name> {
        name.map(|name| Name::new(name.text()))
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
            <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
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
        let location = source.record_as(text_range, SourceRole::Declaration);
        let name_location = name_range.map(|range| source.record(range));
        self.items.alloc(ItemNode::new(
            kind,
            name,
            name_location,
            visibility,
            docs,
            location,
            self.origin.file_id,
        ))
    }

    fn span_mapper(
        span_map: &ExpansionSpanMap,
        fallback_span: TtSpan,
    ) -> impl Fn(TextRange) -> TtSpan + Copy + '_ {
        move |range| span_map.span_for_range(range).unwrap_or(fallback_span)
    }
}
