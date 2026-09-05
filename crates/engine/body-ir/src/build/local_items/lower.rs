//! Signature lowering shared by body-local items and selected current declarations.

use rg_item_tree::{
    ConstItem, Documentation, FromAst as _, FunctionItem, ItemKind, ItemNode, MaybeFromAst,
    OuterDocs, StaticItem, TypeAliasItem, VisibilityLevel,
};
use rg_parse::{LineIndex, Span};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, HasVisibility as _},
};
use rg_text::NameInterner;

use crate::BodySource;

pub(crate) struct LocalItemLowering;

impl LocalItemLowering {
    /// The caller supplies mapped spans so generated syntax keeps its invocation provenance.
    pub(crate) fn declaration(
        item: &ast::Item,
        line_index: &LineIndex,
        interner: &mut NameInterner,
        source: BodySource,
        name_span: Option<Span>,
    ) -> Option<ItemNode> {
        let (kind, name, visibility, docs) = match item {
            ast::Item::Fn(item) => (
                ItemKind::Function(FunctionItem::from_ast(item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
            ),
            ast::Item::Const(item) => (
                ItemKind::Const(ConstItem::from_ast(item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
            ),
            ast::Item::Static(item) => (
                ItemKind::Static(StaticItem::from_ast(item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
            ),
            ast::Item::TypeAlias(item) => (
                ItemKind::TypeAlias(TypeAliasItem::from_ast(item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(item, OuterDocs),
            ),
            _ => return None,
        };
        Some(ItemNode::source(
            kind,
            name.map(|name| interner.intern(name.text())),
            name_span,
            visibility,
            docs,
            source.span,
            source.file_id,
        ))
    }

    pub(crate) fn associated(item: ast::AssocItem) -> Option<ast::Item> {
        match item {
            ast::AssocItem::Fn(item) => Some(ast::Item::Fn(item)),
            ast::AssocItem::Const(item) => Some(ast::Item::Const(item)),
            ast::AssocItem::TypeAlias(item) => Some(ast::Item::TypeAlias(item)),
            ast::AssocItem::MacroCall(_) => None,
        }
    }

    pub(crate) fn name_syntax(item: &ast::Item) -> Option<ast::Name> {
        item.syntax().children().find_map(ast::Name::cast)
    }
}
