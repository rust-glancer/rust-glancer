//! Declarative macro expansion for rust-glancer.
//!
//! This crate keeps the rust-analyzer-derived MBE engine behind a small API that
//! works with rust-glancer's syntax and def-map data. Callers either pass parsed
//! `rg_syntax` macro nodes or the stored macro pieces from def-map, then receive
//! generated source text that can be parsed through the normal item pipeline.
//!
//! Expansion intentionally ends at text for now. That avoids coupling macro
//! expansion to the private frozen-tree builder in `rg_syntax`, while still
//! making generated items visible to later analysis phases.

extern crate ra_ap_rustc_lexer as rustc_lexer;

mod mbe;
mod span;
mod tt;

use crate::tt::symbol::Symbol;
use anyhow::Context as _;
use rg_syntax::{AstNode as _, NodeOrToken, SyntaxKind, SyntaxToken, T, TextRange, TextSize, ast};
use span::{EditionedFileId, ROOT_ERASED_FILE_AST_ID, Span, SpanAnchor, SyntaxContext};

pub use crate::span::Edition;

/// Compiled declarative macro ready to expand function-like calls.
///
/// The inner matcher/transcriber comes from the vendored MBE engine. This wrapper
/// owns the edition and the conversion between `rg_syntax` token trees and the
/// token-tree representation expected by that engine.
#[derive(Debug, Clone)]
pub struct DeclarativeMacro {
    inner: mbe::DeclarativeMacro,
    edition: Edition,
}

impl DeclarativeMacro {
    /// Compiles a `macro_rules!` definition from the pieces stored in def-map.
    ///
    /// Def-map stores only the body token tree text, not a full source item. To
    /// reuse the same parser path as source-backed definitions, this creates a
    /// short-lived synthetic `SourceFile` containing a dummy macro name, finds
    /// the parsed macro node inside it, and immediately converts that node into
    /// the vendored token-tree representation.
    pub fn from_macro_rules_parts(
        body: &str,
        edition: Edition,
        file_id: u32,
    ) -> anyhow::Result<Self> {
        let source = format!("macro_rules! __rg_macro {body}");
        let file = ast::SourceFile::parse(&source, edition)
            .ok()
            .map_err(|errors| {
                anyhow::anyhow!("macro_rules source has syntax errors: {errors:?}")
            })?;
        let item = file
            .syntax()
            .descendants()
            .find_map(ast::MacroRules::cast)
            .context("while attempting to find parsed macro_rules item")?;
        Self::from_macro_rules(&item, edition, file_id)
    }

    /// Compiles a `macro_rules!` definition from a parsed syntax node.
    ///
    /// `file_id` anchors spans created for the vendored expander. The first
    /// integration path renders expanded tokens back to text, so these spans are
    /// used mainly for matching, diagnostics, and future span-aware work.
    pub fn from_macro_rules(
        item: &ast::MacroRules,
        edition: Edition,
        file_id: u32,
    ) -> anyhow::Result<Self> {
        let body = item
            .token_tree()
            .context("while attempting to fetch macro_rules body")?;
        let span_factory = SpanFactory::new(file_id, edition);
        let body = token_tree_to_tt(&body, span_factory);
        let inner = mbe::DeclarativeMacro::parse_macro_rules(&body, move |ctx| ctx.edition());
        Ok(Self { inner, edition })
    }

    /// Compiles a `macro` definition from the pieces stored in def-map.
    ///
    /// Like `from_macro_rules_parts`, this reconstructs a minimal synthetic item
    /// only long enough for `rg_syntax` to parse token-tree boundaries exactly as
    /// it would in real source.
    pub fn from_macro_def_parts(
        args: Option<&str>,
        body: &str,
        edition: Edition,
        file_id: u32,
    ) -> anyhow::Result<Self> {
        let source = match args {
            Some(args) => format!("macro __rg_macro {args} {body}"),
            None => format!("macro __rg_macro {body}"),
        };
        let file = ast::SourceFile::parse(&source, edition)
            .ok()
            .map_err(|errors| anyhow::anyhow!("macro source has syntax errors: {errors:?}"))?;
        let item = file
            .syntax()
            .descendants()
            .find_map(ast::MacroDef::cast)
            .context("while attempting to find parsed macro item")?;
        Self::from_macro_def(&item, edition, file_id)
    }

    /// Compiles a `macro` definition from a parsed syntax node.
    pub fn from_macro_def(
        item: &ast::MacroDef,
        edition: Edition,
        file_id: u32,
    ) -> anyhow::Result<Self> {
        let span_factory = SpanFactory::new(file_id, edition);
        let args = item
            .args()
            .map(|args| token_tree_to_tt(&args, span_factory));
        let body = item
            .body()
            .context("while attempting to fetch macro body")?;
        let body = token_tree_to_tt(&body, span_factory);
        let inner =
            mbe::DeclarativeMacro::parse_macro2(args.as_ref(), &body, move |ctx| ctx.edition());
        Ok(Self { inner, edition })
    }

    /// Expands a parsed function-like macro call into source text.
    ///
    /// The returned text is intentionally not parsed here. Def-map integration
    /// decides the syntactic context of the expansion, such as wrapping generated
    /// item text in a temporary source file before collecting item-tree data.
    pub fn expand_call(
        &self,
        call: &ast::MacroCall,
        file_id: u32,
    ) -> anyhow::Result<ExpansionText> {
        let args = call
            .token_tree()
            .context("while attempting to fetch macro call arguments")?;
        let span_factory = SpanFactory::new(file_id, self.edition);
        let call_site = span_factory.span_for(call.syntax().text_range());
        let args = token_tree_to_tt(&args, span_factory);
        let expanded = self.inner.expand(
            &args,
            |_| {},
            mbe::MacroCallStyle::FnLike,
            call_site,
            move |ctx| ctx.edition(),
        );

        if let Some(err) = expanded.err {
            anyhow::bail!("macro expansion failed: {err}");
        }

        Ok(ExpansionText {
            source: render_expansion(expanded.value.0.view().token_trees()),
        })
    }

    /// Expands a macro call from stored path and argument text.
    ///
    /// This is the call-side twin of the `*_parts` constructors: it builds an
    /// ephemeral source snippet containing the path, stored delimiter+argument
    /// text, and a trailing semicolon. The snippet is parsed as a `SourceFile`,
    /// the macro call AST node is converted to token trees, and the syntax tree
    /// is then discarded. The snippet is not inserted into any database;
    /// `file_id` is only the span anchor used while the vendored engine runs.
    pub fn expand_call_parts(
        &self,
        path: &str,
        args: &str,
        file_id: u32,
    ) -> anyhow::Result<ExpansionText> {
        let source = format!("{path}!{args};");
        let file = ast::SourceFile::parse(&source, self.edition)
            .ok()
            .map_err(|errors| anyhow::anyhow!("macro call source has syntax errors: {errors:?}"))?;
        let call = file
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .context("while attempting to find parsed macro call")?;
        self.expand_call(&call, file_id)
    }
}

/// Source text produced by a successful declarative macro expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionText {
    /// Generated Rust source to parse in the caller's expected syntactic context.
    pub source: String,
}

/// Produces coarse spans for token trees passed into the vendored expander.
///
/// rust-glancer does not preserve macro-expanded syntax trees yet, but the
/// matcher still needs stable span values for errors and syntax-context-aware
/// operations. The factory keeps every generated span anchored to the caller's
/// file id and edition.
#[derive(Clone, Copy)]
struct SpanFactory {
    anchor: SpanAnchor,
    ctx: SyntaxContext,
}

impl SpanFactory {
    fn new(file_id: u32, edition: Edition) -> Self {
        Self {
            anchor: SpanAnchor {
                file_id: EditionedFileId::new(file_id, edition),
                ast_id: ROOT_ERASED_FILE_AST_ID,
            },
            ctx: SyntaxContext::root(edition),
        }
    }

    fn span_for(self, range: TextRange) -> Span {
        Span {
            range,
            anchor: self.anchor,
            ctx: self.ctx,
        }
    }
}

fn token_tree_to_tt(tree: &ast::TokenTree, span_factory: SpanFactory) -> tt::TopSubtree {
    // `rg_syntax` exposes token trees as regular syntax nodes. The vendored MBE
    // engine wants a compact tree with explicit delimiters, so conversion starts
    // by recording the outer delimiter and then streams the children into the
    // flat token-tree builder.
    let delimiter = delimiter_for(tree, span_factory);
    let mut builder = tt::TopSubtreeBuilder::new(delimiter);
    push_token_tree_children(&mut builder, tree, span_factory);
    builder.build()
}

fn push_nested_token_tree(
    builder: &mut tt::TopSubtreeBuilder,
    tree: &ast::TokenTree,
    span_factory: SpanFactory,
) {
    let delimiter = delimiter_for(tree, span_factory);
    builder.open(delimiter.kind, delimiter.open);
    push_token_tree_children(builder, tree, span_factory);
    builder.close(delimiter.close);
}

fn push_token_tree_children(
    builder: &mut tt::TopSubtreeBuilder,
    tree: &ast::TokenTree,
    span_factory: SpanFactory,
) {
    // Delimiter tokens are represented by the subtree itself in `tt`, not by
    // separate punctuation leaves. Skip the concrete syntax tokens after their
    // ranges have been copied into the delimiter span.
    let left = tree.left_delimiter_token().map(|token| token.text_range());
    let right = tree.right_delimiter_token().map(|token| token.text_range());

    for child in tree.token_trees_and_tokens() {
        match child {
            NodeOrToken::Node(tree) => push_nested_token_tree(builder, &tree, span_factory),
            NodeOrToken::Token(token)
                if Some(token.text_range()) == left || Some(token.text_range()) == right => {}
            NodeOrToken::Token(token) => push_token(builder, &token, span_factory),
        }
    }
}

fn delimiter_for(tree: &ast::TokenTree, span_factory: SpanFactory) -> tt::Delimiter {
    let Some(left) = tree.left_delimiter_token() else {
        return tt::Delimiter::invisible_spanned(span_factory.span_for(tree.syntax().text_range()));
    };
    let Some(right) = tree.right_delimiter_token() else {
        return tt::Delimiter::invisible_spanned(span_factory.span_for(tree.syntax().text_range()));
    };

    let kind = match left.kind() {
        T!['('] => tt::DelimiterKind::Parenthesis,
        T!['{'] => tt::DelimiterKind::Brace,
        T!['['] => tt::DelimiterKind::Bracket,
        _ => tt::DelimiterKind::Invisible,
    };

    tt::Delimiter {
        open: span_factory.span_for(left.text_range()),
        close: span_factory.span_for(right.text_range()),
        kind,
    }
}

fn push_token(builder: &mut tt::TopSubtreeBuilder, token: &SyntaxToken, span_factory: SpanFactory) {
    let kind = token.kind();
    if kind.is_trivia() {
        return;
    }

    if kind == SyntaxKind::LIFETIME_IDENT {
        push_lifetime(builder, token, span_factory);
    } else if kind.is_any_identifier() || kind == T![_] {
        builder.push(tt::Leaf::Ident(tt::Ident::new(
            token.text(),
            span_factory.span_for(token.text_range()),
        )));
    } else if kind.is_literal() {
        builder.push(tt::Leaf::Literal(tt::token_to_literal(
            token.text(),
            span_factory.span_for(token.text_range()),
        )));
    } else if kind.is_punct() {
        push_punct_token(builder, token, span_factory);
    }
}

fn push_lifetime(
    builder: &mut tt::TopSubtreeBuilder,
    token: &SyntaxToken,
    span_factory: SpanFactory,
) {
    // rust-analyzer's token-tree format represents lifetimes as a joint
    // apostrophe punctuation followed by an identifier. Matching that shape keeps
    // lifetime fragments compatible with the vendored parser.
    let range = token.text_range();
    let apostrophe = TextRange::at(range.start(), TextSize::of('\''));
    builder.push(tt::Leaf::Punct(tt::Punct {
        char: '\'',
        spacing: tt::Spacing::Joint,
        span: span_factory.span_for(apostrophe),
    }));
    builder.push(tt::Leaf::Ident(tt::Ident {
        sym: Symbol::new(token.text().trim_start_matches('\'')),
        span: span_factory.span_for(TextRange::new(
            range.start() + TextSize::of('\''),
            range.end(),
        )),
        is_raw: tt::IdentIsRaw::No,
    }));
}

fn push_punct_token(
    builder: &mut tt::TopSubtreeBuilder,
    token: &SyntaxToken,
    span_factory: SpanFactory,
) {
    let mut offset = token.text_range().start();
    let chars = token.text().chars().collect::<Vec<_>>();
    for (index, char) in chars.iter().copied().enumerate() {
        let len = TextSize::of(char);
        let spacing = if index + 1 == chars.len() {
            tt::Spacing::Alone
        } else {
            tt::Spacing::Joint
        };
        builder.push(tt::Leaf::Punct(tt::Punct {
            char,
            spacing,
            span: span_factory.span_for(TextRange::at(offset, len)),
        }));
        offset += len;
    }
}

fn render_expansion(tokens: tt::TokenTreesView<'_>) -> String {
    let mut source = tt::pretty(tokens);

    // The vendored transcriber can lose joint spacing for punctuation in a few simple paths.
    // Keep this renderer text-based for now, but repair the multi-character punctuators that must
    // re-lex as one logical token for generated item syntax.
    for (from, to) in [
        (": :", "::"),
        ("= >", "=>"),
        ("- >", "->"),
        ("! =", "!="),
        ("= =", "=="),
        ("< =", "<="),
        ("> =", ">="),
        ("& &", "&&"),
        ("| |", "||"),
        (". . =", "..="),
        (". .", ".."),
        ("< <", "<<"),
        ("> >", ">>"),
    ] {
        source = source.replace(from, to);
    }

    source
}

#[cfg(test)]
mod tests {
    use expect_test::{Expect, expect};
    use rg_syntax::{AstNode as _, ast};

    use super::*;

    #[test]
    fn expands_simple_item_macro_to_text() {
        check_expansion(
            r#"
macro_rules! make_user {
    () => {
        pub struct User;
    };
}

make_user!();
"#,
            expect!["pub struct User ;"],
        );
    }

    #[test]
    fn expands_repetition_to_text() {
        check_expansion(
            r#"
macro_rules! make_fields {
    ($($name:ident),*) => {
        struct User {
            $($name: u32,)*
        }
    };
}

make_fields!(id, age);
"#,
            expect!["struct User {id : u32 , age : u32 ,}"],
        );
    }

    #[test]
    fn renders_joint_path_punctuation() {
        check_expansion(
            r#"
macro_rules! import_thing {
    () => {
        pub use source::Thing;
    };
}

import_thing!();
"#,
            expect!["pub use source :: Thing ;"],
        );
    }

    fn check_expansion(source: &str, expected: Expect) {
        let file = ast::SourceFile::parse(source, Edition::CURRENT)
            .ok()
            .expect("test source should parse");
        let macro_rules = file
            .syntax()
            .descendants()
            .find_map(ast::MacroRules::cast)
            .expect("test source should contain macro_rules");
        let call = file
            .syntax()
            .descendants()
            .filter_map(ast::MacroCall::cast)
            .last()
            .expect("test source should contain a macro call");

        let mac = DeclarativeMacro::from_macro_rules(&macro_rules, Edition::CURRENT, 0)
            .expect("macro should compile");
        let expanded = mac.expand_call(&call, 0).expect("macro should expand");

        expected.assert_eq(&expanded.source);
    }

    #[test]
    fn expands_from_stored_parts() {
        let mac = DeclarativeMacro::from_macro_rules_parts(
            "{ ($name:ident) => { struct $name; }; }",
            Edition::CURRENT,
            0,
        )
        .expect("macro should compile");
        let expanded = mac
            .expand_call_parts("make", "(Generated)", 0)
            .expect("macro should expand");

        assert_eq!(expanded.source, "struct Generated ;");
    }
}
