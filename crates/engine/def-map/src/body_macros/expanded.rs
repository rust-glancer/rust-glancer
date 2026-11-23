use std::sync::Arc;

use rg_ir_model::{BodySource, BuiltinMacroExprKind, CrateRef, LocalDefRef, Span};
use rg_item_tree::{DeclarationSourceStore, dollar_crate_from_span};
use rg_macro_runtime::ExpansionSyntax;
use rg_syntax::{AstNode, Parse, SyntaxError, SyntaxNode, TextRange, ast};
use rg_tt::{
    Span as TtSpan,
    syntax_bridge::{ExpansionSpanMap, SpanFactory, TokenSourceMap},
};

use super::BodyMacroCallOrigin;
use crate::macro_copies::{forwarded_input_id, is_argument_origin};

/// Generated body syntax plus the macro-definition origin needed while lowering it.
///
/// Output tokens can come from invocation arguments or from the macro definition. Keep the full
/// token map during lowering so Body IR can keep the proven physical ranges after dropping the
/// expansion.
///
/// `source_map` goes from expanded syntax offsets to token origins. `input_sources` goes from
/// this invocation's token IDs to physical ranges. A source lookup follows both maps and checks
/// the call's ownership, so a template token cannot borrow an argument's range merely because
/// their numeric IDs happen to match.
///
/// Forwarded inputs also keep their range in the parent expansion. If two parent tokens came
/// from the same written argument, that range tells us which copy the nested call received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedBodyMacro<T> {
    syntax: T,
    source_map: Arc<ExpansionSpanMap>,
    input_sources: BodyMacroInputSources,
    call_site: TtSpan,
    dollar_crate: CrateRef,
    parse_errors: Vec<SyntaxError>,
    transformed_inputs: Arc<[TtSpan]>,
}

impl<T> ExpandedBodyMacro<T> {
    pub(crate) fn new(
        syntax: T,
        source_map: Arc<ExpansionSpanMap>,
        input_sources: BodyMacroInputSources,
        call_site: TtSpan,
        dollar_crate: CrateRef,
    ) -> Self {
        Self {
            syntax,
            source_map,
            input_sources,
            call_site,
            dollar_crate,
            parse_errors: Vec::new(),
            transformed_inputs: Arc::default(),
        }
    }

    pub fn call_origin(&self) -> BodyMacroCallOrigin {
        BodyMacroCallOrigin::Generated {
            dollar_crate: self.dollar_crate,
            source_map: Arc::clone(&self.source_map),
            input_sources: self.input_sources.clone(),
            call_site: self.call_site,
        }
    }

    pub fn has_parse_errors_in(&self, range: TextRange) -> bool {
        self.parse_errors
            .iter()
            .any(|error| range.intersect(error.range()).is_some())
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

    /// Accept only unchanged tokens copied from this invocation.
    ///
    /// Compare the complete source file identity: caller and definition packages can reuse file IDs.
    /// Require the span to be inside this call and keep the original token's length, so a template
    /// token or whole-call fallback cannot pass as a written argument.
    fn source_for_token_range(&self, call_source: BodySource, range: TextRange) -> Option<Span> {
        let (span, _) = self.argument_origin(call_source, range)?;
        (span.len() == u32::from(range.len())).then_some(span)
    }

    pub(super) fn argument_origin(
        &self,
        call_source: BodySource,
        range: TextRange,
    ) -> Option<(Span, TtSpan)> {
        let origin = self.source_map.span_for_range(range)?;
        let span = self.input_span(call_source, origin)?;
        Some((span, origin))
    }

    fn input_span(&self, call_source: BodySource, span: TtSpan) -> Option<Span> {
        if span.anchor.file_id.raw_file_id() != call_source.file_id.as_raw()
            || !is_argument_origin(self.call_site, span)
        {
            return None;
        }
        let span = self.input_sources.span(span)?;
        call_source.span.contains_span(span).then_some(span)
    }

    /// A forwarded `$crate` can come from a different crate than the expansion around it.
    /// Read the defining crate from the token before using the expansion's default.
    pub fn dollar_crate_at(&self, range: TextRange) -> CrateRef {
        Self::mapped_dollar_crate(&self.source_map, range).unwrap_or(self.dollar_crate)
    }

    /// Default defining crate for a `$crate` without its own token origin.
    pub fn dollar_crate(&self) -> CrateRef {
        self.dollar_crate
    }

    pub(super) fn mapped_dollar_crate(
        map: &ExpansionSpanMap,
        range: TextRange,
    ) -> Option<CrateRef> {
        dollar_crate_from_span(map.span_for_range(range)?)
    }
}

impl<T: AstNode> ExpandedBodyMacro<T> {
    /// Splits out typed generated syntax while keeping source context active for descendants.
    pub fn into_syntax_and_context(self) -> (T, ExpandedBodyMacro<SyntaxNode>) {
        let Self {
            syntax,
            source_map,
            input_sources,
            call_site,
            dollar_crate,
            parse_errors,
            transformed_inputs,
        } = self;
        let context_syntax = syntax.syntax().clone();
        (
            syntax,
            ExpandedBodyMacro {
                syntax: context_syntax,
                source_map,
                input_sources,
                call_site,
                dollar_crate,
                parse_errors,
                transformed_inputs,
            },
        )
    }
}

impl ExpandedBodyMacro<SyntaxNode> {
    /// A recovered parse can leave part of the macro output outside the lowered syntax.
    pub fn has_parse_errors(&self) -> bool {
        !self.parse_errors.is_empty()
    }

    /// Find each output copy, including tokens outside the typed node used for lowering.
    ///
    /// Each result gives the token's range in this expansion, its written span, and its
    /// range in the parent expansion if it was forwarded.
    pub fn argument_tokens(
        &self,
        call_source: BodySource,
    ) -> impl Iterator<Item = (TextRange, Span, Option<TextRange>)> + '_ {
        let root = self
            .syntax
            .ancestors()
            .last()
            .expect("expansion syntax has a root");
        self.source_map
            .token_spans(root.text_range())
            .filter_map(move |(range, origin)| {
                let span = self.input_span(call_source, origin)?;
                Some((range, span, self.input_sources.parent_range(origin)))
            })
    }

    /// Some inputs also affect text created by the template, such as a concatenated name.
    pub fn transformed_inputs(&self, call_source: BodySource) -> impl Iterator<Item = Span> + '_ {
        self.transformed_inputs
            .iter()
            .filter_map(move |&span| self.input_span(call_source, span))
    }

    /// Remap one token, a range within it, or a name node with parser-inserted whitespace.
    /// Compound generated ranges still cannot pass as copied arguments.
    /// Return both the exact input range and the output token that contains it.
    pub fn argument_source(
        &self,
        call_source: BodySource,
        range: TextRange,
    ) -> Option<(Span, TextRange)> {
        let mut token = self.syntax.token_at_offset(range.start()).right_biased()?;
        while token.kind().is_trivia() && token.text_range().end() <= range.end() {
            token = token.next_token()?;
        }
        // A format capture such as `user` in `"{user}"` occupies part of one literal token.
        // First prove that the entire token was copied unchanged, then retain the inner offsets.
        let token_range = token.text_range();
        if token_range.contains_range(range) {
            let source = self.source_for_token_range(call_source, token_range)?;
            let start = source.start + u32::from(range.start() - token_range.start());
            return Some((
                Span {
                    start,
                    end: start + u32::from(range.len()),
                },
                token_range,
            ));
        }
        if !range.contains_range(token_range) {
            return None;
        }
        let source = self.source_for_token_range(call_source, token_range)?;
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
        Some((source, token_range))
    }
}

impl ExpandedBodyMacro<Parse<SyntaxNode>> {
    pub(super) fn from_expansion(
        expansion: ExpansionSyntax,
        input_sources: BodyMacroInputSources,
        call_site: TtSpan,
        dollar_crate: CrateRef,
    ) -> Self {
        Self {
            transformed_inputs: expansion.transformed_inputs.into(),
            ..Self::new(
                expansion.parse,
                Arc::new(expansion.span_map),
                input_sources,
                call_site,
                dollar_crate,
            )
        }
    }

    pub(super) fn cast_root_or_child<N: AstNode>(self) -> Option<ExpandedBodyMacro<N>> {
        let Self {
            syntax: parse,
            source_map,
            input_sources,
            call_site,
            dollar_crate,
            transformed_inputs,
            ..
        } = self;
        let root = parse.syntax_node();
        let syntax = N::cast(root.clone()).or_else(|| root.children().find_map(N::cast))?;
        Some(ExpandedBodyMacro {
            syntax,
            source_map,
            input_sources,
            call_site,
            dollar_crate,
            parse_errors: parse.parse_errors().to_vec(),
            transformed_inputs,
        })
    }
}

/// The source table that owns an expansion's input token IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyMacroInputSources {
    /// A transient table from tokenizing a call found during body lowering.
    Body(Arc<BodyMacroInputMap>),
    /// The file's declaration table supplied the IDs in an item macro's input.
    Declaration(Arc<DeclarationSourceStore<Span>>),
}

impl BodyMacroInputSources {
    pub(super) fn span(&self, token: TtSpan) -> Option<Span> {
        match self {
            Self::Body(map) => map
                .written
                .resolve(token.origins)
                .map(Span::from_text_range),
            Self::Declaration(sources) => sources.token_span(token),
        }
    }

    fn parent_range(&self, token: TtSpan) -> Option<TextRange> {
        let Self::Body(map) = self else {
            return None;
        };
        map.parent_ranges
            .get(forwarded_input_id(token)? as usize)
            .copied()
            .flatten()
    }
}

/// Give each tracked input token its own ID. For a forwarded token, that ID can look up
/// both the written range and the position it came from in the parent expansion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodyMacroInputMap {
    written: TokenSourceMap,
    parent_ranges: Vec<Option<TextRange>>,
}

impl BodyMacroInputMap {
    pub(super) fn record(
        &mut self,
        factory: SpanFactory,
        written: TextRange,
        parent: Option<TextRange>,
    ) -> TtSpan {
        let token = self.written.record(factory, written);
        if let Some(range) = parent {
            let id = token.origins.first() as usize;
            self.parent_ranges.resize(id + 1, None);
            self.parent_ranges[id] = Some(range);
        }
        token
    }
}

/// Expression-position macro call result after body macro lookup.
pub enum BodyMacroExprExpansion {
    /// Declarative macro expansion produced syntax that body lowering should lower recursively.
    Expanded(ExpandedBodyMacro<ast::Expr>),
    /// Compiler-provided expression macro that has no declarative transcriber to execute.
    Builtin {
        kind: BuiltinMacroExprKind,
        /// Parsed input expressions, with the same token provenance as declarative output.
        arguments: Option<ExpandedBodyMacro<ast::FormatArgsExpr>>,
        /// Which expression carries the format syntax. The parser also uses
        /// `FormatArgsExpr` for `assert!(condition, message)`, which starts with a condition
        /// and can have a plain message.
        format_template: Option<ast::Expr>,
    },
}

impl BodyMacroExprExpansion {
    pub(super) fn builtin(
        kind: BuiltinMacroExprKind,
        arguments: Option<ExpandedBodyMacro<ast::FormatArgsExpr>>,
        assert_formats_single_message: bool,
    ) -> Self {
        let format_template = arguments.as_ref().and_then(|args| {
            let syntax = &args.syntax;
            match kind {
                BuiltinMacroExprKind::Assert => {
                    // First is the condition. A single legacy panic message is plain data.
                    let mut args = syntax.args();
                    let message = args.next()?.expr();
                    (assert_formats_single_message || args.next().is_some())
                        .then_some(message)
                        .flatten()
                }
                BuiltinMacroExprKind::FormatArgs | BuiltinMacroExprKind::FormatArgsNl => {
                    syntax.template()
                }
                _ => None,
            }
        });
        Self::Builtin {
            kind,
            arguments,
            format_template,
        }
    }
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
    pub(crate) fn new(definition: LocalDefRef, expansion: Option<T>) -> Self {
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
