use std::sync::Arc;

use rg_ir_model::{BodySource, BuiltinMacroExprKind, CrateRef, LocalDefRef, PackageSlot, Span};
use rg_syntax::{AstNode, Parse, SyntaxNode, TextRange, ast};
use rg_tt::syntax_bridge::ExpansionSpanMap;

/// Generated body syntax plus the macro-definition origin needed while lowering it.
///
/// Output tokens can come from invocation arguments or from the macro definition. Keep the full
/// token map while lowering this syntax; Body IR only needs the proven argument spans afterward.
pub struct ExpandedBodyMacro<T> {
    syntax: T,
    source_map: Arc<ExpansionSpanMap>,
    call_package: PackageSlot,
    dollar_crate: CrateRef,
}

impl<T> ExpandedBodyMacro<T> {
    pub(super) fn new(
        syntax: T,
        source_map: ExpansionSpanMap,
        call_package: PackageSlot,
        dollar_crate: CrateRef,
    ) -> Self {
        Self {
            syntax,
            source_map: Arc::new(source_map),
            call_package,
            dollar_crate,
        }
    }

    pub fn source_map(&self) -> Arc<ExpansionSpanMap> {
        Arc::clone(&self.source_map)
    }

    /// Return the original argument span when this node contains exactly one copied token.
    pub fn source_for_exact_syntax(
        &self,
        call_source: BodySource,
        syntax: &SyntaxNode,
    ) -> Option<Span> {
        // The token-tree parser can place separating whitespace inside a name node. Ignore
        // that trivia: `value ` still names exactly one input token, whereas `value.field` does
        // not represent one source occurrence.
        let mut tokens = syntax
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !token.kind().is_trivia());
        let token = tokens.next()?;
        if tokens.next().is_some() {
            return None;
        }
        self.source_for_token_range(call_source, token.text_range())
    }

    /// Find copied input tokens, including tokens forwarded through nested expansions.
    pub fn argument_spans(&self, call_source: BodySource) -> impl Iterator<Item = Span> + '_
    where
        T: AstNode,
    {
        self.syntax
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !token.kind().is_trivia())
            .filter_map(move |token| self.source_for_token_range(call_source, token.text_range()))
    }

    /// Accept only unchanged tokens copied from this invocation.
    ///
    /// Compare the complete source file identity: caller and definition packages can reuse file IDs.
    /// Require the span to be inside this call and keep the original token's length, so a template
    /// token or whole-call fallback cannot pass as a written argument.
    pub fn source_for_token_range(
        &self,
        call_source: BodySource,
        range: TextRange,
    ) -> Option<Span> {
        let span = self.source_map.span_for_range_in_file(
            range,
            self.call_package.0,
            call_source.file_id.0,
        )?;
        let span = Span::from_text_range(span.range);
        (call_source.span.contains_span(span) && span.len() == u32::from(range.len()))
            .then_some(span)
    }

    /// Returns the crate that generated `$crate` paths inside this syntax should resolve to.
    pub fn dollar_crate(&self) -> CrateRef {
        self.dollar_crate
    }
}

impl<T: AstNode> ExpandedBodyMacro<T> {
    /// Splits out typed generated syntax while keeping source context active for descendants.
    pub fn into_syntax_and_context(self) -> (T, ExpandedBodyMacro<SyntaxNode>) {
        let Self {
            syntax,
            source_map,
            call_package,
            dollar_crate,
        } = self;
        let context_syntax = syntax.syntax().clone();
        (
            syntax,
            ExpandedBodyMacro {
                syntax: context_syntax,
                source_map,
                call_package,
                dollar_crate,
            },
        )
    }
}

impl ExpandedBodyMacro<SyntaxNode> {
    /// Type syntax retains ranges of name nodes, which may include parser-inserted whitespace.
    /// Recover the one real token without treating a compound generated range as an argument.
    pub fn source_for_range(&self, call_source: BodySource, range: TextRange) -> Option<Span> {
        let mut token = self.syntax.token_at_offset(range.start()).right_biased()?;
        while token.kind().is_trivia() && token.text_range().end() <= range.end() {
            token = token.next_token()?;
        }
        if !range.contains_range(token.text_range()) {
            return None;
        }
        let source = self.source_for_token_range(call_source, token.text_range())?;
        let mut next = token.next_token();
        while let Some(token) = next {
            if token.text_range().start() >= range.end() {
                break;
            }
            if !token.kind().is_trivia() {
                return None;
            }
            next = token.next_token();
        }
        Some(source)
    }
}

impl ExpandedBodyMacro<Parse<SyntaxNode>> {
    pub(super) fn cast_root_or_child<N: AstNode>(self) -> Option<ExpandedBodyMacro<N>> {
        let Self {
            syntax: parse,
            source_map,
            call_package,
            dollar_crate,
        } = self;
        let root = parse.syntax_node();
        let syntax = N::cast(root.clone()).or_else(|| root.children().find_map(N::cast))?;
        Some(ExpandedBodyMacro {
            syntax,
            source_map,
            call_package,
            dollar_crate,
        })
    }
}

/// Expression-position macro call result after body macro lookup.
pub enum BodyMacroExprExpansion {
    /// Declarative macro expansion produced syntax that body lowering should lower recursively.
    Expanded(ExpandedBodyMacro<ast::Expr>),
    /// Compiler-provided expression macro that has no declarative transcriber to execute.
    Builtin(BuiltinMacroExprKind),
}

/// Result of body macro lookup plus the position-specific expansion, if one was produced.
///
/// A resolved macro call is useful to editor features even when expansion fails or the macro is not
/// supported in the current syntax position. Keeping the selected definition next to the expansion
/// result prevents body lowering from asking macro resolution the same question twice.
pub struct BodyMacroExpansionOutcome<T> {
    /// Macro definition selected by body macro lookup.
    pub definition: LocalDefRef,
    /// Syntax or builtin marker produced for the specific body position.
    pub expansion: Option<T>,
}

impl<T> BodyMacroExpansionOutcome<T> {
    pub(super) fn new(definition: LocalDefRef, expansion: Option<T>) -> Self {
        Self {
            definition,
            expansion,
        }
    }
}

/// Expression-position macro lookup outcome.
///
/// The expansion is generated expression syntax for declarative macros, or a builtin expression
/// marker for compiler-provided expression macros such as `format_args!`.
pub type BodyMacroExprExpansionOutcome = BodyMacroExpansionOutcome<BodyMacroExprExpansion>;

/// Statement-position macro lookup outcome.
///
/// The expansion is generated `MacroStmts` syntax when the selected macro can be parsed as a body
/// statement list.
pub type BodyMacroStmtExpansionOutcome =
    BodyMacroExpansionOutcome<ExpandedBodyMacro<ast::MacroStmts>>;

/// Pattern-position macro lookup outcome.
///
/// The expansion is generated pattern syntax when the selected macro can be parsed in a pattern
/// position.
pub type BodyMacroPatExpansionOutcome = BodyMacroExpansionOutcome<ExpandedBodyMacro<ast::Pat>>;

/// Type-position macro lookup outcome.
///
/// The expansion is generated type syntax when the selected macro can be parsed in a type
/// position.
pub type BodyMacroTypeExpansionOutcome = BodyMacroExpansionOutcome<ExpandedBodyMacro<ast::Type>>;
