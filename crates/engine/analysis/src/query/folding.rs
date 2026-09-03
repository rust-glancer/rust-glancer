//! Exact-source folding ranges.
//!
//! Once a language server supplies folding ranges, editors such as VS Code use them instead of
//! indentation-derived ranges. The collector therefore covers ordinary Rust structure alongside
//! the typed comment and import ranges used by editor folding commands.

use std::collections::HashSet;

use rg_parse::Span;
use rg_syntax::{
    AstNode as _, AstToken as _, Direction, NodeOrToken, SourceFile,
    SyntaxKind::{
        ARG_LIST, ARRAY_EXPR, ASSOC_ITEM_LIST, BLOCK_EXPR, EXPR_STMT, EXTERN_ITEM_LIST,
        GENERIC_ARG_LIST, GENERIC_PARAM_LIST, ITEM_LIST, LET_STMT, MATCH_ARM_LIST, PARAM_LIST,
        RECORD_EXPR_FIELD_LIST, RECORD_FIELD_LIST, RECORD_PAT_FIELD_LIST, RET_TYPE, TOKEN_TREE,
        USE_TREE_LIST, VARIANT_LIST, WHERE_CLAUSE,
    },
    SyntaxNode, TextRange,
    ast::{self, HasVisibility as _},
};

use crate::{Fold, FoldKind};

#[derive(Default)]
pub(crate) struct SyntaxFoldCollector {
    folds: Vec<Fold>,
    visited_comments: HashSet<ast::Comment>,
    visited_imports: HashSet<ast::Use>,
}

impl SyntaxFoldCollector {
    /// Collect every fold from the source text shown by the editor.
    pub(crate) fn collect(syntax: &SourceFile) -> Vec<Fold> {
        let mut collector = Self::default();

        for element in syntax.syntax().descendants_with_tokens() {
            match element {
                NodeOrToken::Token(token) => collector.comment(token),
                NodeOrToken::Node(node) => collector.node(node),
            }
        }

        // Protocol clients expect ranges in source order. Put an outer range before a nested range
        // when both begin at the same offset.
        collector.folds.sort_by(|left, right| {
            left.span
                .text
                .start
                .cmp(&right.span.text.start)
                .then_with(|| right.span.text.end.cmp(&left.span.text.end))
        });
        collector.folds
    }

    fn comment(&mut self, token: rg_syntax::SyntaxToken) {
        let Some(comment) = ast::Comment::cast(token) else {
            return;
        };
        if self.visited_comments.contains(&comment) {
            return;
        }

        // Multiline comment tokens already contain their entire block. Line comments are
        // separate tokens and are grouped below only when their flavor stays the same.
        if comment.text().contains('\n') {
            self.push(comment.syntax().text_range(), FoldKind::Comment);
            return;
        }

        if let Some(range) = self.contiguous_comment_range(comment) {
            self.push(range, FoldKind::Comment);
        }
    }

    fn node(&mut self, node: SyntaxNode) {
        if let Some(function) = ast::Fn::cast(node.clone())
            && Self::spans_multiple_lines(&node)
        {
            // A multiline parameter list can be folded on its own, but the surrounding function
            // range preserves the whole-function fold provided by indentation.
            let has_multiline_parameters = function
                .param_list()
                .is_some_and(|parameters| Self::spans_multiple_lines(parameters.syntax()));
            if has_multiline_parameters {
                let start = function
                    .fn_token()
                    .map(|token| token.text_range().start())
                    .unwrap_or_else(|| node.text_range().start());
                let end = function
                    .body()
                    .map(|body| body.syntax().text_range().end())
                    .unwrap_or_else(|| node.text_range().end());
                self.push(TextRange::new(start, end), FoldKind::Code);
            }
            return;
        }

        if Self::is_structural_fold(&node) && Self::spans_multiple_lines(&node) {
            self.push(node.text_range(), FoldKind::Code);
            return;
        }

        if let Some(import) = ast::Use::cast(node.clone())
            && let Some(range) = self.contiguous_import_range(import)
        {
            self.push(range, FoldKind::Imports);
        }

        // Braced expressions already contribute a more precise nested fold. Only add the whole
        // arm for multiline expression shapes such as method chains.
        if let Some(match_arm) = ast::MatchArm::cast(node)
            && let Some(expression) = match_arm.expr()
            && !Self::is_structural_fold(expression.syntax())
            && Self::spans_multiple_lines(expression.syntax())
        {
            self.push(expression.syntax().text_range(), FoldKind::Code);
        }
    }

    fn contiguous_comment_range(&mut self, first: ast::Comment) -> Option<TextRange> {
        self.visited_comments.insert(first.clone());

        let group_kind = first.kind();
        if !group_kind.shape.is_line() {
            return None;
        }

        let mut last = first.clone();
        for element in first.syntax().siblings_with_tokens(Direction::Next) {
            match element {
                NodeOrToken::Token(token) => {
                    if let Some(whitespace) = ast::Whitespace::cast(token.clone())
                        && !whitespace.spans_multiple_lines()
                    {
                        continue;
                    }
                    if let Some(comment) = ast::Comment::cast(token)
                        && comment.kind() == group_kind
                    {
                        self.visited_comments.insert(comment.clone());
                        last = comment;
                        continue;
                    }
                    break;
                }
                NodeOrToken::Node(_) => break,
            }
        }

        (first != last).then(|| {
            TextRange::new(
                first.syntax().text_range().start(),
                last.syntax().text_range().end(),
            )
        })
    }

    fn contiguous_import_range(&mut self, first: ast::Use) -> Option<TextRange> {
        if !self.visited_imports.insert(first.clone()) {
            return None;
        }

        let mut last = first.clone();
        let mut last_visibility = first.visibility();
        for element in first.syntax().siblings_with_tokens(Direction::Next) {
            let node = match element {
                NodeOrToken::Token(token) => {
                    if let Some(whitespace) = ast::Whitespace::cast(token)
                        && !whitespace.spans_multiple_lines()
                    {
                        continue;
                    }
                    break;
                }
                NodeOrToken::Node(node) => node,
            };

            let Some(next) = ast::Use::cast(node) else {
                break;
            };
            let next_visibility = next.visibility();
            let same_visibility = match (&last_visibility, &next_visibility) {
                (None, None) => true,
                (Some(left), Some(right)) => left.syntax().text() == right.syntax().text(),
                _ => false,
            };
            if !same_visibility {
                break;
            }

            self.visited_imports.insert(next.clone());
            last_visibility = next_visibility;
            last = next;
        }

        (first != last).then(|| {
            TextRange::new(
                first.syntax().text_range().start(),
                last.syntax().text_range().end(),
            )
        })
    }

    fn is_structural_fold(node: &SyntaxNode) -> bool {
        // A tail expression is not wrapped in EXPR_STMT, so recognize its position inside the
        // surrounding block explicitly.
        if let Some(block) = node
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(ast::BlockExpr::cast)
            && block.tail_expr().is_some_and(|tail| tail.syntax() == node)
        {
            return true;
        }

        matches!(
            node.kind(),
            ARG_LIST
                | PARAM_LIST
                | GENERIC_ARG_LIST
                | GENERIC_PARAM_LIST
                | ARRAY_EXPR
                | RET_TYPE
                | WHERE_CLAUSE
                | ASSOC_ITEM_LIST
                | RECORD_FIELD_LIST
                | RECORD_PAT_FIELD_LIST
                | RECORD_EXPR_FIELD_LIST
                | ITEM_LIST
                | EXTERN_ITEM_LIST
                | USE_TREE_LIST
                | BLOCK_EXPR
                | MATCH_ARM_LIST
                | VARIANT_LIST
                | TOKEN_TREE
                | EXPR_STMT
                | LET_STMT
        )
    }

    fn spans_multiple_lines(node: &SyntaxNode) -> bool {
        node.text().contains_char('\n')
    }

    fn push(&mut self, range: TextRange, kind: FoldKind) {
        self.folds.push(Fold {
            span: Span::from_text_range(range),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use rg_syntax::{Edition, SourceFile};

    use super::SyntaxFoldCollector;
    use crate::FoldKind;

    #[test]
    fn folds_line_and_block_comment_flavors_without_crossing_boundaries() {
        let source = r#"// ordinary one
//// ordinary two

/// outer one
/// outer two
//! inner one
//! inner two

/* ordinary
block */
/** outer
block */
/*! inner
block */
"#;

        let comments = folds(source)
            .into_iter()
            .filter(|(kind, _)| *kind == FoldKind::Comment)
            .collect::<Vec<_>>();

        assert_eq!(
            comments,
            [
                (
                    FoldKind::Comment,
                    "// ordinary one\n//// ordinary two".to_string()
                ),
                (
                    FoldKind::Comment,
                    "/// outer one\n/// outer two".to_string()
                ),
                (
                    FoldKind::Comment,
                    "//! inner one\n//! inner two".to_string()
                ),
                (FoldKind::Comment, "/* ordinary\nblock */".to_string()),
                (FoldKind::Comment, "/** outer\nblock */".to_string()),
                (FoldKind::Comment, "/*! inner\nblock */".to_string()),
            ]
        );
    }

    #[test]
    fn keeps_import_and_rust_structure_folds() {
        let source = r#"use std::fmt;
use std::io;

fn demo(
    value: usize,
) {
    let values = [
        value,
    ];
}
"#;
        let folds = folds(source);

        assert!(folds.contains(&(FoldKind::Imports, "use std::fmt;\nuse std::io;".to_string())));
        assert!(
            folds.iter().any(|(kind, text)| {
                *kind == FoldKind::Code && text == "[\n        value,\n    ]"
            })
        );
        assert!(folds.iter().any(|(kind, text)| {
            *kind == FoldKind::Code && text.starts_with("fn demo(") && text.ends_with('}')
        }));
    }

    #[test]
    fn ignores_single_comments() {
        let source = "// alone\nfn flat() {}\n";
        let comments = folds(source)
            .into_iter()
            .filter(|(kind, _)| *kind == FoldKind::Comment)
            .collect::<Vec<_>>();

        assert!(comments.is_empty());
    }

    fn folds(source: &str) -> Vec<(FoldKind, String)> {
        let syntax = SourceFile::parse(source, Edition::Edition2024).tree();
        let mut folds = SyntaxFoldCollector::collect(&syntax);
        folds.sort_by_key(|fold| fold.span.text.start);
        folds
            .into_iter()
            .map(|fold| {
                let start = usize::try_from(fold.span.text.start)
                    .expect("fold start should fit into usize");
                let end =
                    usize::try_from(fold.span.text.end).expect("fold end should fit into usize");
                (fold.kind, source[start..end].to_string())
            })
            .collect()
    }
}
