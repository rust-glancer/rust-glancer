//! Transport-neutral source changes offered for one captured editor document.
//!
//! Analysis decides whether an action is safe and expresses every edit in UTF-8 source
//! coordinates. The LSP boundary later attaches the document URI and captured version, then
//! converts those coordinates to the client's UTF-16 ranges.

use rg_parse::Span;

/// One complete editor action discovered from current syntax and saved semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAction {
    /// Label shown in the editor's action menu.
    pub title: String,
    /// Family used by the client's action-kind filter.
    pub kind: CodeActionKind,
    /// Whether this is the unambiguous default among otherwise applicable actions.
    pub is_preferred: bool,
    /// Non-overlapping changes to the document that received the request.
    pub edits: Vec<CodeActionEdit>,
}

/// Action families produced by analysis and understood by the LSP boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodeActionKind {
    /// A repair for source that does not resolve or satisfy a language requirement.
    QuickFix,
    /// A source rewrite that preserves the resolved program meaning.
    RefactorRewrite,
}

/// One plain-text change inside the document that owns the action request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeActionEdit {
    /// UTF-8 source span to replace; an empty span represents an insertion.
    pub replace: Span,
    /// Plain source text inserted in place of `replace`.
    pub new_text: String,
}
