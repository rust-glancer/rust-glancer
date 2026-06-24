use rg_ir_model::{BodySource, BuiltinMacroExprKind, TargetRef};
use rg_parse::Span;
use rg_syntax::{AstNode, Parse, SyntaxNode, ast};
use rg_tt::syntax_bridge::ExpansionSpanMap;

/// Generated body syntax plus the macro-definition origin needed while lowering it.
pub struct ExpandedBodyMacro<T> {
    syntax: T,
    source_map: ExpansionSpanMap,
    dollar_crate_target: TargetRef,
}

impl<T> ExpandedBodyMacro<T> {
    pub(super) fn new(
        syntax: T,
        source_map: ExpansionSpanMap,
        dollar_crate_target: TargetRef,
    ) -> Self {
        Self {
            syntax,
            source_map,
            dollar_crate_target,
        }
    }

    /// Maps generated syntax back to a macro invocation token when that mapping is precise.
    pub fn source_for_exact_syntax(
        &self,
        call_source: BodySource,
        syntax: &SyntaxNode,
    ) -> Option<Span> {
        let range = syntax.text_range();

        // Only accept exact single-token source mappings from the original call. Compound
        // generated nodes often contain substituted tokens, but the node as a whole was shaped by
        // the macro transcriber and should stay anchored to the call site.
        let token = syntax.first_token()?;
        if token != syntax.last_token()? || token.text_range() != range {
            return None;
        }

        let span = self
            .source_map
            .span_for_range_in_file(range, call_source.file_id.0)?;
        let span = Span::from_text_range(span.range);
        (call_source.span.contains_span(span) && span.len() == u32::from(range.len()))
            .then_some(span)
    }

    /// Returns the crate that generated `$crate` paths inside this syntax should resolve to.
    pub fn dollar_crate_target(&self) -> TargetRef {
        self.dollar_crate_target
    }
}

impl<T: AstNode> ExpandedBodyMacro<T> {
    /// Splits out typed generated syntax while keeping source context active for descendants.
    pub fn into_syntax_and_context(self) -> (T, ExpandedBodyMacro<SyntaxNode>) {
        let Self {
            syntax,
            source_map,
            dollar_crate_target,
        } = self;
        let context_syntax = syntax.syntax().clone();
        (
            syntax,
            ExpandedBodyMacro::new(context_syntax, source_map, dollar_crate_target),
        )
    }
}

impl ExpandedBodyMacro<Parse<SyntaxNode>> {
    pub(super) fn cast_root_or_child<N: AstNode>(self) -> Option<ExpandedBodyMacro<N>> {
        let Self {
            syntax: parse,
            source_map,
            dollar_crate_target,
        } = self;
        let root = parse.syntax_node();
        let syntax = N::cast(root.clone()).or_else(|| root.children().find_map(N::cast))?;
        Some(ExpandedBodyMacro::new(
            syntax,
            source_map,
            dollar_crate_target,
        ))
    }
}

/// Expression-position macro call result after body macro lookup.
pub enum BodyMacroExprExpansion {
    /// Declarative macro expansion produced syntax that body lowering should lower recursively.
    Expanded(ExpandedBodyMacro<ast::Expr>),
    /// Compiler-provided expression macro that has no declarative transcriber to execute.
    Builtin(BuiltinMacroExprKind),
}
