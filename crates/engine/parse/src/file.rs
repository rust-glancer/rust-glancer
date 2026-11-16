use rg_syntax::{
    AstNode as _, Edition, LexedStr, Parse as SyntaxParse, SourceFile, SyntaxKind, SyntaxToken, ast,
};
use rg_text::RustEdition;

/// Parse source text with the Rust edition of its owning package.
///
/// This entry point does not assign a project file id or retain the syntax tree. It is intended for
/// editor text that a caller wants to inspect without adding it to the saved source inputs.
pub fn parse_source_file(source: &str, edition: RustEdition) -> SyntaxParse<SourceFile> {
    SourceFile::parse(source, syntax_edition(edition))
}

/// Inspect the token at a UTF-8 byte offset without building a syntax tree. Comments and
/// whitespace are included, so callers can decide whether they need to parse the source.
/// Token ranges include their start and exclude their end: a shared boundary selects the
/// following token, and offsets at or beyond the end of the source return `None`.
pub fn lexical_token_kind_at(
    source: &str,
    edition: RustEdition,
    offset: u32,
) -> Option<SyntaxKind> {
    let tokens = LexedStr::new(syntax_edition(edition), source);
    (0..tokens.len())
        .find(|&index| tokens.text_range(index).contains(&(offset as usize)))
        .map(|index| tokens.kind(index))
}

/// Recognize doc comments and every token inside a `doc` attribute through the same AST view.
/// This includes attribute punctuation and escapes, whose spelling is not documentation text.
pub fn is_documentation_token(token: &SyntaxToken) -> bool {
    if token
        .parent_ancestors()
        .filter_map(ast::AnyAttr::cast)
        .any(|attr| match attr {
            ast::AnyAttr::DocComment(_) => true,
            ast::AnyAttr::Attr(attr) => attr.simple_name().as_deref() == Some("doc"),
        })
    {
        return true;
    }

    // Inside macro token trees, doc comments are still bare tokens without attribute nodes.
    matches!(
        token.kind(),
        SyntaxKind::INNER_DOC_COMMENT | SyntaxKind::OUTER_DOC_COMMENT
    )
}

/// Translate the project edition into the parser's edition vocabulary.
pub fn syntax_edition(edition: RustEdition) -> Edition {
    match edition {
        RustEdition::Edition2015 => Edition::Edition2015,
        RustEdition::Edition2018 => Edition::Edition2018,
        RustEdition::Edition2021 => Edition::Edition2021,
        RustEdition::Edition2024 => Edition::Edition2024,
    }
}
