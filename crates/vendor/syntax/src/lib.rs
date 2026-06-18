//! Immutable Rust syntax trees and typed AST accessors.
//!
//! rust-glancer parses full source files, traverses their concrete syntax while lowering, and then
//! drops the syntax trees before steady-state queries. The supported API is therefore intentionally
//! read-only and non-incremental: parsing, validation diagnostics, source ranges, token text, and
//! typed AST traversal.

#![cfg_attr(feature = "in-rust-tree", feature(rustc_private))]
#![allow(
    clippy::collapsible_match,
    clippy::iter_kv_map,
    clippy::mutable_key_type,
    clippy::too_many_arguments,
    clippy::vec_init_then_push
)]

#[cfg(feature = "in-rust-tree")]
extern crate rustc_driver as _;

mod parsing;
mod syntax_error;
mod syntax_node;
#[cfg(test)]
mod tests;
mod token_text;
mod validation;

pub mod algo;
pub mod ast;
#[doc(hidden)]
pub mod fuzz;
pub mod hacks;
pub mod utils;

use std::marker::PhantomData;
use std::sync::Arc;

use stdx::format_to;

pub use crate::{
    ast::{AstNode, AstToken},
    syntax_error::SyntaxError,
    syntax_node::{
        Direction, NodeOrToken, Preorder, PreorderWithTokens, RustLanguage, SyntaxElement,
        SyntaxElementChildren, SyntaxNode, SyntaxNodeChildren, SyntaxText, SyntaxToken,
        SyntaxTreeMemoryUsage, TextRange, TextSize, TokenAtOffset, WalkEvent,
    },
    token_text::TokenText,
};
pub use parser::{Edition, SyntaxKind, T};
pub use rustc_literal_escaper as unescape;
pub use smol_str::{SmolStr, SmolStrBuilder, ToSmolStr, format_smolstr};

/// Builder used by token-tree parsers that already know the parser input and token text.
///
/// Normal Rust files should still go through `SourceFile::parse`. This exists for macro
/// expansions, where the parser consumes token trees instead of lexing source text.
#[doc(hidden)]
pub struct GeneratedSyntaxBuilder {
    inner: syntax_node::SyntaxTreeBuilder,
}

impl GeneratedSyntaxBuilder {
    pub fn new() -> Self {
        Self {
            inner: syntax_node::SyntaxTreeBuilder::generated(),
        }
    }

    pub fn current_offset(&self) -> TextSize {
        self.inner.current_offset()
    }

    pub fn token(&mut self, kind: SyntaxKind, text: &str) {
        self.inner.generated_token(kind, text);
    }

    pub fn start_node(&mut self, kind: SyntaxKind) {
        self.inner.start_node(kind);
    }

    pub fn finish_node(&mut self) {
        self.inner.finish_node();
    }

    pub fn error(&mut self, error: String) {
        self.inner.error(error, self.current_offset());
    }

    pub fn finish(self) -> Parse<SyntaxNode> {
        Parse::new(self.inner.finish())
    }
}

impl Default for GeneratedSyntaxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// `Parse` is the result of the parsing: a syntax tree and a collection of
/// errors.
///
/// Note that we always produce a syntax tree, even for completely invalid
/// files.
#[derive(Debug, PartialEq, Eq)]
pub struct Parse<T> {
    tree: Arc<syntax_node::SyntaxTree>,
    _ty: PhantomData<fn() -> T>,
}

impl<T> Clone for Parse<T> {
    fn clone(&self) -> Parse<T> {
        Parse {
            tree: self.tree.clone(),
            _ty: PhantomData,
        }
    }
}

impl<T> Parse<T> {
    fn new(tree: Arc<syntax_node::SyntaxTree>) -> Parse<T> {
        Parse {
            tree,
            _ty: PhantomData,
        }
    }

    pub fn syntax_node(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.tree.clone())
    }

    pub fn errors(&self) -> Vec<SyntaxError> {
        let mut errors = self.syntax_node().parse_errors().to_vec();
        validation::validate(&self.syntax_node(), &mut errors);
        errors
    }

    /// Returns approximate retained storage for the syntax tree behind this parse result.
    ///
    /// The returned value describes the whole immutable tree behind this parse. `Parse` is
    /// cloneable, and typed/untyped parse handles can share the same tree, so callers must account
    /// for this at most once for each underlying tree in one aggregate memory report.
    pub fn retained_tree_memory_usage(&self) -> SyntaxTreeMemoryUsage {
        self.tree.memory_usage()
    }
}

impl<T: AstNode> Parse<T> {
    /// Converts this parse result into a parse result for an untyped syntax tree.
    pub fn to_syntax(self) -> Parse<SyntaxNode> {
        Parse {
            tree: self.tree,
            _ty: PhantomData,
        }
    }

    /// Gets the parsed syntax tree as a typed ast node.
    ///
    /// # Panics
    ///
    /// Panics if the root node cannot be casted into the typed ast node
    /// (e.g. if it's an `ERROR` node).
    pub fn tree(&self) -> T {
        T::cast(self.syntax_node()).unwrap()
    }

    /// Converts from `Parse<T>` to [`Result<T, Vec<SyntaxError>>`].
    pub fn ok(self) -> Result<T, Vec<SyntaxError>> {
        match self.errors() {
            errors if !errors.is_empty() => Err(errors),
            _ => Ok(self.tree()),
        }
    }
}

impl Parse<SyntaxNode> {
    pub fn cast<N: AstNode>(self) -> Option<Parse<N>> {
        if N::cast(self.syntax_node()).is_some() {
            Some(Parse {
                tree: self.tree,
                _ty: PhantomData,
            })
        } else {
            None
        }
    }
}

impl Parse<SourceFile> {
    pub fn debug_dump(&self) -> String {
        let mut buf = format!("{:#?}", self.tree().syntax());
        for err in self.errors() {
            format_to!(buf, "error {:?}: {}\n", err.range(), err);
        }
        buf
    }
}

impl ast::Expr {
    /// Parses an `ast::Expr` from `text`.
    ///
    /// Note that if the parsed root node is not a valid expression, [`Parse::tree`] will panic.
    /// For example:
    /// ```rust,should_panic
    /// # use syntax::{ast, Edition};
    /// ast::Expr::parse("let fail = true;", Edition::CURRENT).tree();
    /// ```
    pub fn parse(text: &str, edition: Edition) -> Parse<ast::Expr> {
        let _p = tracing::info_span!("Expr::parse").entered();
        let tree = parsing::parse_text_at(text, parser::TopEntryPoint::Expr, edition);
        let root = SyntaxNode::new_root(tree.clone());

        assert!(
            ast::Expr::can_cast(root.kind()) || root.kind() == SyntaxKind::ERROR,
            "{:?} isn't an expression",
            root.kind()
        );
        Parse::new(tree)
    }
}

/// `SourceFile` represents a parse tree for a single Rust file.
pub use crate::ast::SourceFile;

impl SourceFile {
    pub fn parse(text: &str, edition: Edition) -> Parse<SourceFile> {
        let _p = tracing::info_span!("SourceFile::parse").entered();
        let tree = parsing::parse_text(text, edition);
        let root = SyntaxNode::new_root(tree.clone());

        assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);
        Parse::new(tree)
    }
}

/// Matches a `SyntaxNode` against an `ast` type.
///
/// # Example:
///
/// ```ignore
/// match_ast! {
///     match node {
///         ast::CallExpr(it) => { ... },
///         ast::MethodCallExpr(it) => { ... },
///         ast::MacroCall(it) => { ... },
///         _ => None,
///     }
/// }
/// ```
#[macro_export]
macro_rules! match_ast {
    (match $node:ident { $($tt:tt)* }) => { $crate::match_ast!(match ($node) { $($tt)* }) };

    (match ($node:expr) {
        $( $( $path:ident )::+ ($it:pat) => $res:expr, )*
        _ => $catch_all:expr $(,)?
    }) => {{
        $( if let Some($it) = $($path::)+cast($node.clone()) { $res } else )*
        { $catch_all }
    }};
}

/// This test does not assert anything and instead just shows off the crate's
/// API.
#[test]
fn api_walkthrough() {
    use ast::{HasModuleItem, HasName};

    let source_code = "
        fn foo() {
            1 + 1
        }
    ";
    // `SourceFile` is the main entry point.
    //
    // The `parse` method returns a `Parse` -- a pair of syntax tree and a list
    // of errors. That is, syntax tree is constructed even in presence of errors.
    let parse = SourceFile::parse(source_code, parser::Edition::CURRENT);
    assert!(parse.errors().is_empty());

    // The `tree` method returns an owned syntax node of type `SourceFile`.
    // Owned nodes are cheap: inside, they are `Rc` handles to the underlying data.
    let file: SourceFile = parse.tree();

    // `SourceFile` is the root of the syntax tree. We can iterate file's items.
    // Let's fetch the `foo` function.
    let mut func = None;
    for item in file.items() {
        match item {
            ast::Item::Fn(f) => func = Some(f),
            _ => unreachable!(),
        }
    }
    let func: ast::Fn = func.unwrap();

    // Each AST node has a bunch of getters for children. All getters return
    // `Option`s though, to account for incomplete code. Some getters are common
    // for several kinds of node. In this case, a trait like `ast::NameOwner`
    // usually exists. By convention, all ast types should be used with `ast::`
    // qualifier.
    let name: Option<ast::Name> = func.name();
    let name = name.unwrap();
    assert_eq!(name.text(), "foo");

    // Let's get the `1 + 1` expression!
    let body: ast::BlockExpr = func.body().unwrap();
    let stmt_list: ast::StmtList = body.stmt_list().unwrap();
    let expr: ast::Expr = stmt_list.tail_expr().unwrap();

    // Enums are used to group related ast nodes together, and can be used for
    // matching. However, because there are no public fields, it's possible to
    // match only the top level enum: that is the price we pay for increased API
    // flexibility
    let bin_expr: &ast::BinExpr = match &expr {
        ast::Expr::BinExpr(e) => e,
        _ => unreachable!(),
    };

    // Besides the "typed" AST API, there's an untyped CST one as well.
    // To switch from AST to CST, call `.syntax()` method:
    let expr_syntax: &SyntaxNode = expr.syntax();

    // Note how `expr` and `bin_expr` are in fact the same node underneath:
    assert!(expr_syntax == bin_expr.syntax());

    // To go from CST to AST, `AstNode::cast` function is used:
    let _expr: ast::Expr = match ast::Expr::cast(expr_syntax.clone()) {
        Some(e) => e,
        None => unreachable!(),
    };

    // The two properties each syntax node has is a `SyntaxKind`:
    assert_eq!(expr_syntax.kind(), SyntaxKind::BIN_EXPR);

    // And text range:
    assert_eq!(
        expr_syntax.text_range(),
        TextRange::new(32.into(), 37.into())
    );

    // You can get node's text as a `SyntaxText` object, which will traverse the
    // tree collecting token's text:
    let text: SyntaxText = expr_syntax.text();
    assert_eq!(text.to_string(), "1 + 1");

    // There's a bunch of traversal methods on `SyntaxNode`:
    assert_eq!(expr_syntax.parent().as_ref(), Some(stmt_list.syntax()));
    assert_eq!(
        stmt_list
            .syntax()
            .first_child_or_token()
            .map(|it| it.kind()),
        Some(T!['{'])
    );
    assert_eq!(
        expr_syntax.next_sibling_or_token().map(|it| it.kind()),
        Some(SyntaxKind::WHITESPACE)
    );

    // As well as some iterator helpers:
    let f = expr_syntax.ancestors().find_map(ast::Fn::cast);
    assert_eq!(f, Some(func));
    assert!(
        expr_syntax
            .siblings_with_tokens(Direction::Next)
            .any(|it| it.kind() == T!['}'])
    );
    assert_eq!(
        expr_syntax.descendants_with_tokens().count(),
        8, // 5 tokens `1`, ` `, `+`, ` `, `1`
           // 2 child literal expressions: `1`, `1`
           // 1 the node itself: `1 + 1`
    );

    // There's also a `preorder` method with a more fine-grained iteration control:
    let mut buf = String::new();
    let mut indent = 0;
    for event in expr_syntax.preorder_with_tokens() {
        match event {
            WalkEvent::Enter(node) => {
                let text = match &node {
                    NodeOrToken::Node(it) => it.text().to_string(),
                    NodeOrToken::Token(it) => it.text().to_owned(),
                };
                format_to!(
                    buf,
                    "{:indent$}{:?} {:?}\n",
                    " ",
                    text,
                    node.kind(),
                    indent = indent
                );
                indent += 2;
            }
            WalkEvent::Leave(_) => indent -= 2,
        }
    }
    assert_eq!(indent, 0);
    assert_eq!(
        buf.trim(),
        r#"
"1 + 1" BIN_EXPR
  "1" LITERAL
    "1" INT_NUMBER
  " " WHITESPACE
  "+" PLUS
  " " WHITESPACE
  "1" LITERAL
    "1" INT_NUMBER
"#
        .trim()
    );

    // To recursively process the tree, there are three approaches:
    // 1. explicitly call getter methods on AST nodes.
    // 2. use descendants and `AstNode::cast`.
    // 3. use descendants and `match_ast!`.
    //
    // Here's how the first one looks like:
    let exprs_cast: Vec<String> = file
        .syntax()
        .descendants()
        .filter_map(ast::Expr::cast)
        .map(|expr| expr.syntax().text().to_string())
        .collect();

    // An alternative is to use a macro.
    let mut exprs_visit = Vec::new();
    for node in file.syntax().descendants() {
        match_ast! {
            match node {
                ast::Expr(it) => {
                    let res = it.syntax().text().to_string();
                    exprs_visit.push(res);
                },
                _ => (),
            }
        }
    }
    assert_eq!(exprs_cast, exprs_visit);
}

#[cfg(test)]
mod memory_usage_tests {
    use crate::{Edition, SourceFile};

    #[test]
    fn reports_retained_parse_tree_memory_usage() {
        let parse = SourceFile::parse(
            r#"
            struct User {
                name: String,
            }
            "#,
            Edition::CURRENT,
        );
        assert!(
            parse.errors().is_empty(),
            "test source should parse as a source file"
        );

        let usage = parse.retained_tree_memory_usage();

        assert!(usage.source_bytes > 0);
        assert!(usage.node_table_bytes > 0);
        assert!(usage.token_table_bytes > 0);
        assert!(usage.child_table_bytes > 0);
    }
}
