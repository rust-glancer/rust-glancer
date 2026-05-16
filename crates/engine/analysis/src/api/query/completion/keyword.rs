//! Conservative keyword and small-snippet completion assembly.
//!
//! Keyword completion runs on a speculative parse of the dirty editor snapshot.
//! Incomplete source like `f$0` or `ma$0` often cannot lower into a semantic
//! cursor site yet, but `ra_syntax` can still tell us whether a fake identifier
//! sits in item, statement, or expression position.

use ra_syntax::{AstNode as _, Edition, SourceFile, SyntaxKind, SyntaxToken, TextSize, ast};
use rg_def_map::TargetRef;
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget, KeywordCompletion,
    },
};

pub(super) struct KeywordCompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> KeywordCompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Collects keyword completions for plain source positions like `ma$0` or `fn $0`.
    pub(super) fn completions_at(
        &self,
        _target: TargetRef,
        _file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.completions_at_with_sort(offset, KeywordSortPosition::Primary)
    }

    /// Collects lower-priority keyword rows to append after semantic name completions.
    pub(super) fn overlay_completions_at(
        &self,
        _target: TargetRef,
        _file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.completions_at_with_sort(offset, KeywordSortPosition::Overlay)
    }

    fn completions_at_with_sort(
        &self,
        offset: u32,
        sort: KeywordSortPosition,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        // First identify the raw source prefix and the coarse syntactic region
        // around it; candidate selection is intentionally small and context-led.
        let Some(site) = KeywordCompletionSite::at(self.0, offset) else {
            return Ok(Vec::new());
        };

        // Keyword filtering mirrors editor prefix filtering, but we still set a
        // replacement edit so accepting `ma` replaces only the typed prefix.
        let prefix = site.prefix;
        let edit = CompletionEdit {
            replace: site.prefix_span,
        };
        let mut completions = KeywordCandidate::for_context(site.context)
            .iter()
            .filter(|candidate| candidate.label.starts_with(prefix))
            .map(|candidate| candidate.completion_item(edit, sort))
            .collect::<Vec<_>>();

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordSortPosition {
    Primary,
    Overlay,
}

impl KeywordSortPosition {
    /// Builds a sort bucket for standalone keyword rows or lower-priority overlays.
    fn sort_text(self, rank: u8, label: &str) -> String {
        match self {
            Self::Primary => format!("00-keyword:{rank:02}:{label}"),
            Self::Overlay => format!("~keyword:{rank:02}:{label}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordCompletionContext {
    Item,
    Statement,
    Expression,
}

/// Text-level cursor site for one partially typed keyword.
struct KeywordCompletionSite<'source> {
    context: KeywordCompletionContext,
    prefix: &'source str,
    prefix_span: Span,
}

impl<'source> KeywordCompletionSite<'source> {
    /// Finds the typed prefix and the surrounding context in the dirty source text.
    fn at(analysis: &'source Analysis<'_>, offset: u32) -> Option<Self> {
        Self::from_source(analysis.dirty_context?.text(), offset)
    }

    fn from_source(source: &'source str, offset: u32) -> Option<Self> {
        let cursor = usize::try_from(offset).ok()?;
        if cursor > source.len() || !source.is_char_boundary(cursor) {
            return None;
        }

        let prefix_start = source[..cursor]
            .char_indices()
            .rev()
            .find(|(_, ch)| !Self::is_identifier_continue(*ch))
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        let prefix = source.get(prefix_start..cursor)?;
        let syntax = KeywordSyntaxContext::parse(source, prefix_start, cursor)?;
        if !syntax.accepts_prefix_site(cursor == prefix_start) {
            return None;
        }

        let context = syntax.context()?;
        Some(Self {
            context,
            prefix,
            prefix_span: Span {
                text: TextSpan {
                    start: u32::try_from(prefix_start).ok()?,
                    end: offset,
                },
            },
        })
    }

    fn is_identifier_continue(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }
}

/// Speculative syntax around the keyword prefix.
struct KeywordSyntaxContext {
    marker: SyntaxToken,
}

impl KeywordSyntaxContext {
    const MARKER: &'static str = "__rg_completion";

    /// Replaces the typed prefix with a stable identifier so the parser can classify the site.
    fn parse(source: &str, prefix_start: usize, cursor: usize) -> Option<Self> {
        let mut speculative =
            String::with_capacity(source.len() - (cursor - prefix_start) + Self::MARKER.len());
        speculative.push_str(source.get(..prefix_start)?);
        speculative.push_str(Self::MARKER);
        speculative.push_str(source.get(cursor..)?);

        let file = SourceFile::parse(&speculative, Edition::CURRENT).tree();
        let marker_offset = TextSize::from(u32::try_from(prefix_start).ok()?);
        let marker = file
            .syntax()
            .token_at_offset(marker_offset)
            .right_biased()?;

        Some(Self { marker })
    }

    /// Rejects syntax positions that are not plain keyword slots.
    fn accepts_prefix_site(&self, empty_prefix: bool) -> bool {
        if self.marker.text() != Self::MARKER || !self.marker.kind().is_any_identifier() {
            return false;
        }

        let previous = self.previous_non_trivia_token();
        if empty_prefix
            && previous.as_ref().is_some_and(|token| {
                matches!(token.kind(), SyntaxKind::L_PAREN | SyntaxKind::COMMA)
            })
        {
            return false;
        }
        if previous.as_ref().is_some_and(|token| {
            matches!(
                token.kind(),
                SyntaxKind::DOT | SyntaxKind::COLON | SyntaxKind::LIFETIME_IDENT
            )
        }) {
            return false;
        }

        !self.inside_use_item()
    }

    /// Classifies the marker by its AST ancestors and nearby non-trivia token.
    fn context(&self) -> Option<KeywordCompletionContext> {
        if !self.inside_block_expr()? {
            return Some(KeywordCompletionContext::Item);
        }

        if self.at_statement_boundary() {
            Some(KeywordCompletionContext::Statement)
        } else {
            Some(KeywordCompletionContext::Expression)
        }
    }

    fn inside_block_expr(&self) -> Option<bool> {
        Some(
            self.marker
                .parent()?
                .ancestors()
                .any(|node| ast::BlockExpr::can_cast(node.kind())),
        )
    }

    fn inside_use_item(&self) -> bool {
        self.marker.parent().is_some_and(|parent| {
            parent
                .ancestors()
                .any(|node| ast::Use::can_cast(node.kind()))
        })
    }

    fn at_statement_boundary(&self) -> bool {
        self.previous_non_trivia_token().is_none_or(|token| {
            matches!(
                token.kind(),
                SyntaxKind::L_CURLY | SyntaxKind::R_CURLY | SyntaxKind::SEMICOLON
            )
        })
    }

    fn previous_non_trivia_token(&self) -> Option<SyntaxToken> {
        let mut token = self.marker.prev_token();
        while let Some(previous) = token {
            if !previous.kind().is_trivia() {
                return Some(previous);
            }
            token = previous.prev_token();
        }
        None
    }
}

/// One keyword row and its optional snippet body.
#[derive(Debug, Clone, Copy)]
struct KeywordCandidate {
    keyword: KeywordCompletion,
    label: &'static str,
    snippet: Option<&'static str>,
    sort_rank: u8,
}

impl KeywordCandidate {
    const ITEM: &'static [Self] = &[
        Self::new(
            KeywordCompletion::Fn,
            "fn",
            Some("fn ${1:name}(${2:args}) {\n    $0\n}"),
            0,
        ),
        Self::new(
            KeywordCompletion::Struct,
            "struct",
            Some("struct ${1:Name} {\n    $0\n}"),
            1,
        ),
        Self::new(KeywordCompletion::Enum, "enum", None, 2),
        Self::new(KeywordCompletion::Trait, "trait", None, 3),
        Self::new(
            KeywordCompletion::Impl,
            "impl",
            Some("impl ${1:Type} {\n    $0\n}"),
            4,
        ),
        Self::new(
            KeywordCompletion::ImplFor,
            "impl for",
            Some("impl ${1:Trait} for ${2:Type} {\n    $0\n}"),
            5,
        ),
        Self::new(KeywordCompletion::Mod, "mod", None, 6),
        Self::new(KeywordCompletion::Use, "use", None, 7),
        Self::new(KeywordCompletion::Const, "const", None, 8),
        Self::new(KeywordCompletion::Static, "static", None, 9),
        Self::new(KeywordCompletion::Type, "type", None, 10),
    ];

    const STATEMENT: &'static [Self] = &[
        Self::new(
            KeywordCompletion::Let,
            "let",
            Some("let ${1:name} = $0;"),
            0,
        ),
        Self::new(KeywordCompletion::Return, "return", None, 1),
        Self::new(
            KeywordCompletion::If,
            "if",
            Some("if ${1:condition} {\n    $0\n}"),
            2,
        ),
        Self::new(
            KeywordCompletion::Match,
            "match",
            Some("match ${1:value} {\n    $0\n}"),
            3,
        ),
        Self::new(KeywordCompletion::While, "while", None, 4),
        Self::new(
            KeywordCompletion::Loop,
            "loop",
            Some("loop {\n    $0\n}"),
            5,
        ),
        Self::new(KeywordCompletion::For, "for", None, 6),
    ];

    const EXPRESSION: &'static [Self] = &[
        Self::new(
            KeywordCompletion::If,
            "if",
            Some("if ${1:condition} {\n    $0\n}"),
            0,
        ),
        Self::new(
            KeywordCompletion::Match,
            "match",
            Some("match ${1:value} {\n    $0\n}"),
            1,
        ),
        Self::new(
            KeywordCompletion::Loop,
            "loop",
            Some("loop {\n    $0\n}"),
            2,
        ),
        Self::new(KeywordCompletion::Return, "return", None, 3),
        Self::new(KeywordCompletion::True, "true", None, 4),
        Self::new(KeywordCompletion::False, "false", None, 5),
        Self::new(KeywordCompletion::Async, "async", None, 6),
        Self::new(KeywordCompletion::Move, "move", None, 7),
    ];

    const fn new(
        keyword: KeywordCompletion,
        label: &'static str,
        snippet: Option<&'static str>,
        sort_rank: u8,
    ) -> Self {
        Self {
            keyword,
            label,
            snippet,
            sort_rank,
        }
    }

    fn for_context(context: KeywordCompletionContext) -> &'static [Self] {
        match context {
            KeywordCompletionContext::Item => Self::ITEM,
            KeywordCompletionContext::Statement => Self::STATEMENT,
            KeywordCompletionContext::Expression => Self::EXPRESSION,
        }
    }

    fn completion_item(self, edit: CompletionEdit, sort: KeywordSortPosition) -> CompletionItem {
        let target = CompletionTarget::Keyword(self.keyword);
        CompletionItem {
            label: self.label.to_string(),
            kind: CompletionKind::Keyword,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(format!("keyword {}", self.label)),
            documentation: None,
            sort_text: sort.sort_text(self.sort_rank, self.label),
            insert_text: self
                .snippet
                .map(|snippet| CompletionInsertText::Snippet(snippet.to_string()))
                .unwrap_or(CompletionInsertText::Plain),
            edit: Some(edit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{KeywordCompletionContext, KeywordCompletionSite};

    #[test]
    fn classifies_keyword_sites_from_speculative_syntax() {
        let cases = [
            (
                "item position",
                "f$0",
                Some((KeywordCompletionContext::Item, "f")),
            ),
            (
                "statement position",
                "fn main() {\n    le$0\n}",
                Some((KeywordCompletionContext::Statement, "le")),
            ),
            (
                "expression position",
                "fn main() {\n    let _ = ma$0;\n}",
                Some((KeywordCompletionContext::Expression, "ma")),
            ),
            (
                "bare expression position",
                "fn main() {\n    let _ = $0;\n}",
                Some((KeywordCompletionContext::Expression, "")),
            ),
        ];

        for (label, fixture, expected) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let actual = KeywordCompletionSite::from_source(&source, offset)
                .map(|site| (site.context, site.prefix));

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
        ];

        for (label, fixture) in cases {
            let (source, offset) = source_with_cursor(fixture);

            assert!(
                KeywordCompletionSite::from_source(&source, offset).is_none(),
                "{label}"
            );
        }
    }

    fn source_with_cursor(fixture: &str) -> (String, u32) {
        let offset = fixture
            .find("$0")
            .expect("keyword fixture should include a cursor marker");
        let mut source = fixture.to_string();
        source.replace_range(offset..offset + "$0".len(), "");
        (
            source,
            u32::try_from(offset).expect("keyword fixture offset should fit into u32"),
        )
    }
}
