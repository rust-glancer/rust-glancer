//! Declarative macro expansion for rust-glancer.
//!
//! This crate keeps the rust-analyzer-derived MBE engine behind a small API that
//! works with rust-glancer's syntax and def-map data. Callers pass token trees
//! and receive generated syntax parsed directly from the expanded token tree.

extern crate ra_ap_rustc_lexer as rustc_lexer;

mod builtins;
mod mbe;

use std::sync::Arc;

use rg_syntax::{Parse, SyntaxNode};
use rg_tt::{
    TokenOrigins,
    span::SyntaxContext,
    syntax_bridge::{ExpansionSpanMap, token_tree_to_syntax_node},
    tt::{Delimiter, DelimiterKind, Ident, Leaf, Punct, Spacing, TopSubtree, TopSubtreeBuilder},
};

pub use self::builtins::{CfgSelect, CfgSelectArm};

/// Parser entry point used for a successful macro expansion.
///
/// Def-map expands macros as items, while Body IR needs the same token
/// expansion parsed as statements, expressions, patterns, or types before it
/// can splice generated syntax into body lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpansionParseKind {
    Items,
    Statements,
    Expr,
    Pattern,
    Type,
}

impl ExpansionParseKind {
    fn top_entry_point(self) -> parser::TopEntryPoint {
        match self {
            Self::Items => parser::TopEntryPoint::MacroItems,
            Self::Statements => parser::TopEntryPoint::MacroStmts,
            Self::Expr => parser::TopEntryPoint::Expr,
            Self::Pattern => parser::TopEntryPoint::Pattern,
            Self::Type => parser::TopEntryPoint::Type,
        }
    }
}

/// Compiled declarative macro ready to expand function-like calls.
///
/// The inner matcher/transcriber comes from the vendored MBE engine. This wrapper
/// compiles token trees using the edition carried by each token.
#[derive(Debug, Clone)]
pub struct DeclarativeMacro {
    inner: self::mbe::DeclarativeMacro,
}

impl DeclarativeMacro {
    /// Compiles a `macro_rules!` definition from a stored token tree.
    pub fn from_macro_rules_tokens(body: &TopSubtree) -> Self {
        let inner = mbe::DeclarativeMacro::parse_macro_rules(body, SyntaxContext::edition);
        Self { inner }
    }

    /// Compiles a `macro` definition from stored token trees.
    pub fn from_macro_def_tokens(args: Option<&TopSubtree>, body: &TopSubtree) -> Self {
        let inner = mbe::DeclarativeMacro::parse_macro2(args, body, SyntaxContext::edition);
        Self { inner }
    }

    /// Bind template `$crate` once, before forwarding can move those tokens through
    /// another macro's input.
    pub fn bind_dollar_crate(&mut self, package: u32, target: u32) {
        self.inner.bind_dollar_crate(package, target);
    }

    /// Expands stored function-like call arguments into the requested syntax kind.
    pub fn expand_call_tokens(
        &self,
        args: &TopSubtree,
        call_site: rg_tt::Span,
        parse_kind: ExpansionParseKind,
    ) -> anyhow::Result<ExpansionSyntax> {
        let expanded = self.inner.expand(
            args,
            |_| {},
            mbe::MacroCallStyle::FnLike,
            call_site,
            move |ctx| ctx.edition(),
        );

        if let Some(err) = expanded.err {
            anyhow::bail!("macro expansion failed: {err}");
        }

        let transcription = expanded.value.0;
        let mut expansion = ExpansionSyntax::from_token_tree(transcription.tokens, parse_kind);
        expansion.transformed_inputs = transcription.transformed_inputs;
        Ok(expansion)
    }
}

/// Parsed syntax produced by a successful declarative macro expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionSyntax {
    /// Generated syntax parsed directly from the expanded token tree.
    pub parse: Parse<SyntaxNode>,
    /// Offset-to-span map for generated tokens.
    pub span_map: ExpansionSpanMap,
    /// Item bodies can be opened later from these tokens, with jointness and origins intact.
    pub item_tokens: Option<Arc<TopSubtree>>,
    /// Inputs that influenced output text without being copied, as in `${concat(..)}`.
    pub transformed_inputs: Vec<rg_tt::Span>,
}

impl ExpansionSyntax {
    /// Parse builtin input as a leading expression followed by positional or named arguments.
    ///
    /// Both `format_args!(template, values...)` and `assert!(condition, message...)` fit the
    /// parser's `builtin#format_args` grammar. Wrapping tokens keeps their original source spans,
    /// including expressions forwarded through declarative macros.
    pub fn from_builtin_arguments(args: &TopSubtree) -> Self {
        let delimiter = args.top_subtree().delimiter;
        let mut generated = delimiter.open;
        // Wrapper names are parser scaffolding. Mark their origins as synthetic so the body
        // source map cannot mistake them for copied input tokens.
        generated.origins = TokenOrigins::synthetic(generated.origins.first());
        let mut builder = TopSubtreeBuilder::new(Delimiter::invisible_spanned(generated));
        builder.push(Leaf::Ident(Ident::new("builtin", generated)));
        builder.push(Leaf::Punct(Punct {
            char: '#',
            spacing: Spacing::Alone,
            span: generated,
        }));
        builder.push(Leaf::Ident(Ident::new("format_args", generated)));
        builder.open(DelimiterKind::Parenthesis, delimiter.open);
        builder.extend_with_tt(args.view().token_trees());
        builder.close(delimiter.close);
        Self::from_token_tree(builder.build(), ExpansionParseKind::Expr)
    }

    /// Parses an expanded token tree using the requested syntax entry point.
    pub fn from_token_tree(token_tree: TopSubtree, parse_kind: ExpansionParseKind) -> Self {
        let (parse, span_map) = Self::parse_token_tree(&token_tree, parse_kind);
        Self {
            parse,
            span_map,
            item_tokens: (parse_kind == ExpansionParseKind::Items).then(|| Arc::new(token_tree)),
            transformed_inputs: Vec::new(),
        }
    }

    pub fn parse_token_tree(
        tokens: &TopSubtree,
        parse_kind: ExpansionParseKind,
    ) -> (Parse<SyntaxNode>, ExpansionSpanMap) {
        token_tree_to_syntax_node(
            tokens,
            parse_kind.top_entry_point(),
            &mut |ctx: SyntaxContext| ctx.edition(),
        )
    }
}

#[cfg(test)]
mod tests {
    use expect_test::{Expect, expect};
    use rg_syntax::{AstNode as _, ast};
    use rg_tt::{
        TokenOwner,
        span::Edition,
        syntax_bridge::{SpanFactory, TokenSourceMap, syntax_node_to_token_tree},
    };

    use super::*;

    #[test]
    fn nested_expansion_preserves_each_tokens_package_and_file() {
        use rg_syntax::{TextRange, TextSize};
        use rg_tt::syntax_bridge::syntax_node_to_token_tree_with_span;

        let outer_source = "macro_rules! outer { ($value:expr) => { inner!($value, Template) }; }";
        let inner_source =
            "macro_rules! inner { ($value:expr, $name:ident) => { ($value, $name, Helper) }; }";
        let [(outer, outer_sources), (inner, inner_sources)] =
            [(1, outer_source), (2, inner_source)].map(|(package, source)| {
                let file = ast::SourceFile::parse(source, Edition::CURRENT).tree();
                let definition = file
                    .syntax()
                    .descendants()
                    .find_map(ast::MacroRules::cast)
                    .expect("fixture contains a macro definition");
                let mut sources = TokenSourceMap::default();
                let mac = compile_macro_rules(&definition, package, 7, &mut sources);
                (mac, sources)
            });
        let caller_source = "outer!(input);";
        let file = ast::SourceFile::parse(caller_source, Edition::CURRENT).tree();
        let call = file
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .expect("fixture contains a macro call");
        let mut caller_sources = TokenSourceMap::default();
        let (args, call_site) = stored_call_args(&call, 0, 7, &mut caller_sources);
        let expanded = outer
            .expand_call_tokens(&args, call_site, ExpansionParseKind::Expr)
            .expect("outer macro should expand");
        let nested = expanded
            .parse
            .syntax_node()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .expect("outer expansion contains a nested call");
        let args = nested.token_tree().expect("nested call has arguments");
        let args = syntax_node_to_token_tree_with_span(&args, &mut |range| {
            expanded
                .span_map
                .span_for_range(range)
                .expect("expanded tokens have source spans")
        });
        let expanded = inner
            .expand_call_tokens(&args, call_site, ExpansionParseKind::Expr)
            .expect("inner macro should expand");

        // All three files use slot 7. The output still identifies the original file for each token.
        for (name, package, source) in [
            ("input", 0, caller_source),
            ("Template", 1, outer_source),
            ("Helper", 2, inner_source),
        ] {
            let token = expanded
                .parse
                .syntax_node()
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| token.text() == name)
                .expect("expanded tuple contains the expected token");
            let start = u32::try_from(source.find(name).expect("source contains the token"))
                .expect("fixture offset fits u32");
            let expected = TextRange::at(TextSize::new(start), TextSize::new(name.len() as u32));
            let origin = expanded
                .span_map
                .span_for_range(token.text_range())
                .expect("expanded token has an origin");
            assert_eq!(origin.anchor.package, package, "origin of {name}");
            assert_eq!(origin.anchor.file_id.raw_file_id(), 7);
            let sources = match package {
                0 => &caller_sources,
                1 => &outer_sources,
                2 => &inner_sources,
                _ => unreachable!(),
            };
            assert_eq!(
                sources.resolve(origin.origins),
                Some(expected),
                "source of {name}"
            );
        }
    }

    #[test]
    fn preserves_doc_attributes_and_their_spans_through_item_expansion() {
        use rg_syntax::{AstToken as _, ast::HasAttrs as _};

        let source = r##"
macro_rules! forward { ($item:item) => { $item }; }
forward! {
    /// Outer "quoted" café.
    #[doc = "Explicit attribute."]
    pub mod generated {
        /*! Inner block docs. */
        pub struct Item;
    }
}
"##;
        let file = ast::SourceFile::parse(source, Edition::CURRENT).tree();
        let definition = file
            .syntax()
            .descendants()
            .find_map(ast::MacroRules::cast)
            .expect("fixture contains macro definition");
        let call = file
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .expect("fixture contains macro call");
        let mut sources = TokenSourceMap::default();
        let mac = compile_macro_rules(&definition, 0, 0, &mut sources);
        let (args, call_site) = stored_call_args(&call, 0, 0, &mut sources);
        let expansion = mac
            .expand_call_tokens(&args, call_site, ExpansionParseKind::Items)
            .expect("documented item should expand");
        assert!(expansion.parse.errors().is_empty());
        let module = expansion
            .parse
            .syntax_node()
            .descendants()
            .find_map(ast::Module::cast)
            .expect("expanded module should exist");
        assert_eq!(module.attrs().count(), 2, "outer docs stay on the module");
        let strings = expansion
            .parse
            .syntax_node()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter_map(ast::String::cast)
            .collect::<Vec<_>>();
        let values = strings
            .iter()
            .map(|string| string.value().expect("valid doc literal").into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [
                " Outer \"quoted\" café.",
                "Explicit attribute.",
                " Inner block docs. "
            ]
        );
        let span = expansion
            .span_map
            .span_for_range(strings[0].syntax().text_range())
            .expect("generated doc attribute should map to its comment");
        assert_eq!(
            &source[sources
                .resolve(span.origins)
                .expect("retained token source")],
            "/// Outer \"quoted\" café."
        );
    }

    #[test]
    fn doc_comments_match_raw_attributes() {
        check_expansion(
            r####"
macro_rules! documented {
    (#[doc = r###" Path C:\docs includes "quotes"##."###]) => {
        struct Matched;
    };
}

documented! {
    /// Path C:\docs includes "quotes"##.
}
"####,
            expect!["struct Matched ;"],
        );
    }

    #[test]
    fn recovers_exponent_field_access_in_macro_output() {
        for expression in ["value.1e3", "{ value.1e3; value.0.1 }"] {
            let source = format!(
                "macro_rules! identity {{ ($($tokens:tt)*) => {{ $($tokens)* }}; }}\n\
                 identity!({expression});"
            );
            let file = ast::SourceFile::parse(&source, Edition::CURRENT)
                .ok()
                .expect("fixture macro invocation should parse");
            let definition = file
                .syntax()
                .descendants()
                .find_map(ast::MacroRules::cast)
                .expect("fixture contains macro definition");
            let call = file
                .syntax()
                .descendants()
                .find_map(ast::MacroCall::cast)
                .expect("fixture contains macro call");
            let mut sources = TokenSourceMap::default();
            let mac = compile_macro_rules(&definition, 0, 0, &mut sources);
            let (args, call_site) = stored_call_args(&call, 0, 0, &mut sources);
            let expanded = mac
                .expand_call_tokens(&args, call_site, ExpansionParseKind::Expr)
                .expect("identity macro should retain malformed expression tokens");

            assert!(
                expanded
                    .parse
                    .errors()
                    .iter()
                    .any(|error| error.to_string() == "illegal float literal"),
                "expected a syntax diagnostic for {expression}",
            );
            let literal = expanded
                .parse
                .syntax_node()
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| token.kind() == rg_syntax::SyntaxKind::FLOAT_NUMBER)
                .expect("recovery should retain the float token");
            assert_eq!(literal.text(), "1e3", "{expression}");
            let span = expanded
                .span_map
                .span_for_range(literal.text_range())
                .expect("recovered token should map to macro input");
            assert_eq!(
                &source[sources
                    .resolve(span.origins)
                    .expect("retained token source")],
                "1e3",
                "{expression}"
            );
        }
    }

    #[test]
    fn expands_simple_item_macro_to_syntax() {
        check_expansion(
            r#"
macro_rules! make_item {
    ($name:ident) => {
        pub struct $name;
    };
}

make_item!(User);
"#,
            expect!["pub struct User ;"],
        );
    }

    #[test]
    fn expands_statement_macro_to_syntax() {
        check_expansion_as(
            r#"
macro_rules! make_statements {
    () => {
        let value = 1;
        value
    };
}

make_statements!();
"#,
            ExpansionParseKind::Statements,
            expect!["let value = 1 ;value"],
        );
    }

    #[test]
    fn expands_expression_macro_to_syntax() {
        check_expansion_as(
            r#"
macro_rules! make_expr {
    () => {
        1 + 2
    };
}

make_expr!();
"#,
            ExpansionParseKind::Expr,
            expect!["1 + 2"],
        );
    }

    #[test]
    fn expands_pattern_macro_to_syntax() {
        check_expansion_as(
            r#"
macro_rules! make_pat {
    () => {
        Some(value)
    };
}

make_pat!();
"#,
            ExpansionParseKind::Pattern,
            expect!["Some(value)"],
        );
    }

    #[test]
    fn expands_type_macro_to_syntax() {
        check_expansion_as(
            r#"
macro_rules! make_type {
    () => {
        Option<User>
    };
}

make_type!();
"#,
            ExpansionParseKind::Type,
            expect!["Option < User >"],
        );
    }

    #[test]
    fn expands_repetition_to_syntax() {
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
            expect!["struct User{id : u32 ,age : u32 ,}"],
        );
    }

    #[test]
    fn matcher_error_does_not_transcribe_partial_repetition() {
        let file = ast::SourceFile::parse(
            r#"
macro_rules! expand_params {
    (
        $($code:literal),+,
        $(param: $param:expr,)?
    ) => {{
        let _ = expand_params!(@param $($param)*);
    }};
    (@param) => { () };
    (@param $param:expr) => { $param };
}

expand_params!("RAND", param: value.clone());
"#,
            Edition::CURRENT,
        )
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

        let mac = compile_macro_rules(&macro_rules, 0, 0, &mut TokenSourceMap::default());
        let (args, call_site) = stored_call_args(&call, 0, 0, &mut TokenSourceMap::default());
        let error = mac
            .expand_call_tokens(&args, call_site, ExpansionParseKind::Expr)
            .expect_err("invalid macro input should fail to expand");

        assert!(
            error.to_string().contains("expected literal"),
            "unexpected expansion error: {error:#}",
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

    #[test]
    fn keeps_punctuation_inside_literals_untouched() {
        check_expansion(
            r#"
macro_rules! make_const {
    () => {
        const TEXT: &str = "a : : b";
    };
}

make_const!();
"#,
            expect!["const TEXT : & str = \"a : : b\" ;"],
        );
    }

    #[test]
    fn generated_dollar_crate_macro_call_keeps_full_path() {
        let file = ast::SourceFile::parse(
            r#"
macro_rules! outer {
    () => {
        $crate::inner!();
    };
}

outer!();
"#,
            Edition::CURRENT,
        )
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

        let mac = compile_macro_rules(&macro_rules, 0, 0, &mut TokenSourceMap::default());
        let (args, call_site) = stored_call_args(&call, 0, 0, &mut TokenSourceMap::default());
        let expanded = mac
            .expand_call_tokens(&args, call_site, ExpansionParseKind::Items)
            .expect("macro should expand");
        let generated_call = expanded
            .parse
            .syntax_node()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .expect("expansion should contain a macro call");

        let path = generated_call
            .path()
            .expect("generated call should have a path")
            .syntax()
            .text()
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        assert_eq!(path, "$crate :: inner");
    }

    fn check_expansion(source: &str, expected: Expect) {
        check_expansion_as(source, ExpansionParseKind::Items, expected);
    }

    fn check_expansion_as(source: &str, parse_kind: ExpansionParseKind, expected: Expect) {
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

        let mac = compile_macro_rules(&macro_rules, 0, 0, &mut TokenSourceMap::default());
        let (args, call_site) = stored_call_args(&call, 0, 0, &mut TokenSourceMap::default());
        let expanded = mac
            .expand_call_tokens(&args, call_site, parse_kind)
            .expect("macro should expand");

        expected.assert_eq(&expanded.parse.syntax_node().text().to_string());
    }

    #[test]
    fn preserves_dollar_crate_in_generated_syntax() {
        let file = ast::SourceFile::parse(
            r#"
macro_rules! import_thing {
    () => {
        pub use $crate::source::Thing;
    };
}

import_thing!();
"#,
            Edition::CURRENT,
        )
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

        let mac = compile_macro_rules(&macro_rules, 0, 0, &mut TokenSourceMap::default());
        let (args, call_site) = stored_call_args(&call, 0, 0, &mut TokenSourceMap::default());
        let expanded = mac
            .expand_call_tokens(&args, call_site, ExpansionParseKind::Items)
            .expect("macro should expand");

        assert_eq!(
            expanded.parse.syntax_node().text().to_string(),
            "pub use $crate :: source :: Thing ;"
        );
    }

    fn compile_macro_rules(
        definition: &ast::MacroRules,
        package: u32,
        file: u32,
        sources: &mut TokenSourceMap,
    ) -> DeclarativeMacro {
        let body = definition
            .token_tree()
            .expect("macro_rules fixture should have a body");
        let factory = SpanFactory::new(package, file, TokenOwner::Declaration(0), Edition::CURRENT);
        let body = syntax_node_to_token_tree(&body, factory, sources);
        DeclarativeMacro::from_macro_rules_tokens(&body)
    }

    fn stored_call_args(
        call: &ast::MacroCall,
        package: u32,
        file: u32,
        sources: &mut TokenSourceMap,
    ) -> (TopSubtree, rg_tt::Span) {
        let span_factory = SpanFactory::new(package, file, TokenOwner::Body, Edition::CURRENT);
        let call_site = sources.record(span_factory, call.syntax().text_range());
        let args = call
            .token_tree()
            .expect("macro call fixture should have arguments");
        (
            syntax_node_to_token_tree(&args, span_factory, sources),
            call_site,
        )
    }
}
