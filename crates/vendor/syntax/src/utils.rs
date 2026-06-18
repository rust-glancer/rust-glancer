//! A set of utils methods to reuse on other abstraction levels

use crate::{AstNode, SyntaxKind};

#[inline]
pub fn is_raw_identifier(name: &str, edition: parser::Edition) -> bool {
    let is_keyword = SyntaxKind::from_keyword(name, edition).is_some();
    is_keyword && !matches!(name, "self" | "crate" | "super" | "Self")
}

/// Compacts syntax by normalizing trivia while preserving each non-trivia token exactly.
pub fn normalized_syntax_text(node: &impl AstNode) -> String {
    let mut text = String::new();
    let mut pending_trivia = false;

    for token in node
        .syntax()
        .descendants_with_tokens()
        .filter_map(|it| it.into_token())
    {
        if token.kind().is_trivia() {
            pending_trivia = !text.is_empty();
            continue;
        }

        if pending_trivia {
            text.push(' ');
            pending_trivia = false;
        }
        text.push_str(token.text());
    }

    text
}
