//! This file is actually hand-written, but the submodules are indeed generated.
#[cfg_attr(
    dylint_lib = "rust_glancer_lints",
    allow(
        rust_glancer_impl_helpers,
        rust_glancer_implicit_local_imports,
        rust_glancer_non_adjacent_impls,
        rust_glancer_pub_in
    )
)]
#[rustfmt::skip]
pub(crate) mod nodes;
#[cfg_attr(
    dylint_lib = "rust_glancer_lints",
    allow(
        rust_glancer_impl_helpers,
        rust_glancer_implicit_local_imports,
        rust_glancer_non_adjacent_impls,
        rust_glancer_pub_in
    )
)]
#[rustfmt::skip]
pub(crate) mod tokens;

pub(crate) use self::nodes::*;
use crate::{
    AstNode,
    SyntaxKind::{self, *},
    SyntaxNode,
};

// Stmt is the only nested enum, so it's easier to just hand-write it
impl AstNode for Stmt {
    fn can_cast(kind: SyntaxKind) -> bool {
        match kind {
            LET_STMT | EXPR_STMT => true,
            _ => Item::can_cast(kind),
        }
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        let res = match syntax.kind() {
            LET_STMT => Stmt::LetStmt(LetStmt { syntax }),
            EXPR_STMT => Stmt::ExprStmt(ExprStmt { syntax }),
            _ => {
                let item = Item::cast(syntax)?;
                Stmt::Item(item)
            }
        };
        Some(res)
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Stmt::LetStmt(it) => &it.syntax,
            Stmt::ExprStmt(it) => &it.syntax,
            Stmt::Item(it) => it.syntax(),
        }
    }
}
