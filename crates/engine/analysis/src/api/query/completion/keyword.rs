//! Conservative keyword and small-snippet completion assembly.
//!
//! Keyword completion runs on a speculative parse of the dirty editor snapshot.
//! Incomplete source like `f$0` or `ma$0` often cannot lower into a semantic
//! cursor site yet, but `rg_syntax` can still tell us whether a fake identifier
//! sits in item, statement, or expression position.

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget, KeywordCompletion,
};

use super::{
    CompletionClientCapabilities,
    syntax::{CompletionSyntaxContext, KeywordSyntaxPosition},
};

pub(super) struct KeywordCompletionResolver {
    client_capabilities: CompletionClientCapabilities,
}

impl KeywordCompletionResolver {
    pub(super) fn new(client_capabilities: CompletionClientCapabilities) -> Self {
        Self {
            client_capabilities,
        }
    }

    /// Collects keyword completions for plain source positions like `ma$0` or `fn $0`.
    pub(super) fn completions(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.completions_at_with_sort(syntax, KeywordSortPosition::Primary)
    }

    /// Collects lower-priority keyword rows to append after semantic name completions.
    pub(super) fn overlay_completions(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.completions_at_with_sort(syntax, KeywordSortPosition::Overlay)
    }

    fn completions_at_with_sort(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
        sort: KeywordSortPosition,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        // First identify the raw source prefix and the coarse syntactic region
        // around it; candidate selection is intentionally small and context-led.
        let Some(syntax) = syntax else {
            return Ok(Vec::new());
        };
        let Some(context) = syntax.keyword_position() else {
            return Ok(Vec::new());
        };
        let prefix = syntax.prefix();

        // Keyword filtering mirrors editor prefix filtering, but we still set a
        // replacement edit so accepting `ma` replaces only the typed prefix.
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut completions = KeywordCandidate::for_context(context)
            .iter()
            .filter(|candidate| candidate.label.starts_with(prefix.text()))
            .map(|candidate| candidate.completion_item(edit, sort, self.client_capabilities))
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

    fn for_context(context: KeywordSyntaxPosition) -> &'static [Self] {
        match context {
            KeywordSyntaxPosition::Item => Self::ITEM,
            KeywordSyntaxPosition::Statement => Self::STATEMENT,
            KeywordSyntaxPosition::Expression => Self::EXPRESSION,
        }
    }

    fn completion_item(
        self,
        edit: CompletionEdit,
        sort: KeywordSortPosition,
        client_capabilities: CompletionClientCapabilities,
    ) -> CompletionItem {
        let target = CompletionTarget::Keyword(self.keyword);
        CompletionItem {
            label: self.label.to_string(),
            kind: CompletionKind::Keyword,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(format!("keyword {}", self.label)),
            documentation: None,
            sort_text: sort.sort_text(self.sort_rank, self.label),
            insert_text: if client_capabilities.snippet_support {
                self.snippet
                    .map(|snippet| CompletionInsertText::Snippet(snippet.to_string()))
                    .unwrap_or(CompletionInsertText::Plain)
            } else {
                CompletionInsertText::Plain
            },
            edit: Some(edit),
        }
    }
}
