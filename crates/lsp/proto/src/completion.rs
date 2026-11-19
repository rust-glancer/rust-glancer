use std::ops::Range;

use gen_lsp_types::{CompletionItem, CompletionItemKind, Position, Uri};
use serde::{Deserialize, Serialize};

use crate::{OpenDocumentsRevision, TargetDocumentRevision};

/// Completion client features that the analysis engine needs while rendering items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct CompletionClientCapabilities {
    pub snippet_support: bool,
    pub resolve_additional_text_edits: bool,
}

impl CompletionClientCapabilities {
    pub fn from_lsp_client_capabilities(capabilities: &gen_lsp_types::ClientCapabilities) -> Self {
        let item = capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref());
        Self {
            snippet_support: item.and_then(|item| item.snippet_support).unwrap_or(false),
            resolve_additional_text_edits: item
                .and_then(|item| item.resolve_support.as_ref())
                .is_some_and(|support| {
                    support
                        .properties
                        .iter()
                        .any(|name| name == "additionalTextEdits")
                }),
        }
    }
}

/// Completion rows plus the engine's refresh policy for the captured cursor.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct CompletionResult {
    pub items: Vec<CompletionCandidate>,
    pub is_incomplete: bool,
    /// All auto-import rows replace the same name at the completion cursor.
    pub import_site: Option<CompletionImportSite>,
}

/// The LSP item and the import the server can resolve for it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CompletionCandidate {
    pub item: CompletionItem,
    pub import: Option<CompletionImport>,
}

/// An import path in canonical spelling, with the binding mode carried separately.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompletionImport {
    pub path: String,
    pub anonymous: bool,
}

/// The name and declaration kind selected in an older editor snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompletionImportCandidate {
    pub label: String,
    pub kind: CompletionItemKind,
    /// Present when the cursor was relocated in a changed document.
    pub inline_module_path: Option<Vec<String>>,
}

/// A short stretch of source immediately before the name being completed.
///
/// When the target changes, look for that stretch again and check the name that follows it.
/// This can find either `widget.ren$0` or `widget.render$0()` after the client adds the call.
/// The row carries the anchor so resolve can work after the original buffer is gone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompletionImportSite {
    pub inline_module_path: Vec<String>,
    before: String,
    replaced: String,
    cursor: usize,
}

impl CompletionImportSite {
    /// Save the original cursor within the replacement so it can be restored if the name
    /// stays the same and edits above it move the site.
    pub fn capture(
        text: &str,
        cursor: usize,
        replace: Range<usize>,
        inline_module_path: Vec<String>,
    ) -> Option<Self> {
        let replaced = text.get(replace.clone())?;
        let cursor = cursor.checked_sub(replace.start)?;
        if !replaced.is_char_boundary(cursor) {
            return None;
        }

        // Keep the text immediately before the name, including the receiver and dot for a
        // method on a short expression. Repeated source is ambiguous even before the edit.
        let anchor_start = text[..replace.start]
            .char_indices()
            .rev()
            .nth(63)
            .map_or(0, |(offset, _)| offset);
        let before = &text[anchor_start..replace.start];
        (Self::unique_anchor_end(text, before)? == replace.start).then(|| Self {
            inline_module_path,
            before: before.to_owned(),
            replaced: replaced.to_owned(),
            cursor,
        })
    }

    /// Locate the selected name for import validation. The anchor must still be unique.
    pub fn offset_in(&self, text: &str, label: &str) -> Option<usize> {
        let start = Self::unique_anchor_end(text, &self.before)?;
        let tail = &text[start..];
        let starts_with_name = |name: &str| {
            !name.is_empty()
                && tail.strip_prefix(name).is_some_and(|rest| {
                    rest.chars().next().is_none_or(|ch| {
                        ch.is_whitespace() || (ch.is_ascii_punctuation() && ch != '_')
                    })
                })
        };

        // An accepted snippet may have added arguments. Check the name at its end even
        // if the editor's cursor has moved into those arguments.
        if starts_with_name(label) {
            Some(start + label.len())
        } else if starts_with_name(&self.replaced) && self.replaced.is_char_boundary(self.cursor) {
            Some(start + self.cursor)
        } else {
            None
        }
    }

    fn unique_anchor_end(text: &str, before: &str) -> Option<usize> {
        let first_width = before.chars().next()?.len_utf8();
        let start = text.find(before)?;
        // Start at the next character so overlapping matches count as ambiguous as well.
        (!text[start + first_width..].contains(before)).then_some(start + before.len())
    }
}

/// Data round-tripped by the client when it selects an auto-import row.
///
/// The target document revision determines whether `position` can still be used directly.
/// The editor revision also tracks sibling documents, whose unsaved changes can affect the
/// candidate even if its own document is unchanged.
#[derive(Debug, Deserialize, Serialize)]
pub struct CompletionImportData {
    pub uri: Uri,
    pub position: Position,
    pub document: TargetDocumentRevision,
    pub editor_revision: OpenDocumentsRevision,
    pub site: Option<CompletionImportSite>,
    #[serde(flatten)]
    pub import: CompletionImport,
}
