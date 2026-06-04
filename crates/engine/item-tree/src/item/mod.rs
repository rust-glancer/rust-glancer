mod decl;
mod docs;
mod import;
mod macro_item;
mod type_ref;
mod visibility;

pub use rg_ir_model::items::*;

pub use self::{
    decl::{ImplItemContext, TraitItemContext},
    docs::{InnerDocs, OuterDocs},
    macro_item::{
        MacroCallContext, MacroDefAst, MacroDefContext, MacroRulesAst, MacroRulesContext,
    },
};

pub(crate) use self::type_ref::type_bound_list_from_ast;

pub trait FromAst<Mode = ()> {
    type AstNode: ?Sized;
    type Context<'a>;

    fn from_ast(node: &Self::AstNode, ctx: Self::Context<'_>) -> Self;
}

pub trait MaybeFromAst<Mode = ()> {
    type AstNode: ?Sized;
    type Context<'a>;

    fn maybe_from_ast(node: &Self::AstNode, ctx: Self::Context<'_>) -> Option<Self>
    where
        Self: Sized;
}

fn normalized_syntax(node: &impl rg_syntax::AstNode) -> String {
    node.syntax()
        .text()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
