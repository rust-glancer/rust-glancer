//! Builtin inputs and implicit references written inside format strings.

use std::ops::Range;

use rg_ir_model::{BuiltinMacroExprKind, ExprId, ScopeId};
use rg_syntax::{
    AstNode as _, TextRange, TextSize,
    ast::{self, AstToken as _, HasName as _, IsString as _},
};
use rustc_parse_format::{Count, ParseMode, Parser, Piece, Position};

use super::body::BodyLowering;
use crate::body::{BodyPath, BodySource, ExprKind};

impl BodyLowering<'_> {
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
            .map(|name| self.interner.intern(name.text()))
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
        let Ok(value) = string.value() else {
            return args;
        };
        let style = string.is_raw().then(|| {
            string
                .text()
                .bytes()
                .skip(1)
                .take_while(|&byte| byte == b'#')
                .count()
        });
        // Supplying the original literal lets rustc's parser report ranges before unescaping,
        // including Unicode escapes and line continuations. Offsets count the token's quote prefix.
        let parser = Parser::new(
            &value,
            style,
            Some(string.text().to_owned()),
            false,
            ParseMode::Format,
        );
        for piece in parser {
            let Piece::NextArgument(argument) = piece else {
                continue;
            };
            let captures = [
                match argument.position {
                    Position::ArgumentNamed(name) => Some((name, argument.position_span)),
                    _ => None,
                },
                match argument.format.width {
                    Count::CountIsName(name, range) => Some((name, range)),
                    _ => None,
                },
                match argument.format.precision {
                    Count::CountIsName(name, range) => Some((name, range)),
                    _ => None,
                },
            ];
            for (name, range) in captures.into_iter().flatten() {
                // `{name}` refers to an explicit formatting label when one was supplied; it does
                // not capture a same-named local. Its value was already lowered above.
                if named.iter().any(|explicit| explicit.as_str() == name) {
                    continue;
                }
                args.push(self.lower_format_capture(&string, name, range, source, scope));
            }
        }
        args
    }

    fn lower_format_capture(
        &mut self,
        string: &ast::String,
        name: &str,
        range: Range<usize>,
        mut source: BodySource,
        scope: ScopeId,
    ) -> ExprId {
        let range = TextRange::new(
            TextSize::try_from(range.start).expect("literal offset fits syntax storage"),
            TextSize::try_from(range.end).expect("literal offset fits syntax storage"),
        );
        // The parser's position range includes trailing whitespace in `{name  }`. Trim decoded
        // whitespace while preserving the full source spelling of an escaped identifier character.
        let snippet = &string.text()[range];
        let mut end = range.start();
        if string.is_raw() {
            end += TextSize::of(snippet.trim_end());
        } else {
            string.unescape(snippet, |character_range, character| {
                if character.is_ok_and(|character| !character.is_whitespace()) {
                    end = range.start()
                        + TextSize::try_from(character_range.end)
                            .expect("literal offset fits syntax storage");
                }
            });
        }
        let range = TextRange::new(range.start(), end) + string.syntax().text_range().start();
        let span = self.source_span(range);
        source.span = span;
        let name = self.interner.intern(name);
        let path = BodyPath::unqualified_name(name, span);
        // Captures are ordinary value paths. Reuse lexical lookup, inference, and source queries
        // instead of teaching each editor feature how to interpret a format string.
        // TODO: Apply `usize` expectations to width and precision inputs during inference.
        self.alloc_expr_with_source(source, scope, ExprKind::Path { path })
    }
}
