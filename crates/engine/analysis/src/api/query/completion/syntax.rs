//! Shared syntax context for cursor-sensitive completion routing.
//!
//! Completion providers often need the same low-level facts: the typed prefix,
//! nearby tokens, and whether a speculative identifier lands in item, statement,
//! or expression position. This module keeps those source-shape questions in
//! one place so semantic resolvers can stay focused on producing candidates.

use rg_parse::{Span, TextSpan};
use rg_syntax::{AstNode as _, Edition, SourceFile, SyntaxKind, SyntaxToken, TextSize, ast};

/// Lazily builds syntax context only for completion paths that need source recovery.
pub(super) struct CompletionSyntaxContextCache<'source> {
    source_text: Option<&'source str>,
    offset: u32,
    loaded: bool,
    context: Option<CompletionSyntaxContext<'source>>,
}

impl<'source> CompletionSyntaxContextCache<'source> {
    pub(super) fn new(source_text: Option<&'source str>, offset: u32) -> Self {
        Self {
            source_text,
            offset,
            loaded: false,
            context: None,
        }
    }

    /// Returns parsed request-source context, building it at most once per request.
    pub(super) fn get(&mut self) -> Option<&CompletionSyntaxContext<'source>> {
        if !self.loaded {
            self.context = CompletionSyntaxContext::at(self.source_text, self.offset);
            self.loaded = true;
        }

        self.context.as_ref()
    }
}

/// Parsed view of the dirty source around one completion offset.
pub(super) struct CompletionSyntaxContext<'source> {
    prefix: CompletionPrefix<'source>,
    marker: SyntaxToken,
}

impl<'source> CompletionSyntaxContext<'source> {
    const MARKER: &'static str = "__rg_completion";

    /// Builds syntax context from the request-local editor buffer.
    pub(super) fn at(source_text: Option<&'source str>, offset: u32) -> Option<Self> {
        Self::from_source(source_text?, offset)
    }

    fn from_source(source: &'source str, offset: u32) -> Option<Self> {
        let cursor = usize::try_from(offset).ok()?;
        if cursor > source.len() || !source.is_char_boundary(cursor) {
            return None;
        }

        let prefix_start = Self::prefix_start(source, cursor);
        let prefix = CompletionPrefix {
            text: source.get(prefix_start..cursor)?,
            span: Span {
                text: TextSpan {
                    start: u32::try_from(prefix_start).ok()?,
                    end: offset,
                },
            },
        };

        let marker = Self::marker_token(source, prefix_start, cursor)?;
        Some(Self { prefix, marker })
    }

    /// Returns the raw identifier prefix that should be replaced by a completion item.
    pub(super) fn prefix(&self) -> CompletionPrefix<'source> {
        self.prefix
    }

    /// Classifies this cursor as a keyword completion site, if keywords fit here.
    pub(super) fn keyword_position(&self) -> Option<KeywordSyntaxPosition> {
        if !self.accepts_keyword_site() {
            return None;
        }

        if !self.inside_block_expr()? {
            return Some(KeywordSyntaxPosition::Item);
        }

        if self.at_statement_boundary() {
            Some(KeywordSyntaxPosition::Statement)
        } else {
            Some(KeywordSyntaxPosition::Expression)
        }
    }

    /// Returns true when the marker follows a plain dot access like `self.$0`.
    pub(super) fn after_dot(&self) -> bool {
        self.previous_non_trivia_token()
            .is_some_and(|token| token.kind() == SyntaxKind::DOT)
    }

    /// Returns true when the marker follows a path qualifier like `crate::$0`.
    pub(super) fn after_colon_colon(&self) -> bool {
        self.previous_non_trivia_token()
            .is_some_and(|token| token.kind() == SyntaxKind::COLON2)
    }

    /// Returns true when the marker lives syntactically inside a `use` item.
    pub(super) fn inside_use_item(&self) -> bool {
        self.marker.parent().is_some_and(|parent| {
            parent
                .ancestors()
                .any(|node| ast::Use::can_cast(node.kind()))
        })
    }

    /// Returns the nearest meaningful token before the speculative marker.
    pub(super) fn previous_non_trivia_token(&self) -> Option<SyntaxToken> {
        let mut token = self.marker.prev_token();
        while let Some(previous) = token {
            if !previous.kind().is_trivia() {
                return Some(previous);
            }
            token = previous.prev_token();
        }
        None
    }

    fn accepts_keyword_site(&self) -> bool {
        if self.marker.text() != Self::MARKER || !self.marker.kind().is_any_identifier() {
            return false;
        }

        if self.prefix.is_empty()
            && self.previous_non_trivia_token().is_some_and(|token| {
                matches!(token.kind(), SyntaxKind::L_PAREN | SyntaxKind::COMMA)
            })
        {
            return false;
        }
        if self.after_dot() || self.after_colon_colon() {
            return false;
        }
        if self.previous_non_trivia_token().is_some_and(|token| {
            matches!(token.kind(), SyntaxKind::COLON | SyntaxKind::LIFETIME_IDENT)
        }) {
            return false;
        }

        !self.inside_use_item()
    }

    fn inside_block_expr(&self) -> Option<bool> {
        Some(
            self.marker
                .parent()?
                .ancestors()
                .any(|node| ast::BlockExpr::can_cast(node.kind())),
        )
    }

    fn at_statement_boundary(&self) -> bool {
        self.previous_non_trivia_token().is_none_or(|token| {
            matches!(
                token.kind(),
                SyntaxKind::L_CURLY | SyntaxKind::R_CURLY | SyntaxKind::SEMICOLON
            )
        })
    }

    fn marker_token(source: &str, prefix_start: usize, cursor: usize) -> Option<SyntaxToken> {
        let mut speculative =
            String::with_capacity(source.len() - (cursor - prefix_start) + Self::MARKER.len());
        speculative.push_str(source.get(..prefix_start)?);
        speculative.push_str(Self::MARKER);
        speculative.push_str(source.get(cursor..)?);

        // TODO: Thread the real package edition through completion syntax context.
        let file = SourceFile::parse(&speculative, Edition::CURRENT).tree();
        let marker_offset = TextSize::from(u32::try_from(prefix_start).ok()?);
        file.syntax().token_at_offset(marker_offset).right_biased()
    }

    fn prefix_start(source: &str, cursor: usize) -> usize {
        source[..cursor]
            .char_indices()
            .rev()
            .find(|(_, ch)| !Self::is_identifier_continue(*ch))
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0)
    }

    fn is_identifier_continue(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }
}

/// Identifier text and edit span already typed at the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompletionPrefix<'source> {
    text: &'source str,
    span: Span,
}

impl<'source> CompletionPrefix<'source> {
    pub(super) fn text(self) -> &'source str {
        self.text
    }

    pub(super) fn span(self) -> Span {
        self.span
    }

    pub(super) fn is_empty(self) -> bool {
        self.text.is_empty()
    }
}

/// Coarse keyword region used by the conservative keyword resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeywordSyntaxPosition {
    Item,
    Statement,
    Expression,
}

#[cfg(test)]
mod tests {
    use super::{CompletionSyntaxContext, KeywordSyntaxPosition};

    #[test]
    fn computes_prefix_and_replacement_span() {
        let (source, offset) = source_with_cursor("fn main() {\n    let value = ma$0;\n}");
        let syntax = CompletionSyntaxContext::from_source(&source, offset)
            .expect("keyword syntax context should be created");
        let prefix = syntax.prefix();

        assert_eq!(prefix.text(), "ma");
        assert_eq!(prefix.span().text.start, 28);
        assert_eq!(prefix.span().text.end, 30);
    }

    #[test]
    fn classifies_keyword_positions_from_speculative_syntax() {
        let cases = [
            (
                "item position",
                "f$0",
                Some((KeywordSyntaxPosition::Item, "f")),
            ),
            (
                "statement position",
                "fn main() {\n    le$0\n}",
                Some((KeywordSyntaxPosition::Statement, "le")),
            ),
            (
                "expression position",
                "fn main() {\n    let _ = ma$0;\n}",
                Some((KeywordSyntaxPosition::Expression, "ma")),
            ),
            (
                "bare expression position",
                "fn main() {\n    let _ = $0;\n}",
                Some((KeywordSyntaxPosition::Expression, "")),
            ),
        ];

        for (label, fixture, expected) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let actual = CompletionSyntaxContext::from_source(&source, offset)
                .and_then(|syntax| Some((syntax.keyword_position()?, syntax.prefix().text())));

            assert_eq!(actual, expected, "{label}");
        }
    }

    #[test]
    fn rejects_keyword_sites_inside_non_code_syntax() {
        let cases = [
            ("line comment", "fn main() {\n    // ma$0\n}"),
            ("block comment", "fn main() {\n    /* ma$0 */\n}"),
            ("string literal", r#"fn main() { let _ = "ma$0"; }"#),
            (
                "raw string literal",
                r##"fn main() { let _ = r#"ma$0"#; }"##,
            ),
            ("use item", "use ma$0;"),
            ("field access", "fn main() { value.ma$0 }"),
            ("path qualifier", "fn main() { crate::ma$0 }"),
        ];

        for (label, fixture) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let actual = CompletionSyntaxContext::from_source(&source, offset)
                .and_then(|syntax| syntax.keyword_position());

            assert_eq!(actual, None, "{label}");
        }
    }

    #[test]
    fn exposes_common_token_neighborhood_predicates() {
        let cases = [
            ("dot access", "fn main() { self.$0 }", true, false, false),
            (
                "path qualifier",
                "fn main() { crate::$0 }",
                false,
                true,
                false,
            ),
            ("use path", "use std::collections::$0;", false, true, true),
        ];

        for (label, fixture, after_dot, after_colon_colon, inside_use) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let syntax = CompletionSyntaxContext::from_source(&source, offset)
                .expect("syntax context should be created");

            assert_eq!(syntax.after_dot(), after_dot, "{label}: after_dot");
            assert_eq!(
                syntax.after_colon_colon(),
                after_colon_colon,
                "{label}: after_colon_colon"
            );
            assert_eq!(syntax.inside_use_item(), inside_use, "{label}: use item");
        }
    }

    fn source_with_cursor(fixture: &str) -> (String, u32) {
        let offset = fixture
            .find("$0")
            .expect("syntax fixture should include a cursor marker");
        let mut source = fixture.to_string();
        source.replace_range(offset..offset + "$0".len(), "");
        (
            source,
            u32::try_from(offset).expect("syntax fixture offset should fit into u32"),
        )
    }
}
