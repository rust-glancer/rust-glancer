//! A set of utils methods to reuse on other abstraction levels

use crate::{AstNode, AstToken, SyntaxKind, ast};

#[inline]
/// Checks that the name is an identifier.
/// This also means that it is not a strict keyword.
/// But it may be a weak keyword.
pub fn is_identifier(name: &str, edition: parser::Edition) -> bool {
    if rustc_lexer::is_ident(name) {
        if let Some(syntax_kind) = SyntaxKind::from_keyword(name, edition)
            && syntax_kind.is_strict_keyword(edition)
        {
            false
        } else {
            true
        }
    } else {
        false
    }
}

#[inline]
pub fn is_raw_identifier(name: &str, edition: parser::Edition) -> bool {
    let is_keyword = SyntaxKind::from_keyword(name, edition).is_some();
    is_keyword && !matches!(name, "self" | "crate" | "super" | "Self")
}

/// Compacts syntax by treating whitespace and comments as separators.
/// Other tokens keep their original spelling.
pub fn normalized_syntax_text(node: &impl AstNode) -> String {
    let mut text = String::new();
    let mut pending_trivia = false;

    for token in node
        .syntax()
        .descendants_with_tokens()
        .filter_map(|it| it.into_token())
    {
        if token.kind().is_trivia() || ast::AnyComment::can_cast(token.kind()) {
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
