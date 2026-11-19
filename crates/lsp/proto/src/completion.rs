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
}

/// The LSP item and the import the server can resolve for it.
///
/// An importable row can carry a source anchor along with the path. The server passes both
/// back to the editor in the row's `data` so they are available when it later asks to resolve.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CompletionCandidate {
    pub item: CompletionItem,
    pub import: Option<CompletionImport>,
    pub import_site: Option<CompletionImportSite>,
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
}

/// A short stretch of source immediately before the name being completed.
///
/// When the target changes, look for that stretch again and check the name that follows it.
/// This can find either `widget.ren$0` or `widget.render$0()` after the client adds the call.
/// The row carries the anchor so resolve can work after the original buffer is gone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompletionImportSite {
    before: String,
    replaced: String,
    cursor: usize,
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
