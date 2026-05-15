//! Conservative keyword and small-snippet completion assembly.
//!
//! Keyword completion is intentionally text-based. It runs on the dirty editor
//! snapshot so incomplete source like `f$0` or `ma$0` can still produce useful
//! rows even when the parser cannot lower that fragment into a semantic cursor
//! site.

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
        let source = analysis.dirty_context?.text();
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
        if !Self::accepts_prefix_site(source, prefix_start, cursor) {
            return None;
        }

        let context = SourceContext::before_offset(source, prefix_start)?;
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

    /// Rejects obvious non-keyword sites before the broader context scan runs.
    fn accepts_prefix_site(source: &str, prefix_start: usize, cursor: usize) -> bool {
        if Self::inside_comment_or_string(source, cursor) {
            return false;
        }

        let previous = source[..prefix_start]
            .chars()
            .rev()
            .find(|ch| !ch.is_whitespace());
        if cursor == prefix_start && matches!(previous, Some('(' | ',')) {
            return false;
        }
        if matches!(previous, Some('.') | Some(':') | Some('\'')) {
            return false;
        }

        !Self::looks_like_use_path(source, prefix_start)
    }

    /// Provides a cheap guard against suggesting keywords inside trivia and strings.
    fn inside_comment_or_string(source: &str, cursor: usize) -> bool {
        let line_start = source[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let line_before_cursor = &source[line_start..cursor];
        if line_before_cursor.contains("//") {
            return true;
        }

        let block_comment_open = source[..cursor].rfind("/*");
        let block_comment_close = source[..cursor].rfind("*/");
        if block_comment_open > block_comment_close {
            return true;
        }

        line_before_cursor
            .chars()
            .fold((false, false), |(inside, escaped), ch| {
                if escaped {
                    (inside, false)
                } else if ch == '\\' {
                    (inside, inside)
                } else if ch == '"' {
                    (!inside, false)
                } else {
                    (inside, false)
                }
            })
            .0
    }

    fn looks_like_use_path(source: &str, prefix_start: usize) -> bool {
        let line_start = source[..prefix_start]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let before_prefix = source[line_start..prefix_start].trim_start();

        before_prefix.starts_with("use ") || before_prefix.starts_with("pub use ")
    }

    fn is_identifier_continue(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }
}

/// Classifies a keyword site into item, statement, or expression position.
struct SourceContext;

impl SourceContext {
    fn before_offset(source: &str, offset: usize) -> Option<KeywordCompletionContext> {
        let stack = BraceStack::scan(source, offset);
        if !stack.is_body() {
            return Some(KeywordCompletionContext::Item);
        }

        if Self::is_statement_position(source, offset) {
            Some(KeywordCompletionContext::Statement)
        } else {
            Some(KeywordCompletionContext::Expression)
        }
    }

    fn is_statement_position(source: &str, offset: usize) -> bool {
        let previous = source[..offset]
            .chars()
            .rev()
            .find(|ch| !ch.is_whitespace());

        previous.is_none_or(|ch| matches!(ch, '{' | '}' | ';'))
    }
}

/// Approximate stack of brace owners before the completion offset.
#[derive(Debug, Default)]
struct BraceStack {
    contexts: Vec<BraceContext>,
}

impl BraceStack {
    fn scan(source: &str, end: usize) -> Self {
        let mut scanner = BraceScanner::new(source, end);
        scanner.scan()
    }

    fn is_body(&self) -> bool {
        self.contexts
            .iter()
            .any(|context| matches!(context, BraceContext::Body))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BraceContext {
    Body,
    Item,
}

/// Scans source text just far enough to classify braces as body or item braces.
struct BraceScanner<'a> {
    source: &'a str,
    end: usize,
    stack: BraceStack,
    state: TextScanState,
}

impl<'a> BraceScanner<'a> {
    fn new(source: &'a str, end: usize) -> Self {
        Self {
            source,
            end,
            stack: BraceStack::default(),
            state: TextScanState::Code,
        }
    }

    fn scan(&mut self) -> BraceStack {
        let mut chars = self.source[..self.end].char_indices().peekable();
        while let Some((idx, ch)) = chars.next() {
            if self.state.advance(ch, chars.peek().map(|(_, ch)| *ch)) {
                chars.next();
                continue;
            }
            if !matches!(self.state, TextScanState::Code) {
                continue;
            }

            match ch {
                '{' => self.push_context(idx),
                '}' => {
                    self.stack.contexts.pop();
                }
                _ => {}
            }
        }

        std::mem::take(&mut self.stack)
    }

    fn push_context(&mut self, brace: usize) {
        // A full parser would know the brace owner directly. Here the nearby
        // header is enough to separate body blocks from item-like blocks.
        let header = Self::brace_header_before(&self.source[..brace]);
        let context = if Self::header_has_any_word(
            header,
            &[
                "fn", "if", "else", "match", "loop", "while", "for", "unsafe",
            ],
        ) || header.trim_end().ends_with("=>")
        {
            BraceContext::Body
        } else if Self::header_has_any_word(header, &["struct", "enum", "trait", "impl", "mod"]) {
            BraceContext::Item
        } else if self.stack.is_body() {
            BraceContext::Body
        } else {
            BraceContext::Item
        };
        self.stack.contexts.push(context);
    }

    fn brace_header_before(source_before_brace: &str) -> &str {
        let start = source_before_brace
            .rfind(|ch| matches!(ch, '{' | '}' | ';'))
            .map(|idx| idx + 1)
            .unwrap_or(0);
        &source_before_brace[start..]
    }

    fn header_has_any_word(header: &str, words: &[&str]) -> bool {
        header
            .split(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
            .filter(|word| !word.is_empty())
            .any(|word| words.contains(&word))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextScanState {
    Code,
    LineComment,
    BlockComment,
    String,
}

impl TextScanState {
    /// Advances the lightweight lexer enough to avoid counting braces in trivia and literals.
    fn advance(&mut self, ch: char, next: Option<char>) -> bool {
        match (*self, ch, next) {
            (Self::Code, '/', Some('/')) => {
                *self = Self::LineComment;
                true
            }
            (Self::Code, '/', Some('*')) => {
                *self = Self::BlockComment;
                true
            }
            (Self::Code, '"', _) => {
                *self = Self::String;
                false
            }
            (Self::LineComment, '\n', _) => {
                *self = Self::Code;
                false
            }
            (Self::BlockComment, '*', Some('/')) => {
                *self = Self::Code;
                true
            }
            (Self::String, '"', _) => {
                *self = Self::Code;
                false
            }
            _ => false,
        }
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
