use rg_syntax::{
    AstNode as _, Edition,
    ast::{self, HasArgList as _},
};

/// Recover an argument list from the exact written bytes of a projected call.
/// The file parser sees these bytes only as part of the enclosing macro's token tree.
pub(super) fn argument_list(fragment: &str, edition: Edition) -> Option<ast::ArgList> {
    let parsed = ast::Expr::parse(fragment, edition);
    match ast::Expr::cast(parsed.syntax_node())? {
        ast::Expr::CallExpr(call) => call.arg_list(),
        ast::Expr::MethodCallExpr(call) => call.arg_list(),
        _ => None,
    }
}
