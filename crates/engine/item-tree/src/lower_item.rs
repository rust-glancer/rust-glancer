//! Shared syntax lowering for file, generated, and body-local declarations.

use rg_ir_model::{CrateRef, FileId};
use rg_syntax::{
    AstNode as _, TextRange,
    ast::{self, HasModuleItem as _},
};
use rg_text::Name;
use rg_tt::Span as TtSpan;

use crate::{
    CfgExpr, ConstItem, Documentation, EnumItem, ExternBlockItem, ExternCrateItem, FromAst,
    FunctionItem, ImplItem, InnerDocs, ItemKind, ItemNode, ItemTreeId, LangItem, MacroCallContext,
    MacroCallItem, MacroDefAst, MacroDefContext, MacroDefinitionItem, MacroRulesAst,
    MacroRulesContext, MacroUseAttr, MaybeFromAst, ModuleItem, ModuleSource, OuterDocs,
    RecordSource, SourceRole, StaticItem, StructItem, TraitItem, TypeAliasItem, UnionItem, UseItem,
    UserFacingAttrs, VisibilityLevel,
};

/// Token origins used to retain macro input along with the declaration.
pub struct ItemMacroSpans<'a> {
    pub token: &'a dyn Fn(TextRange, &mut dyn RecordSource) -> TtSpan,
    /// An item macro can emit another call. Use the original invocation as its fallback.
    pub call_site: Option<TtSpan>,
}

pub struct LoweredItem {
    pub node: ItemNode,
    /// Range of the whole declaration in the parse being lowered, including the function
    /// signature when present. This lets a body reader reopen the item with its parameters.
    pub body_range: Option<TextRange>,
}

/// Lowers a declaration using the caller's source recorder and child item IDs.
pub struct ItemLowering<'a> {
    pub file_id: FileId,
    pub dollar_crate: Option<CrateRef>,
    /// Body-local lowering handles expression and statement macros separately.
    pub macro_spans: Option<ItemMacroSpans<'a>>,
}

impl ItemLowering<'_> {
    /// Visit child declarations before lowering the parent header. Each child gets its own
    /// source recording, and the parent refers to it by its allocated item ID.
    pub fn lower_children(
        item: &ast::Item,
        lower: impl FnMut(ast::Item) -> Option<ItemTreeId>,
    ) -> Vec<ItemTreeId> {
        let associated = match item {
            ast::Item::Impl(item) => item.assoc_item_list(),
            ast::Item::Trait(item) => item.assoc_item_list(),
            ast::Item::Module(item) => {
                return item
                    .item_list()
                    .into_iter()
                    .flat_map(|list| list.items())
                    .filter_map(lower)
                    .collect();
            }
            ast::Item::ExternBlock(item) => {
                return item
                    .extern_item_list()
                    .into_iter()
                    .flat_map(|list| list.extern_items())
                    .map(|item| match item {
                        ast::ExternItem::Fn(item) => item.into(),
                        ast::ExternItem::Static(item) => item.into(),
                        ast::ExternItem::TypeAlias(item) => item.into(),
                        ast::ExternItem::MacroCall(item) => item.into(),
                    })
                    .filter_map(lower)
                    .collect();
            }
            _ => return Vec::new(),
        };
        associated
            .into_iter()
            .flat_map(|list| list.assoc_items())
            .map(|item| match item {
                ast::AssocItem::Const(item) => item.into(),
                ast::AssocItem::Fn(item) => item.into(),
                ast::AssocItem::MacroCall(item) => item.into(),
                ast::AssocItem::TypeAlias(item) => item.into(),
            })
            .filter_map(lower)
            .collect()
    }

    pub fn lower(
        &self,
        item: &ast::Item,
        children: Vec<ItemTreeId>,
        source: &mut dyn RecordSource,
    ) -> Option<LoweredItem> {
        let mut has_body = false;
        let kind = match item {
            ast::Item::AsmExpr(_) => ItemKind::AsmExpr,
            ast::Item::Const(item) => {
                has_body = item.body().is_some();
                ItemKind::Const(ConstItem::from_ast(item, source))
            }
            ast::Item::Enum(item) => {
                ItemKind::Enum(EnumItem::from_ast(item, (self.dollar_crate, source)))
            }
            ast::Item::ExternBlock(item) => {
                ItemKind::ExternBlock(ExternBlockItem::from_ast(item, children))
            }
            ast::Item::ExternCrate(item) => {
                ItemKind::ExternCrate(ExternCrateItem::from_ast(item, source))
            }
            ast::Item::Fn(item) => {
                has_body = item.body().is_some();
                ItemKind::Function(FunctionItem::from_ast(item, source))
            }
            ast::Item::Impl(item) => ItemKind::Impl(ImplItem::from_ast(item, (children, source))),
            ast::Item::MacroCall(item) => {
                let call = if let Some(spans) = &self.macro_spans {
                    let call_site = spans
                        .call_site
                        .unwrap_or_else(|| (spans.token)(item.syntax().text_range(), source));
                    MacroCallItem::from_ast(
                        item,
                        MacroCallContext {
                            builtin: None,
                            call_site,
                            span_for_range: &mut |range| (spans.token)(range, source),
                        },
                    )
                } else if item
                    .syntax()
                    .parent()
                    .is_some_and(|parent| ast::ExternItemList::can_cast(parent.kind()))
                {
                    // Keep foreign calls visible even when the caller cannot expand them.
                    MacroCallItem {
                        path: item.path().map(|path| path.syntax().text().to_string()),
                        callee: item
                            .path()
                            .and_then(|path| path.segment())
                            .and_then(|segment| segment.name_ref())
                            .map(|name| Name::new(name.text())),
                        input: None,
                        builtin: None,
                    }
                } else {
                    return None;
                };
                ItemKind::MacroCall(call)
            }
            ast::Item::MacroDef(item) => {
                let spans = self.macro_spans.as_ref()?;
                ItemKind::MacroDefinition(<MacroDefinitionItem as FromAst<MacroDefAst>>::from_ast(
                    item,
                    MacroDefContext {
                        span_for_range: &mut |range| (spans.token)(range, source),
                    },
                ))
            }
            ast::Item::MacroRules(item) => {
                let spans = self.macro_spans.as_ref()?;
                ItemKind::MacroDefinition(
                    <MacroDefinitionItem as FromAst<MacroRulesAst>>::from_ast(
                        item,
                        MacroRulesContext {
                            span_for_range: &mut |range| (spans.token)(range, source),
                        },
                    ),
                )
            }
            ast::Item::Module(item) => ItemKind::Module(ModuleItem {
                inner_docs: <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(
                    item, InnerDocs,
                ),
                macro_use: MacroUseAttr::maybe_from_ast(item, ()),
                path_override: rg_parse::module_path_override(item),
                source: if item.item_list().is_some() {
                    ModuleSource::Inline { items: children }
                } else {
                    ModuleSource::OutOfLine
                },
            }),
            ast::Item::Static(item) => {
                has_body = item.body().is_some();
                ItemKind::Static(StaticItem::from_ast(item, source))
            }
            ast::Item::Struct(item) => {
                ItemKind::Struct(StructItem::from_ast(item, (self.dollar_crate, source)))
            }
            ast::Item::Trait(item) => {
                ItemKind::Trait(TraitItem::from_ast(item, (children, source)))
            }
            ast::Item::TypeAlias(item) => {
                ItemKind::TypeAlias(TypeAliasItem::from_ast(item, source))
            }
            ast::Item::Union(item) => ItemKind::Union(UnionItem::from_ast(item, source)),
            ast::Item::Use(item) => ItemKind::Use(UseItem::from_ast(item, source)),
        };

        let (name, name_range) = match item {
            ast::Item::ExternCrate(item) => item
                .name_ref()
                .map(|name| (Name::new(name.text()), name.syntax().text_range())),
            _ => item
                .syntax()
                .children()
                .find_map(ast::Name::cast)
                .map(|name| (Name::new(name.text()), name.syntax().text_range())),
        }
        .unzip();
        let visibility = item.syntax().children().find_map(ast::Visibility::cast);
        let range = item.syntax().text_range();
        let name_location = name_range.map(|range| source.record(range));
        let location = source.record_as(range, SourceRole::Declaration);
        // Global assembly is an unnamed marker. Its payload is opaque to declaration lowering.
        let (cfg, lang_item, docs, user_facing_attrs) = if matches!(item, ast::Item::AsmExpr(_)) {
            (CfgExpr::default(), None, None, UserFacingAttrs::default())
        } else {
            (
                CfgExpr::from_attrs(item),
                LangItem::maybe_from_ast(item, ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
                UserFacingAttrs::from_ast(item),
            )
        };
        Some(LoweredItem {
            node: ItemNode {
                kind,
                name,
                name_location,
                visibility: VisibilityLevel::from_ast(&visibility, ()),
                cfg,
                lang_item,
                docs,
                user_facing_attrs,
                file_id: self.file_id,
                location,
            },
            body_range: has_body.then_some(range),
        })
    }
}
