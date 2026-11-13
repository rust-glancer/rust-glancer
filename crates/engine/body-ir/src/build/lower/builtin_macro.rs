//! Builtin inputs and implicit references written inside format strings.

use rg_ir_model::{BuiltinMacroExprKind, ExprId, ScopeId};
use rg_syntax::{
    AstNode as _,
    ast::{self, AstToken as _, FormatStringPartKind, HasName as _},
};
use rg_text::Name;

use super::body::BodyLowering;
use crate::body::{BodyPath, BodySource, ExprKind};

impl BodyLowering<'_> {
    /// Lower written arguments and add variable references for captures such as `{user}`.
    pub(super) fn lower_builtin_macro_args(
        &mut self,
        kind: BuiltinMacroExprKind,
        syntax: ast::FormatArgsExpr,
        scope: ScopeId,
    ) -> Vec<ExprId> {
        // Assertion input starts with the condition; its optional message is the format template.
        // Other formatting builtins start with the template itself.
        let template = if kind == BuiltinMacroExprKind::Assert {
            syntax.args().next().and_then(|arg| arg.expr())
        } else {
            syntax.template()
        };
        let named = syntax
            .args()
            .filter_map(|arg| arg.name())
            .map(|name| Name::new(name.text()))
            .collect::<Vec<_>>();
        let mut args = Vec::new();
        for arg in syntax
            .template()
            .into_iter()
            .chain(syntax.args().filter_map(|arg| arg.expr()))
        {
            let Some(arg) = self.cfg.enabled_syntax(arg) else {
                continue;
            };
            args.push(self.lower_expr(arg, scope));
        }

        // Implicit captures require a literal template. Strings computed by macros such as
        // `concat!` cannot capture caller variables, even when their output contains `{name}`.
        let Some(ast::Expr::Literal(template)) = template else {
            return args;
        };
        let source = self.source(template.syntax());
        let ast::LiteralKind::String(string) = template.kind() else {
            return args;
        };
        string.format_parts(|part| {
            let FormatStringPartKind::Name(name) = part.kind else {
                return;
            };
            // In `format!("{user}", user = expr)`, `user` refers to `expr`, not a local.
            // That argument is already in `args`; only implicit captures need another path.
            if named.iter().any(|explicit| explicit.as_str() == name) {
                return;
            }
            let range = part.range + string.syntax().text_range().start();
            let written_span = self.written_source_span(range);
            let mut source = source;
            source.span = written_span.unwrap_or(source.span);
            let expr = self.lower_format_capture(name, source, scope);
            args.push(expr);
            if let Some(span) = written_span {
                self.builder.source_map.record_format_capture(expr, span);
            }
        });
        args
    }

    fn lower_format_capture(&mut self, name: &str, source: BodySource, scope: ScopeId) -> ExprId {
        let name = Name::new(name);
        let path = BodyPath::unqualified_name(name, source.span);
        // Treat `{user}` like a `user` expression here. Normal name lookup then finds
        // the right local, and hover and navigation can use the same path.
        // TODO: Require width and precision captures to have type `usize` during inference.
        self.alloc_expr_with_source(source, scope, ExprKind::Path { path })
    }
}
