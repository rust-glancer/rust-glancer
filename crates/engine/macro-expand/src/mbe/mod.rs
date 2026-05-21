//! Adapted from rust-analyzer's `mbe` crate.
//!
//! `mbe` (short for Macro By Example) contains code for handling
//! `macro_rules` macros. It uses `TokenTree` from the local `tt` module as its
//! interface.

mod expander;
mod macro_call_style;
mod parser;

use rg_tt::span::{Edition, Span, SyntaxContext};
use rg_tt::tt;
use rg_tt::tt::DelimSpan;
use rg_tt::tt::iter::TtIter;

use std::fmt;
use std::sync::Arc;

pub use self::macro_call_style::MacroCallStyle;
use self::parser::MetaTemplate;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ParseError {
    UnexpectedToken(Box<str>),
    Expected(Box<str>),
    InvalidRepeat,
}

impl ParseError {
    fn expected(e: &str) -> ParseError {
        ParseError::Expected(e.into())
    }

    fn unexpected(e: &str) -> ParseError {
        ParseError::UnexpectedToken(e.into())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnexpectedToken(it) => f.write_str(it),
            ParseError::Expected(it) => f.write_str(it),
            ParseError::InvalidRepeat => f.write_str("invalid repeat"),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct ExpandError {
    pub inner: Arc<(Span, ExpandErrorKind)>,
}
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum ExpandErrorKind {
    BindingError(Box<Box<str>>),
    UnresolvedBinding(Box<Box<str>>),
    LeftoverTokens,
    LimitExceeded,
    NoMatchingRule,
    UnexpectedToken,
}

impl ExpandError {
    fn new(span: Span, kind: ExpandErrorKind) -> ExpandError {
        ExpandError {
            inner: Arc::new((span, kind)),
        }
    }
    fn binding_error(span: Span, e: impl Into<Box<str>>) -> ExpandError {
        ExpandError {
            inner: Arc::new((span, ExpandErrorKind::BindingError(Box::new(e.into())))),
        }
    }
}
impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.1.fmt(f)
    }
}

impl fmt::Display for ExpandErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExpandErrorKind::NoMatchingRule => f.write_str("no rule matches input tokens"),
            ExpandErrorKind::UnexpectedToken => f.write_str("unexpected token in input"),
            ExpandErrorKind::BindingError(e) => f.write_str(e),
            ExpandErrorKind::UnresolvedBinding(binding) => {
                f.write_str("could not find binding ")?;
                f.write_str(binding)
            }
            ExpandErrorKind::LimitExceeded => f.write_str("Expand exceed limit"),
            ExpandErrorKind::LeftoverTokens => f.write_str("leftover tokens"),
        }
    }
}

/// Index of the matched macro arm on successful expansion.
pub type MatchedArmIndex = Option<u32>;

/// This struct contains AST for a single `macro_rules` definition. What might
/// be very confusing is that AST has almost exactly the same shape as
/// `tt::TokenTree`, but there's a crucial difference: in macro rules, `$ident`
/// and `$()*` have special meaning (see `Var` and `Repeat` data structures)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarativeMacro {
    rules: Box<[Rule]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    /// Is this a normal fn-like rule, an `attr()` rule, or a `derive()` rule?
    style: MacroCallStyle,
    lhs: MetaTemplate,
    rhs: MetaTemplate,
}

impl DeclarativeMacro {
    /// The old, `macro_rules! m {}` flavor.
    pub fn parse_macro_rules(
        tt: &tt::TopSubtree,
        ctx_edition: impl Copy + Fn(SyntaxContext) -> Edition,
    ) -> DeclarativeMacro {
        // Note: this parsing can be implemented using mbe machinery itself, by
        // matching against `$($lhs:tt => $rhs:tt);*` pattern, but implementing
        // manually seems easier.
        let mut src = tt.iter();
        let mut rules = Vec::new();

        while !src.is_empty() {
            let rule = match Rule::parse(ctx_edition, &mut src) {
                Ok(it) => it,
                Err(_) => break,
            };
            rules.push(rule);
            if let Err(()) = src.expect_char(';') {
                break;
            }
        }

        DeclarativeMacro {
            rules: rules.into_boxed_slice(),
        }
    }

    /// The new, unstable `macro m {}` flavor.
    pub fn parse_macro2(
        args: Option<&tt::TopSubtree>,
        body: &tt::TopSubtree,
        ctx_edition: impl Copy + Fn(SyntaxContext) -> Edition,
    ) -> DeclarativeMacro {
        let mut rules = Vec::new();

        if let Some(args) = args {
            // The presence of an argument list means that this macro uses the
            // "simple" syntax, where the body is the RHS of a single rule.
            let rule: Result<Rule, ParseError> = (|| {
                let lhs = MetaTemplate::parse_pattern(ctx_edition, args.iter())?;
                let rhs = MetaTemplate::parse_template(ctx_edition, body.iter())?;

                // In the "simple" syntax, there is apparently no way to specify
                // that the single rule is an attribute or derive rule, so it
                // must be a function-like rule.
                Ok(Rule {
                    style: MacroCallStyle::FnLike,
                    lhs,
                    rhs,
                })
            })();

            if let Ok(rule) = rule {
                rules.push(rule);
            }
        } else {
            // There was no top-level argument list, so this macro uses the
            // list-of-rules syntax, similar to `macro_rules!`.
            let mut src = body.iter();
            while !src.is_empty() {
                let rule = match Rule::parse(ctx_edition, &mut src) {
                    Ok(it) => it,
                    Err(_) => break,
                };
                rules.push(rule);
                if let Err(()) = src.expect_any_char(&[';', ',']) {
                    break;
                }
            }
        }

        DeclarativeMacro {
            rules: rules.into_boxed_slice(),
        }
    }

    pub fn expand(
        &self,
        tt: &tt::TopSubtree,
        marker: impl Fn(&mut Span) + Copy,
        call_style: MacroCallStyle,
        call_site: Span,
        ctx_edition: impl Copy + Fn(SyntaxContext) -> Edition,
    ) -> ExpandResult<(tt::TopSubtree, MatchedArmIndex)> {
        expander::expand_rules(&self.rules, tt, marker, call_style, call_site, ctx_edition)
    }
}

impl Rule {
    fn parse(
        edition: impl Copy + Fn(SyntaxContext) -> Edition,
        src: &mut TtIter<'_>,
    ) -> Result<Self, ParseError> {
        // Parse an optional `attr()` or `derive()` prefix before the LHS pattern.
        let style = parser::parse_rule_style(src)?;

        let (_, lhs) = src
            .expect_subtree()
            .map_err(|()| ParseError::expected("expected subtree"))?;
        src.expect_char('=')
            .map_err(|()| ParseError::expected("expected `=`"))?;
        src.expect_char('>')
            .map_err(|()| ParseError::expected("expected `>`"))?;
        let (_, rhs) = src
            .expect_subtree()
            .map_err(|()| ParseError::expected("expected subtree"))?;

        let lhs = MetaTemplate::parse_pattern(edition, lhs)?;
        let rhs = MetaTemplate::parse_template(edition, rhs)?;

        Ok(Rule { style, lhs, rhs })
    }
}

pub type ExpandResult<T> = ValueResult<T, ExpandError>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ValueResult<T, E> {
    pub value: T,
    pub err: Option<E>,
}

impl<T: Default, E> Default for ValueResult<T, E> {
    fn default() -> Self {
        Self {
            value: Default::default(),
            err: Default::default(),
        }
    }
}

impl<T, E> ValueResult<T, E> {
    pub fn new(value: T, err: E) -> Self {
        Self {
            value,
            err: Some(err),
        }
    }

    pub fn ok(value: T) -> Self {
        Self { value, err: None }
    }

    pub fn only_err(err: E) -> Self
    where
        T: Default,
    {
        Self {
            value: Default::default(),
            err: Some(err),
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ValueResult<U, E> {
        ValueResult {
            value: f(self.value),
            err: self.err,
        }
    }
}

impl<T: Default, E> From<Result<T, E>> for ValueResult<T, E> {
    fn from(result: Result<T, E>) -> Self {
        result.map_or_else(Self::only_err, Self::ok)
    }
}

pub fn expect_fragment<'t>(
    ctx_edition: impl Copy + Fn(SyntaxContext) -> Edition,
    tt_iter: &mut TtIter<'t>,
    entry_point: ::parser::PrefixEntryPoint,
    delim_span: DelimSpan,
) -> ExpandResult<tt::TokenTreesView<'t>> {
    use ::parser;
    let buffer = tt_iter.remaining();
    let parser_input = to_parser_input(buffer, ctx_edition);
    let tree_traversal = entry_point.parse(&parser_input);
    let mut cursor = buffer.cursor();
    let mut error = false;
    for step in tree_traversal.iter() {
        match step {
            parser::Step::Token {
                kind,
                mut n_input_tokens,
            } => {
                if kind == ::parser::SyntaxKind::LIFETIME_IDENT {
                    n_input_tokens = 2;
                }
                for _ in 0..n_input_tokens {
                    cursor.bump_or_end();
                }
            }
            parser::Step::FloatSplit { .. } => {
                // FIXME: We need to split the tree properly here, but mutating the token trees
                // in the buffer is somewhat tricky to pull off.
                cursor.bump_or_end();
            }
            parser::Step::Enter { .. } | parser::Step::Exit => (),
            parser::Step::Error { .. } => error = true,
        }
    }

    let err = if error || !cursor.is_root() {
        Some(ExpandError::binding_error(
            buffer
                .cursor()
                .token_tree()
                .map_or(delim_span.close, |tt| tt.first_span()),
            format!("expected {entry_point:?}"),
        ))
    } else {
        None
    };

    while !cursor.is_root() {
        cursor.bump_or_end();
    }

    let res = cursor.crossed();
    tt_iter.flat_advance(res.len());

    ExpandResult { value: res, err }
}

fn to_parser_input(
    buffer: tt::TokenTreesView<'_>,
    ctx_edition: impl Copy + Fn(SyntaxContext) -> Edition,
) -> ::parser::Input {
    use ::parser::{SyntaxKind, T};
    use rustc_hash::FxHashMap;

    let mut res = ::parser::Input::with_capacity(buffer.len());
    let mut current = buffer.cursor();
    let mut context_cache = FxHashMap::default();
    let mut edition = |ctx| *context_cache.entry(ctx).or_insert_with(|| ctx_edition(ctx));

    while !current.eof() {
        let tt = current.token_tree();

        if let Some(tt::TokenTree::Leaf(tt::Leaf::Punct(punct))) = tt
            && punct.char == '\''
        {
            current.bump();
            match current.token_tree() {
                Some(tt::TokenTree::Leaf(tt::Leaf::Ident(ident))) => {
                    res.push(SyntaxKind::LIFETIME_IDENT, edition(ident.span.ctx));
                    current.bump();
                    continue;
                }
                _ => panic!("next token after lifetime quote must be an ident"),
            }
        }

        match tt {
            Some(tt::TokenTree::Leaf(leaf)) => {
                match leaf {
                    tt::Leaf::Literal(lit) => {
                        let kind = match lit.kind {
                            tt::LitKind::Byte => SyntaxKind::BYTE,
                            tt::LitKind::Char => SyntaxKind::CHAR,
                            tt::LitKind::Integer => SyntaxKind::INT_NUMBER,
                            tt::LitKind::Float => SyntaxKind::FLOAT_NUMBER,
                            tt::LitKind::Str | tt::LitKind::StrRaw(_) => SyntaxKind::STRING,
                            tt::LitKind::ByteStr | tt::LitKind::ByteStrRaw(_) => {
                                SyntaxKind::BYTE_STRING
                            }
                            tt::LitKind::CStr | tt::LitKind::CStrRaw(_) => SyntaxKind::C_STRING,
                            tt::LitKind::Err(_) => SyntaxKind::ERROR,
                        };
                        res.push(kind, edition(lit.span.ctx));

                        if kind == SyntaxKind::FLOAT_NUMBER && !lit.text().ends_with('.') {
                            res.was_joint();
                        }
                    }
                    tt::Leaf::Ident(ident) => {
                        let edition = edition(ident.span.ctx);
                        match ident.sym.as_str() {
                            "_" => res.push(T![_], edition),
                            i if i.starts_with('\'') => {
                                res.push(SyntaxKind::LIFETIME_IDENT, edition)
                            }
                            _ if ident.is_raw.yes() => res.push(SyntaxKind::IDENT, edition),
                            text => match SyntaxKind::from_keyword(text, edition) {
                                Some(kind) => res.push(kind, edition),
                                None => {
                                    let contextual_keyword =
                                        SyntaxKind::from_contextual_keyword(text, edition)
                                            .unwrap_or(SyntaxKind::IDENT);
                                    res.push_ident(contextual_keyword, edition);
                                }
                            },
                        }
                    }
                    tt::Leaf::Punct(punct) => {
                        let kind = SyntaxKind::from_char(punct.char)
                            .unwrap_or_else(|| panic!("{punct:#?} is not a valid punct"));
                        res.push(kind, edition(punct.span.ctx));
                        if punct.spacing == tt::Spacing::Joint {
                            res.was_joint();
                        }
                    }
                }
                current.bump();
            }
            Some(tt::TokenTree::Subtree(subtree)) => {
                if let Some(kind) = match subtree.delimiter.kind {
                    tt::DelimiterKind::Parenthesis => Some(T!['(']),
                    tt::DelimiterKind::Brace => Some(T!['{']),
                    tt::DelimiterKind::Bracket => Some(T!['[']),
                    tt::DelimiterKind::Invisible => None,
                } {
                    res.push(kind, edition(subtree.delimiter.open.ctx));
                }
                current.bump();
            }
            None => {
                let subtree = current.end();
                if let Some(kind) = match subtree.delimiter.kind {
                    tt::DelimiterKind::Parenthesis => Some(T![')']),
                    tt::DelimiterKind::Brace => Some(T!['}']),
                    tt::DelimiterKind::Bracket => Some(T![']']),
                    tt::DelimiterKind::Invisible => None,
                } {
                    res.push(kind, edition(subtree.delimiter.close.ctx));
                }
            }
        };
    }

    res
}
