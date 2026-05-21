//! Declarative macro expansion for rust-glancer.
//!
//! This crate keeps the rust-analyzer-derived MBE engine behind a small API that
//! works with rust-glancer's syntax and def-map data. Callers pass parsed macro
//! nodes or stored token trees, then receive generated syntax parsed directly
//! from the expanded token tree.

extern crate ra_ap_rustc_lexer as rustc_lexer;

mod builtins;
mod mbe;

use anyhow::Context as _;
use rg_syntax::{AstNode as _, Parse, SyntaxNode, ast};
use rg_tt::{
    span::SyntaxContext,
    syntax_bridge::{
        ExpansionSpanMap, SpanFactory, syntax_node_to_token_tree, token_tree_to_syntax_node,
    },
};

pub use rg_tt::span::Edition;
pub use rg_tt::tt::TopSubtree;

pub use self::builtins::expand_cfg_select;

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
    /// Compiles a `macro_rules!` definition from a parsed syntax node.
    ///
    /// `file_id` anchors spans created for the vendored expander. The generated
    /// syntax keeps a span map, so callers can later map expanded tokens back to
    /// the definition or call site.
    pub fn from_macro_rules(
        item: &ast::MacroRules,
        edition: Edition,
        file_id: u32,
    ) -> anyhow::Result<Self> {
        let body = item
            .token_tree()
            .context("while attempting to fetch macro_rules body")?;
        let span_factory = SpanFactory::new(file_id, edition);
        let body = syntax_node_to_token_tree(&body, span_factory);
        let inner = mbe::DeclarativeMacro::parse_macro_rules(&body, move |ctx| ctx.edition());
        Ok(Self { inner, edition })
    }

    /// Compiles a `macro_rules!` definition from a stored token tree.
    pub fn from_macro_rules_tokens(body: &TopSubtree, edition: Edition) -> anyhow::Result<Self> {
        let inner = mbe::DeclarativeMacro::parse_macro_rules(body, move |ctx| ctx.edition());
        Ok(Self { inner, edition })
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
            .map(|args| syntax_node_to_token_tree(&args, span_factory));
        let body = item
            .body()
            .context("while attempting to fetch macro body")?;
        let body = syntax_node_to_token_tree(&body, span_factory);
        let inner =
            mbe::DeclarativeMacro::parse_macro2(args.as_ref(), &body, move |ctx| ctx.edition());
        Ok(Self { inner, edition })
    }

    /// Compiles a `macro` definition from stored token trees.
    pub fn from_macro_def_tokens(
        args: Option<&TopSubtree>,
        body: &TopSubtree,
        edition: Edition,
    ) -> anyhow::Result<Self> {
        let inner = mbe::DeclarativeMacro::parse_macro2(args, body, move |ctx| ctx.edition());
        Ok(Self { inner, edition })
    }

    /// Expands a parsed function-like macro call into parsed item-position syntax.
    pub fn expand_call(
        &self,
        call: &ast::MacroCall,
        file_id: u32,
    ) -> anyhow::Result<ExpansionSyntax> {
        let args = call
            .token_tree()
            .context("while attempting to fetch macro call arguments")?;
        let span_factory = SpanFactory::new(file_id, self.edition);
        let call_site = span_factory.span_for(call.syntax().text_range());
        let args = syntax_node_to_token_tree(&args, span_factory);
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

        Ok(ExpansionSyntax::from_token_tree(expanded.value.0))
    }

    /// Expands a function-like macro call from stored argument token trees.
    pub fn expand_call_tokens(
        &self,
        args: &TopSubtree,
        call_site: rg_tt::Span,
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

        Ok(ExpansionSyntax::from_token_tree(expanded.value.0))
    }
}

/// Parsed syntax produced by a successful declarative macro expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionSyntax {
    /// Generated item-position syntax parsed directly from the expanded token tree.
    pub parse: Parse<SyntaxNode>,
    /// Offset-to-span map for generated tokens.
    pub span_map: ExpansionSpanMap,
}

impl ExpansionSyntax {
    /// Parses item-position syntax directly from an expanded token tree.
    pub fn from_token_tree(token_tree: TopSubtree) -> Self {
        let mut span_to_edition = |ctx: SyntaxContext| ctx.edition();
        let (parse, span_map) = token_tree_to_syntax_node(
            &token_tree,
            parser::TopEntryPoint::MacroItems,
            &mut span_to_edition,
        );
        Self { parse, span_map }
    }
}

#[cfg(test)]
mod tests {
    use expect_test::{Expect, expect};
    use rg_syntax::{AstNode as _, ast};

    use super::*;

    #[test]
    fn expands_simple_item_macro_to_syntax() {
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

        let macro_rules = stored_macro_rules_body(&macro_rules);
        let (args, call_site) = stored_call_args(&call);
        let mac = DeclarativeMacro::from_macro_rules_tokens(&macro_rules, Edition::CURRENT)
            .expect("macro should compile");
        let expanded = mac
            .expand_call_tokens(&args, call_site)
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

        let macro_rules = stored_macro_rules_body(&macro_rules);
        let (args, call_site) = stored_call_args(&call);
        let mac = DeclarativeMacro::from_macro_rules_tokens(&macro_rules, Edition::CURRENT)
            .expect("macro should compile");
        let expanded = mac
            .expand_call_tokens(&args, call_site)
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

        let macro_rules = stored_macro_rules_body(&macro_rules);
        let (args, call_site) = stored_call_args(&call);
        let mac = DeclarativeMacro::from_macro_rules_tokens(&macro_rules, Edition::CURRENT)
            .expect("macro should compile");
        let expanded = mac
            .expand_call_tokens(&args, call_site)
            .expect("macro should expand");

        assert_eq!(
            expanded.parse.syntax_node().text().to_string(),
            "pub use $crate :: source :: Thing ;"
        );
    }

    #[test]
    fn expands_from_stored_token_trees() {
        let file = ast::SourceFile::parse(
            r#"
macro_rules! make {
    ($name:ident) => { struct $name; };
}

make!(Generated);
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

        let macro_rules = stored_macro_rules_body(&macro_rules);
        let (args, call_site) = stored_call_args(&call);
        let mac = DeclarativeMacro::from_macro_rules_tokens(&macro_rules, Edition::CURRENT)
            .expect("macro should compile");
        let expanded = mac
            .expand_call_tokens(&args, call_site)
            .expect("macro should expand");

        assert_eq!(
            expanded.parse.syntax_node().text().to_string(),
            "struct Generated ;"
        );
    }

    fn stored_macro_rules_body(macro_rules: &ast::MacroRules) -> TopSubtree {
        let body = macro_rules
            .token_tree()
            .expect("macro_rules fixture should have a body");
        syntax_node_to_token_tree(&body, SpanFactory::new(0, Edition::CURRENT))
    }

    fn stored_call_args(call: &ast::MacroCall) -> (TopSubtree, rg_tt::Span) {
        let span_factory = SpanFactory::new(0, Edition::CURRENT);
        let call_site = span_factory.span_for(call.syntax().text_range());
        let args = call
            .token_tree()
            .expect("macro call fixture should have arguments");
        (syntax_node_to_token_tree(&args, span_factory), call_site)
    }
}
