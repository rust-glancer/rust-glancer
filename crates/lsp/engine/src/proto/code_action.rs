//! Conversion from analysis actions to LSP actions that already contain their document edits.
//!
//! Analysis actions still contain UTF-8 spans and know nothing about document identity. This is
//! the single boundary that converts those spans to UTF-16 ranges and attaches the captured URI
//! and document version, so an editor cannot apply the action to a different buffer revision.

use anyhow::Context as _;
use ls_types::{
    CodeAction as LspCodeAction, CodeActionKind as LspCodeActionKind, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};
use rg_analysis::{CodeAction, CodeActionKind};
use rg_parse::LineIndex;

use crate::proto::position;

/// Convert one validated single-document action into an LSP action with a versioned workspace
/// edit.
pub(crate) fn code_action(
    document_path: &std::path::Path,
    document_version: Option<i32>,
    line_index: &LineIndex,
    action: CodeAction,
) -> anyhow::Result<LspCodeAction> {
    let uri = Uri::from_file_path(document_path).with_context(|| {
        format!(
            "while attempting to convert code-action path `{}` to URI",
            document_path.display()
        )
    })?;
    let edits = action
        .edits
        .into_iter()
        .map(|edit| {
            OneOf::Left(TextEdit {
                range: position::range(line_index, edit.replace),
                new_text: edit.new_text,
            })
        })
        .collect();
    let document_edit = TextDocumentEdit {
        text_document: OptionalVersionedTextDocumentIdentifier {
            uri,
            version: document_version,
        },
        edits,
    };
    Ok(LspCodeAction {
        title: action.title,
        kind: Some(match action.kind {
            CodeActionKind::QuickFix => LspCodeActionKind::QUICKFIX,
            CodeActionKind::RefactorRewrite => LspCodeActionKind::REFACTOR_REWRITE,
        }),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![document_edit])),
            change_annotations: None,
        }),
        command: None,
        is_preferred: action.is_preferred.then_some(true),
        disabled: None,
        data: None,
    })
}
